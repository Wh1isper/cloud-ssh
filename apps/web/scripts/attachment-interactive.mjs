import { randomUUID } from "node:crypto";

import WebSocket from "ws";

const server = process.env.OWLMUX_E2E_SERVER;
const machineId = process.env.OWLMUX_E2E_MACHINE_ID;
const apiKey = process.env.OWLMUX_E2E_API_KEY;
if (!server || !machineId || !apiKey)
  throw new Error("missing interactive attachment configuration");

const origin = server.replace(/^ws/, "http");

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

class Peer {
  constructor(name) {
    this.name = name;
    this.messages = [];
    this.waiters = [];
    this.socket = new WebSocket(`${server}/attachment/v1/machines/${machineId}`, { origin });
    this.socket.on("message", (bytes) => {
      const frame = JSON.parse(bytes.toString());
      const index = this.waiters.findIndex((waiter) => waiter.predicate(frame));
      if (index >= 0) {
        const [waiter] = this.waiters.splice(index, 1);
        clearTimeout(waiter.timer);
        waiter.resolve(frame);
      } else {
        this.messages.push(frame);
      }
    });
    const rejectWaiters = (reason) => {
      for (const waiter of this.waiters.splice(0)) {
        clearTimeout(waiter.timer);
        waiter.reject(reason);
      }
    };
    this.socket.on("close", (code, reason) => {
      rejectWaiters(new Error(`${this.name}: socket closed ${code} ${reason.toString()}`));
    });
    this.socket.on("error", (error) => rejectWaiters(error));
  }

  async open() {
    await new Promise((resolve, reject) => {
      this.socket.once("open", resolve);
      this.socket.once("error", reject);
    });
    this.send({ type: "auth.api_key", api_key: apiKey });
  }

  send(frame) {
    this.socket.send(JSON.stringify(frame));
  }

  wait(predicate, message, milliseconds = 20_000) {
    const index = this.messages.findIndex(predicate);
    if (index >= 0) return Promise.resolve(this.messages.splice(index, 1)[0]);
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error(`${this.name}: ${message}`)), milliseconds);
      this.waiters.push({ predicate, resolve, timer });
    });
  }

  request(type, fields) {
    const requestId = randomUUID();
    this.send({ type, request_id: requestId, ...fields });
    return requestId;
  }

  close() {
    if (this.socket.readyState === WebSocket.OPEN) {
      this.send({ type: "workspace.detach" });
      this.socket.close(1000, "complete");
    } else if (this.socket.readyState !== WebSocket.CLOSED) {
      this.socket.terminate();
    }
  }
}

async function waitChooser(peer) {
  return peer.wait((frame) => frame.type === "session.list", "chooser did not arrive");
}

async function waitResult(peer, requestId) {
  return peer.wait(
    (frame) => frame.type === "operation.result" && frame.request_id === requestId,
    `operation result ${requestId} did not arrive`,
  );
}

async function waitReady(peer) {
  const projection = await peer.wait(
    (frame) => frame.type === "workspace.projection",
    "workspace projection did not arrive",
  );
  await peer.wait(
    (frame) => frame.type === "workspace.phase" && frame.phase === "ready",
    "workspace did not become ready",
  );
  return projection;
}

async function sendText(peer, projection, text) {
  const requestId = peer.request("pane.input", {
    machine_connection_epoch: projection.machine_connection_epoch,
    workspace_epoch: projection.workspace_epoch,
    pane_id: projection.panes.find((pane) => pane.active).pane_id,
    data_base64: Buffer.from(text).toString("base64url"),
  });
  const result = await waitResult(peer, requestId);
  assert(result.outcome === "succeeded", `pane input did not succeed: ${result.code}`);
}

