import { randomUUID } from "node:crypto";

import WebSocket from "ws";

const server = process.env.OWLMUX_E2E_SERVER;
const machineId = process.env.OWLMUX_E2E_MACHINE_ID;
const apiKey = process.env.OWLMUX_E2E_API_KEY;
if (!server || !machineId || !apiKey) throw new Error("missing live output configuration");

const origin = server.replace(/^ws/, "http");
const socket = new WebSocket(`${server}/attachment/v1/machines/${machineId}`, { origin });
const messages = [];
const waiters = [];
let latestProjection = null;
let strictWorkspaceEpoch = null;
let fatalError = null;

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

function rejectWaiters(error) {
  for (const waiter of waiters.splice(0)) {
    clearTimeout(waiter.timer);
    waiter.reject(error);
  }
}

socket.on("message", (bytes) => {
  const frame = JSON.parse(bytes.toString());
  if (frame.type === "workspace.projection") {
    latestProjection = frame;
    if (strictWorkspaceEpoch !== null && frame.workspace_epoch !== strictWorkspaceEpoch) {
      fatalError = new Error("unexpected resynchronization during paced live-output test");
      rejectWaiters(fatalError);
      return;
    }
  }
  const index = waiters.findIndex((waiter) => waiter.predicate(frame));
  if (index >= 0) {
    const [waiter] = waiters.splice(index, 1);
    clearTimeout(waiter.timer);
    waiter.resolve(frame);
  } else {
    messages.push(frame);
  }
});
socket.on("error", rejectWaiters);
socket.on("close", (code, reason) => {
  rejectWaiters(new Error(`live-output socket closed ${code} ${reason.toString()}`));
});

function waitFor(predicate, message, milliseconds = 20_000) {
  if (fatalError !== null) return Promise.reject(fatalError);
  const index = messages.findIndex(predicate);
  if (index >= 0) return Promise.resolve(messages.splice(index, 1)[0]);
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(message)), milliseconds);
    waiters.push({ predicate, resolve, reject, timer });
  });
}

function request(type, fields) {
  const requestId = randomUUID();
  socket.send(JSON.stringify({ type, request_id: requestId, ...fields }));
  return requestId;
}

async function waitResult(requestId) {
  return waitFor(
    (frame) => frame.type === "operation.result" && frame.request_id === requestId,
    `operation result ${requestId} did not arrive`,
  );
}

