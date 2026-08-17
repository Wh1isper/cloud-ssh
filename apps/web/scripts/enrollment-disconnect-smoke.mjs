import WebSocket from "ws";

const server = process.env.OWLMUX_E2E_SERVER;
const token = process.env.OWLMUX_E2E_ENROLLMENT_TOKEN;
if (!server || !token) throw new Error("missing enrollment disconnect configuration");

const socket = new WebSocket(`${server}/relay/v1/enroll`);
const deadline = setTimeout(() => fail("enrollment disconnect smoke timed out"), 10_000);
let accepted = false;

function fail(message) {
  clearTimeout(deadline);
  console.error(message);
  process.exit(1);
}

socket.on("open", () => socket.send(JSON.stringify({ type: "token", token })));
socket.on("message", (bytes) => {
  const frame = JSON.parse(bytes.toString());
  if (frame.type === "accepted") {
    accepted = true;
    socket.close(1000, "intentional disconnect");
  } else if (frame.type === "error") {
    fail(`enrollment token was rejected: ${frame.code}`);
  }
});
socket.on("error", (error) => fail(error.message));
socket.on("close", () => {
  clearTimeout(deadline);
  if (!accepted) fail("enrollment closed before durable token acceptance");
  console.log("enrollment disconnected after durable token acceptance");
});
