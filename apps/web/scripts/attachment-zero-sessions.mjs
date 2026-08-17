import WebSocket from "ws";

const server = process.env.OWLMUX_E2E_SERVER;
const machineId = process.env.OWLMUX_E2E_MACHINE_ID;
const apiKey = process.env.OWLMUX_E2E_API_KEY;
if (!server || !machineId || !apiKey) throw new Error("missing zero-session configuration");

const origin = server.replace(/^ws/, "http");
const socket = new WebSocket(`${server}/attachment/v1/machines/${machineId}`, { origin });
const deadline = setTimeout(() => fail("zero-session smoke timed out"), 15_000);
let observed = false;

function fail(message) {
  clearTimeout(deadline);
  console.error(message);
  process.exit(1);
}

socket.on("open", () => socket.send(JSON.stringify({ type: "auth.api_key", api_key: apiKey })));
socket.on("message", (bytes) => {
  const frame = JSON.parse(bytes.toString());
  if (frame.type === "session.list") {
    if (frame.sessions.length !== 0) fail("zero-session chooser was not empty");
    if (frame.tmux_server_version !== null) fail("dead tmux server was reported as running");
    observed = true;
    socket.send(JSON.stringify({ type: "workspace.detach" }));
    socket.close(1000, "zero sessions observed");
  } else if (frame.type === "workspace.error") {
    fail(`zero-session probe failed: ${frame.code}`);
  }
});
socket.on("error", (error) => fail(error.message));
socket.on("close", () => {
  clearTimeout(deadline);
  if (!observed) fail("attachment closed before empty chooser");
  console.log("zero-session chooser verified without target creation");
});
