import WebSocket from "ws";

const server = process.env.OWLMUX_E2E_SERVER;
const machineId = process.env.OWLMUX_E2E_MACHINE_ID;
const apiKey = process.env.OWLMUX_E2E_API_KEY;
const origin = process.env.OWLMUX_E2E_ORIGIN ?? server?.replace(/^ws/, "http");
if (!server || !machineId || !apiKey || !origin) {
  throw new Error("missing attachment failure configuration");
}

const socket = new WebSocket(`${server}/attachment/v1/machines/${machineId}`, { origin });
const deadline = setTimeout(() => fail("owner-unreachable check timed out"), 15_000);
let observed = false;

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
  if (frame.type === "workspace.error") {
    if (frame.code !== "owner_unreachable") {
      fail(`unexpected error: ${frame.code}`);
    }
    observed = true;
  }
});

socket.on("close", () => {
  clearTimeout(deadline);
  if (!observed) fail("owner_unreachable was not reported");
  console.log("owner-unreachable-ok");
});

socket.on("error", (error) => fail(error.message));
