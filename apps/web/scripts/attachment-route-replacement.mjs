import WebSocket from "ws";

const server = process.env.OWLMUX_E2E_SERVER;
const machineId = process.env.OWLMUX_E2E_MACHINE_ID;
const apiKey = process.env.OWLMUX_E2E_API_KEY;
if (!server || !machineId || !apiKey) throw new Error("missing route replacement configuration");

const origin = server.replace(/^ws/, "http");
const socket = new WebSocket(`${server}/attachment/v1/machines/${machineId}`, { origin });
const deadline = setTimeout(() => fail("route replacement smoke timed out"), 30_000);
let selected = false;
let workspaceReady = false;

function fail(message) {
  clearTimeout(deadline);
  console.error(message);
  process.exit(1);
}

socket.on("open", () => {
  socket.send(JSON.stringify({ type: "auth.api_key", api_key: apiKey }));
});

socket.on("message", (bytes) => {
  const frame = JSON.parse(bytes.toString());
  if (frame.type === "session.list" && !selected) {
    if (frame.sessions.length === 0) fail("target fixture has no session");
    selected = true;
    const session = frame.sessions[0];
    socket.send(
      JSON.stringify({
        type: "session.select",
        machine_connection_epoch: frame.machine_connection_epoch,
        selection_epoch: frame.selection_epoch,
        session_id: session.session_id,
        session_created: session.session_created,
      }),
    );
  } else if (frame.type === "workspace.phase" && frame.phase === "ready" && selected) {
    workspaceReady = true;
    console.log("workspace-ready");
  } else if (frame.type === "session.list" && workspaceReady) {
    fail("stale attachment crossed a replaced Machine route");
  } else if (frame.type === "workspace.error") {
    fail(`attachment failed before route replacement: ${frame.code}`);
  }
});

socket.on("error", (error) => fail(error.message));
socket.on("close", () => {
  clearTimeout(deadline);
  if (!workspaceReady) fail("attachment closed before workspace was ready");
  console.log("route replacement closed the stale attachment");
});