async function main() {
  await new Promise((resolve, reject) => {
    socket.once("open", resolve);
    socket.once("error", reject);
  });
  socket.send(JSON.stringify({ type: "auth.api_key", api_key: apiKey }));

  const chooser = await waitFor(
    (frame) => frame.type === "session.list",
    "live-output chooser did not arrive",
  );
  const session = chooser.sessions.find((candidate) => candidate.name === "owlmux-live-output");
  assert(session !== undefined, "dedicated live-output session is missing");

  const claimId = request("writer.claim", {
    machine_connection_epoch: chooser.machine_connection_epoch,
    attachment_epoch: chooser.selection_epoch,
    columns: 80,
    rows: 24,
  });
  const claim = await waitResult(claimId);
  assert(claim.outcome === "succeeded", `writer claim failed: ${claim.code}`);

  socket.send(
    JSON.stringify({
      type: "session.select",
      machine_connection_epoch: chooser.machine_connection_epoch,
      selection_epoch: chooser.selection_epoch,
      session_id: session.session_id,
      session_created: session.session_created,
    }),
  );
  await waitFor(
    (frame) => frame.type === "workspace.projection",
    "live-output projection did not arrive",
  );
  await waitFor(
    (frame) => frame.type === "workspace.phase" && frame.phase === "ready",
    "live-output workspace did not become ready",
  );

  let syncSucceeded = false;
  for (let attempt = 0; attempt < 3 && !syncSucceeded; attempt += 1) {
    assert(latestProjection !== null, "live-output projection state is missing");
    const activePane = latestProjection.panes.find((pane) => pane.active);
    assert(activePane !== undefined, "live-output projection has no active pane");
    const syncId = request("pane.input", {
      machine_connection_epoch: latestProjection.machine_connection_epoch,
      workspace_epoch: latestProjection.workspace_epoch,
      pane_id: activePane.pane_id,
      data_base64: Buffer.from("0\n").toString("base64url"),
    });
    const sync = await waitResult(syncId);
    if (sync.outcome === "succeeded") syncSucceeded = true;
    else assert(sync.code === "stale_epoch", `live-output synchronization failed: ${sync.code}`);
  }
  assert(syncSucceeded, "live-output subscription synchronization was not dispatched");
  const hasSynchronizedTitle = () =>
    latestProjection?.panes.some((pane) => pane.active && pane.title === "owlmux-live-ready") ===
    true;
  if (!hasSynchronizedTitle()) {
    await waitFor(
      (frame) =>
        frame.type === "workspace.projection" &&
        frame.panes.some((pane) => pane.active && pane.title === "owlmux-live-ready"),
      "live-output subscription synchronization did not converge",
    );
  }

  let refreshSucceeded = false;
  for (let attempt = 0; attempt < 3 && !refreshSucceeded; attempt += 1) {
    assert(latestProjection !== null, "live-output projection state is missing");
    const refreshId = request("workspace.refresh", {
      machine_connection_epoch: latestProjection.machine_connection_epoch,
      workspace_epoch: latestProjection.workspace_epoch,
    });
    const refresh = await waitResult(refreshId);
    if (refresh.outcome === "succeeded") refreshSucceeded = true;
    else assert(refresh.code === "stale_epoch", `workspace refresh failed: ${refresh.code}`);
  }
  assert(refreshSucceeded, "live-output workspace did not stabilize");
  assert(latestProjection !== null, "refreshed live-output projection is missing");
  const projection = latestProjection;
  strictWorkspaceEpoch = projection.workspace_epoch;
  console.log("workspace-ready");

  const activePanes = projection.panes.filter((pane) => pane.active);
  assert(activePanes.length === 1, "live-output projection must have one active pane");
  const paneId = activePanes[0].pane_id;
  let totalPayload = 0;

  for (let round = 1; round <= 9; round += 1) {
    const inputId = request("pane.input", {
      machine_connection_epoch: projection.machine_connection_epoch,
      workspace_epoch: projection.workspace_epoch,
      pane_id: paneId,
      data_base64: Buffer.from(`${round}\n`).toString("base64url"),
    });
    const input = await waitResult(inputId);
    assert(input.outcome === "succeeded", `pane input failed: ${input.code}`);

    const marker = Buffer.concat([Buffer.from([0xff]), Buffer.from(`LIVE-${round}`)]);
    let output = Buffer.alloc(0);
    while (output.indexOf(marker) < 0) {
      const frame = await waitFor(
        (candidate) =>
          candidate.type === "workspace.output" &&
          candidate.workspace_epoch === projection.workspace_epoch &&
          candidate.pane_id === paneId,
        `live-output round ${round} did not complete`,
      );
      const chunk = Buffer.from(frame.data_base64, "base64url");
      assert(chunk.length <= 16_384, "decoded output chunk exceeds contract bound");
      output = Buffer.concat([output, chunk]);
      assert(output.length <= 8192, `live-output round ${round} exceeded its bound`);
    }
    const markerIndex = output.indexOf(marker);
    const payloadStart = markerIndex - 4096;
    assert(payloadStart >= 0, `live-output round ${round} payload length changed`);
    assert(
      output.subarray(payloadStart, markerIndex).every((byte) => byte === 0x78),
      `live-output round ${round} payload changed`,
    );
    totalPayload += 4096;
  }

  assert(
    latestProjection?.workspace_epoch === strictWorkspaceEpoch,
    "paced live output crossed a workspace epoch",
  );
  assert(totalPayload >= 32_768, "paced large live output was truncated");
  socket.send(JSON.stringify({ type: "workspace.detach" }));
  socket.close(1000, "large output complete");
  console.log("bounded large and non-UTF-8 live output verified in one workspace epoch");
}

try {
  await main();
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
} finally {
  if (socket.readyState === WebSocket.OPEN) socket.close(1000, "test complete");
  else if (socket.readyState !== WebSocket.CLOSED) socket.terminate();
}
