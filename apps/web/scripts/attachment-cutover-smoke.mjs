import WebSocket from "ws";

const server = process.env.OWLMUX_E2E_SERVER;
const machineId = process.env.OWLMUX_E2E_MACHINE_ID;
const apiKey = process.env.OWLMUX_E2E_API_KEY;
if (!server || !machineId || !apiKey) throw new Error("missing attachment cutover configuration");

const sessionName = "owlmux-cutover";
const origin = server.replace(/^ws/, "http");
const socket = new WebSocket(`${server}/attachment/v1/machines/${machineId}`, { origin });
const deadline = setTimeout(() => fail("attachment cutover smoke timed out"), 30_000);
let projection = null;
let ready = false;
let complete = false;
const liveChunks = [];

function fail(message) {
  clearTimeout(deadline);
  console.error(message);
  process.exit(1);
}

function assert(condition, message) {
  if (!condition) fail(message);
}

function tokens(bytes) {
  return [...bytes.toString("latin1").matchAll(/CUTOVER-(\d{8})/g)].map((match) =>
    Number.parseInt(match[1], 10),
  );
}

function verifyCutover() {
  if (!ready || projection === null) return;
  const liveTokens = tokens(Buffer.concat(liveChunks));
  if (liveTokens.length < 100) return;
  const snapshotTokens = tokens(Buffer.concat(projection.snapshotChunks));
  assert(snapshotTokens.length > 0, "cutover token missing from final snapshot");
  const snapshotToken = snapshotTokens.at(-1);
  assert(
    liveTokens[0] === snapshotToken + 1,
    `snapshot/live cutover lost or duplicated output: snapshot=${snapshotToken} live=${liveTokens[0]}`,
  );
  for (let index = 1; index < liveTokens.length; index += 1) {
    assert(
      liveTokens[index] === liveTokens[index - 1] + 1,
      "live cutover tokens are not contiguous",
    );
  }
  complete = true;
  socket.send(JSON.stringify({ type: "workspace.detach" }));
  socket.close(1000, "complete");
}

socket.on("open", () => {
  socket.send(JSON.stringify({ type: "auth.api_key", api_key: apiKey }));
});

socket.on("message", (bytes) => {
  assert(bytes.length <= 32_768, "attachment frame exceeds contract bound");
  const frame = JSON.parse(bytes.toString());
  if (frame.type === "session.list") {
    const session = frame.sessions.find((candidate) => candidate.name === sessionName);
    assert(session !== undefined, "cutover session missing");
    socket.send(
      JSON.stringify({
        type: "session.select",
        machine_connection_epoch: frame.machine_connection_epoch,
        selection_epoch: frame.selection_epoch,
        session_id: session.session_id,
        session_created: session.session_created,
      }),
    );
  } else if (frame.type === "workspace.projection") {
    assert(frame.panes.length === 4, "cutover projection does not contain four panes");
    const active = frame.panes.find((pane) => pane.active);
    assert(active !== undefined, "cutover projection has no active pane");
    ready = false;
    liveChunks.length = 0;
    projection = {
      epoch: frame.workspace_epoch,
      paneId: active.pane_id,
      snapshotChunks: [],
      snapshotComplete: false,
    };
  } else if (frame.type === "workspace.pane_snapshot") {
    assert(projection !== null, "cutover snapshot arrived without projection");
    if (frame.pane_id === projection.paneId) {
      projection.snapshotChunks.push(Buffer.from(frame.data_base64, "base64url"));
      if (frame.final) projection.snapshotComplete = true;
    }
  } else if (frame.type === "workspace.phase" && frame.phase === "ready") {
    assert(projection?.snapshotComplete, "ready arrived before the active pane snapshot");
    ready = true;
    verifyCutover();
  } else if (frame.type === "workspace.output") {
    if (
      projection !== null &&
      frame.workspace_epoch === projection.epoch &&
      frame.pane_id === projection.paneId
    ) {
      liveChunks.push(Buffer.from(frame.data_base64, "base64url"));
      verifyCutover();
    }
  } else if (frame.type === "workspace.error") {
    fail(`attachment cutover failed: ${frame.code}`);
  }
});

socket.on("unexpected-response", (_request, response) => {
  fail(`attachment cutover upgrade failed: ${response.statusCode}`);
});
socket.on("error", (error) => fail(error.message));
socket.on("close", () => {
  clearTimeout(deadline);
  if (!complete) fail("attachment closed before cutover verification");
  console.log("continuous snapshot/live cutover tokens verified");
});
