import WebSocket from "ws";

const server = process.env.OWLMUX_E2E_SERVER;
const machineId = process.env.OWLMUX_E2E_MACHINE_ID;
const apiKey = process.env.OWLMUX_E2E_API_KEY;
if (!server || !machineId || !apiKey) throw new Error("missing route replacement configuration");

const origin = server.replace(/^ws/, "http");
const socket = new WebSocket(`${server}/attachment/v1/machines/${machineId}`, { origin });
const deadline = setTimeout(() => fail("route replacement smoke timed out"), 30_000);
let firstSelectionEpoch = null;
let selected = false;
let workspaceReady = false;
let replacementChooser = false;

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
  if (frame.type === "session.list" && firstSelectionEpoch === null) {
    if (frame.sessions.length === 0) fail("target fixture has no session");
    firstSelectionEpoch = frame.selection_epoch;
    const session = frame.sessions[0];
    selected = true;
    socket.send(
      JSON.stringify({
        type: "session.select",
        selection_epoch: frame.selection_epoch,
        session_id: session.session_id,
        session_created: session.session_created,
      }),
    );
  } else if (frame.type === "workspace.phase" && frame.phase === "ready" && selected) {
    workspaceReady = true;
    console.log("workspace-ready");
  } else if (frame.type === "session.list" && workspaceReady) {
    if (frame.selection_epoch === firstSelectionEpoch)
      fail("route replacement reused selection epoch");
    replacementChooser = true;
    socket.send(JSON.stringify({ type: "workspace.detach" }));
    socket.close(1000, "replacement chooser observed");
  } else if (frame.type === "workspace.error") {
    fail(`attachment failed instead of returning to chooser: ${frame.code}`);
  }
});

socket.on("error", (error) => fail(error.message));
socket.on("close", () => {
  clearTimeout(deadline);
  if (!replacementChooser)
    fail("attachment did not return to a fresh chooser after route replacement");
  console.log("route replacement returned workspace to a fresh chooser");
});
