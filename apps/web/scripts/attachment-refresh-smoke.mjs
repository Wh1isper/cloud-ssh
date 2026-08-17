import WebSocket from "ws";

const server = process.env.OWLMUX_E2E_SERVER;
const machineId = process.env.OWLMUX_E2E_MACHINE_ID;
const apiKey = process.env.OWLMUX_E2E_API_KEY;
if (!server || !machineId || !apiKey) throw new Error("missing attachment refresh configuration");

const origin = server.replace(/^ws/, "http");
const socket = new WebSocket(`${server}/attachment/v1/machines/${machineId}`, { origin });
const deadline = setTimeout(() => fail("attachment refresh timed out"), 40_000);
let pending = null;
let initialReady = false;
let refreshed = false;
let completed = false;

function fail(message) {
  clearTimeout(deadline);
  console.error(message);
  process.exit(1);
}

function assert(condition, message) {
  if (!condition) fail(message);
}

socket.on("open", () => {
  socket.send(JSON.stringify({ type: "auth.api_key", api_key: apiKey }));
});

socket.on("message", (bytes) => {
  assert(bytes.length <= 32_768, "attachment frame exceeds contract bound");
  const frame = JSON.parse(bytes.toString());
  if (frame.type === "session.list") {
    if (!initialReady) {
      assert(frame.sessions.length > 0, "initial chooser has no session");
      const session = frame.sessions[0];
      socket.send(
        JSON.stringify({
          type: "session.select",
          selection_epoch: frame.selection_epoch,
          session_id: session.session_id,
          session_created: session.session_created,
        }),
      );
    } else {
      assert(refreshed, "target session exited before projection refresh");
      assert(frame.sessions.length === 0, "session exit did not return to zero-session chooser");
      completed = true;
      console.log("target session exit returned attachment to a fresh chooser");
      socket.send(JSON.stringify({ type: "workspace.detach" }));
      socket.close(1000, "complete");
    }
  } else if (frame.type === "workspace.projection") {
    pending = {
      epoch: frame.workspace_epoch,
      panes: frame.panes.length,
      paneIds: new Set(frame.panes.map((pane) => pane.pane_id)),
      complete: new Set(),
      content: new Map(frame.panes.map((pane) => [pane.pane_id, []])),
    };
  } else if (frame.type === "workspace.pane_snapshot") {
    assert(pending !== null, "snapshot arrived without projection");
    assert(frame.workspace_epoch === pending.epoch, "snapshot epoch changed");
    assert(pending.paneIds.has(frame.pane_id), "snapshot pane is unknown");
    pending.content.get(frame.pane_id).push(Buffer.from(frame.data_base64, "base64url"));
    if (frame.final) pending.complete.add(frame.pane_id);
  } else if (frame.type === "workspace.phase" && frame.phase === "ready") {
    assert(pending !== null, "ready arrived without projection");
    assert(pending.complete.size === pending.paneIds.size, "ready arrived before snapshots");
    if (!initialReady) {
      assert(pending.panes === 2, "initial projection does not have two panes");
      initialReady = true;
      console.log("workspace-ready");
    } else if (pending.panes === 3) {
      const content = [...pending.content.values()].map((chunks) => Buffer.concat(chunks));
      assert(
        content.some((snapshot) => snapshot.includes("tertiary-ready")),
        "new pane output was not captured during projection refresh",
      );
      refreshed = true;
      console.log("projection-refreshed");
    }
    pending = null;
  } else if (frame.type === "workspace.error") {
    fail(`attachment failed: ${frame.code}`);
  }
});

socket.on("unexpected-response", (_request, response) => {
  fail(`attachment upgrade failed: ${response.statusCode}`);
});
socket.on("error", (error) => fail(error.message));
socket.on("close", () => {
  clearTimeout(deadline);
  if (!completed) fail("attachment closed before refresh and session-exit evidence");
});
