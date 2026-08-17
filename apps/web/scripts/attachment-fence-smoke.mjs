import WebSocket from "ws";

const server = process.env.OWLMUX_E2E_SERVER;
const machineId = process.env.OWLMUX_E2E_MACHINE_ID;
const apiKey = process.env.OWLMUX_E2E_API_KEY;
if (!server || !machineId || !apiKey) throw new Error("missing attachment fence configuration");

const origin = server.replace(/^ws/, "http");
const socket = new WebSocket(`${server}/attachment/v1/machines/${machineId}`, { origin });
const deadline = setTimeout(() => fail("attachment fence smoke timed out"), 45_000);
let selected = false;
let ready = false;
let closedAfterReady = false;

function fail(message) {
  clearTimeout(deadline);
  console.error(message);
  process.exit(1);
}

socket.on("open", () => socket.send(JSON.stringify({ type: "auth.api_key", api_key: apiKey })));
socket.on("message", (bytes) => {
  const frame = JSON.parse(bytes.toString());
  if (frame.type === "session.list" && !selected) {
    if (frame.sessions.length === 0) fail("target fixture has no session");
    selected = true;
    const session = frame.sessions[0];
    socket.send(
      JSON.stringify({
        type: "session.select",
        selection_epoch: frame.selection_epoch,
        session_id: session.session_id,
        session_created: session.session_created,
      }),
    );
  } else if (frame.type === "workspace.phase" && frame.phase === "ready") {
    ready = true;
    console.log("workspace-ready");
  } else if (frame.type === "workspace.error" && !ready) {
    fail(`attachment failed before fence: ${frame.code}`);
  }
});
socket.on("error", () => {
  // A hard fence may surface as either error then close or a direct close.
});
socket.on("close", () => {
  clearTimeout(deadline);
  closedAfterReady = ready;
  if (!closedAfterReady) fail("attachment closed before reaching ready");
  console.log("hard fence closed the active attachment");
});