async function waitOutputMatch(peer, pattern, message) {
  const deadline = Date.now() + 20_000;
  let output = "";
  while (Date.now() < deadline) {
    const frame = await peer.wait(
      (candidate) => candidate.type === "workspace.output",
      message,
      deadline - Date.now(),
    );
    output += Buffer.from(frame.data_base64, "base64url").toString();
    const match = output.match(pattern);
    if (match) return match;
  }
  throw new Error(`${peer.name}: ${message}`);
}

const first = new Peer("first");
const second = new Peer("second");
try {
  await Promise.all([first.open(), second.open()]);
  const [firstChooser, secondChooser] = await Promise.all([
    waitChooser(first),
    waitChooser(second),
  ]);
  assert(
    firstChooser.machine_connection_epoch === secondChooser.machine_connection_epoch,
    "peers resolved different Machine connection epochs",
  );

  const claim = (peer, chooser) =>
    peer.request("writer.claim", {
      machine_connection_epoch: chooser.machine_connection_epoch,
      attachment_epoch: chooser.selection_epoch,
      columns: 100,
      rows: 30,
    });
  const firstClaim = claim(first, firstChooser);
  const secondClaim = claim(second, secondChooser);
  const [firstClaimResult, secondClaimResult] = await Promise.all([
    waitResult(first, firstClaim),
    waitResult(second, secondClaim),
  ]);
  const successfulClaims = [firstClaimResult, secondClaimResult].filter(
    (result) => result.outcome === "succeeded",
  );
  assert(
    successfulClaims.length === 1,
    "concurrent claims did not elect exactly one Browser writer",
  );

  const winner = firstClaimResult.outcome === "succeeded" ? first : second;
  const observer = winner === first ? second : first;
  const winnerChooser = winner === first ? firstChooser : secondChooser;
  const observerChooser = winner === first ? secondChooser : firstChooser;

  const createId = winner.request("session.create", {
    machine_connection_epoch: winnerChooser.machine_connection_epoch,
    selection_epoch: winnerChooser.selection_epoch,
    name: "owlmux-interactive",
  });
  const createResult = await waitResult(winner, createId);
  assert(createResult.outcome === "succeeded", `session creation failed: ${createResult.code}`);
  const refreshedWinnerChooser = await waitChooser(winner);
  const createdSession = refreshedWinnerChooser.sessions.find(
    (session) => session.name === "owlmux-interactive",
  );
  assert(createdSession !== undefined, "created session was not visible after fresh discovery");

  const refreshId = observer.request("session.refresh", {
    machine_connection_epoch: observerChooser.machine_connection_epoch,
    selection_epoch: observerChooser.selection_epoch,
  });
  assert(
    (await waitResult(observer, refreshId)).outcome === "succeeded",
    "observer refresh failed",
  );
  const refreshedObserverChooser = await waitChooser(observer);
  const observedCreatedSession = refreshedObserverChooser.sessions.find(
    (session) => session.session_id === createdSession.session_id,
  );
  assert(observedCreatedSession !== undefined, "observer did not discover the created session");

  for (const [peer, chooser] of [
    [winner, refreshedWinnerChooser],
    [observer, refreshedObserverChooser],
  ]) {
    peer.send({
      type: "session.select",
      machine_connection_epoch: chooser.machine_connection_epoch,
      selection_epoch: chooser.selection_epoch,
      session_id: createdSession.session_id,
      session_created: createdSession.session_created,
    });
  }
  const winnerProjection = await waitReady(winner);
  let observerProjection = await waitReady(observer);
  assert(
    winnerProjection.window.width === 100 && winnerProjection.window.height === 30,
    "writer dimensions were not applied before hydration",
  );
  if (observerProjection.window.width !== 100 || observerProjection.window.height !== 30) {
    observerProjection = await waitReady(observer);
  }
  assert(
    observerProjection.window.width === 100 && observerProjection.window.height === 30,
    "observer did not converge to the writer-authoritative geometry",
  );

  await sendText(winner, winnerProjection, "printf 'OWLMUX_WRITER_INPUT_OK\\n'\n");
  const writerOutput = await winner.wait(
    (frame) =>
      frame.type === "workspace.output" &&
      Buffer.from(frame.data_base64, "base64url").includes("OWLMUX_WRITER_INPUT_OK"),
    "writer input was not observed in live target output",
  );
  assert(
    writerOutput.workspace_epoch === winnerProjection.workspace_epoch,
    "writer output epoch changed",
  );

  const takeoverId = observer.request("writer.takeover", {
    machine_connection_epoch: observerProjection.machine_connection_epoch,
    attachment_epoch: observerProjection.workspace_epoch,
    columns: 90,
    rows: 28,
  });
  const takeoverProjection = await waitReady(observer);
  const takeoverResult = await waitResult(observer, takeoverId);
  assert(takeoverResult.outcome === "succeeded", `writer takeover failed: ${takeoverResult.code}`);
  assert(
    takeoverProjection.window.width === 90 && takeoverProjection.window.height === 28,
    "takeover dimensions were not installed authoritatively",
  );
  await winner.wait(
    (frame) => frame.type === "writer.state" && frame.role === "observer",
    "former writer was not demoted",
  );
  const demotedProjection = await waitReady(winner);
  assert(
    demotedProjection.window.width === 90 && demotedProjection.window.height === 28,
    "former writer did not converge after a refresh observed under dispatch contention",
  );

  const rejectedId = winner.request("pane.input", {
    machine_connection_epoch: winnerProjection.machine_connection_epoch,
    workspace_epoch: winnerProjection.workspace_epoch,
    pane_id: winnerProjection.panes.find((pane) => pane.active).pane_id,
    data_base64: Buffer.from("touch /tmp/owlmux-should-not-dispatch\n").toString("base64url"),
  });
  const rejected = await waitResult(winner, rejectedId);
  assert(
    rejected.outcome === "failed" && ["writer_required", "stale_epoch"].includes(rejected.code),
    `former writer input was not rejected before dispatch: ${rejected.outcome}/${rejected.code}`,
  );

  await sendText(observer, takeoverProjection, "printf 'OWLMUX_TAKEOVER_INPUT_OK\\n'\n");
  await observer.wait(
    (frame) =>
      frame.type === "workspace.output" &&
      Buffer.from(frame.data_base64, "base64url").includes("OWLMUX_TAKEOVER_INPUT_OK"),
    "takeover writer input was not observed",
  );

  await sendText(
    observer,
    takeoverProjection,
    '/usr/bin/tmux -L owlmux list-clients -F \'#{session_name}:#{client_flags}\' | /usr/bin/awk -F: \'$1=="owlmux-interactive" {f=substr($0,index($0,":")+1); r=index(f,"read-only"); i=index(f,"ignore-size"); if(r&&i)o++; else if(!r&&!i)w++; else if(r)ro++; else io++} END {printf "OWLMUX_CLIENT_%s_OK:%d:%d:%d:%d\\n","FLAGS",o,w,ro,io}\'\n',
  );
  const clientFlags = await waitOutputMatch(
    observer,
    /OWLMUX_CLIENT_FLAGS_OK:(\d+):(\d+):(\d+):(\d+)/,
    "target tmux client flags were not observed",
  );
  const clientFlagCounts = clientFlags.slice(1).map(Number);
  const [observerClientCount, writerClientCount] = clientFlagCounts;
  assert(
    observerClientCount >= 1,
    `target tmux did not retain a read-only ignore-size observer (${clientFlagCounts.join("/")})`,
  );
  assert(
    writerClientCount === 1,
    `target tmux did not retain exactly one writable OwlMux client (${clientFlagCounts.join("/")})`,
  );

  console.log(
    "interactive attachment verified: one writer, explicit create, literal input, observer geometry, target client flags, takeover, stale-writer rejection, and authoritative resize",
  );
} finally {
  first.close();
  second.close();
}
