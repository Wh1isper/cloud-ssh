import WebSocket from "ws";

const server = process.env.OWLMUX_E2E_SERVER;
const machineId = process.env.OWLMUX_E2E_MACHINE_ID;
const apiKey = process.env.OWLMUX_E2E_API_KEY;
if (!server || !machineId || !apiKey) throw new Error("missing live output configuration");

const origin = server.replace(/^ws/, "http");
const socket = new WebSocket(`${server}/attachment/v1/machines/${machineId}`, { origin });
const deadline = setTimeout(() => fail("live output smoke timed out"), 30_000);
let selected = false;
let workspaceEpoch = null;
let targetPane = null;
let totalOutput = 0;
let sawNonUtf8 = false;
let tail = Buffer.alloc(0);

function fail(message) {
  clearTimeout(deadline);
  console.error(message);
  process.exit(1);
}

socket.on("open", () => socket.send(JSON.stringify({ type: "auth.api_key", api_key: apiKey })));
socket.on("message", (bytes) => {
  if (bytes.length > 32_768) fail("live output frame exceeds contract bound");
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
  } else if (frame.type === "workspace.projection") {
    workspaceEpoch = frame.workspace_epoch;
    targetPane = (frame.panes.find((pane) => pane.active) ?? frame.panes[0]).pane_id;
  } else if (frame.type === "workspace.phase" && frame.phase === "ready") {
    console.log("workspace-ready");
  } else if (
    frame.type === "workspace.output" &&
    frame.workspace_epoch === workspaceEpoch &&
    frame.pane_id === targetPane
  ) {
    const data = Buffer.from(frame.data_base64, "base64url");
    if (data.length > 16_384) fail("decoded output chunk exceeds contract bound");
    totalOutput += data.length;
    sawNonUtf8 ||= data.includes(0xff);
    tail = Buffer.concat([tail, data]).subarray(-128);
    if (tail.includes("LIVE-END")) {
      if (totalOutput < 32_768) fail("large live output was truncated");
      if (!sawNonUtf8) fail("non-UTF-8 live output byte was not preserved");
      socket.send(JSON.stringify({ type: "workspace.detach" }));
      socket.close(1000, "large output complete");
    }
  } else if (frame.type === "workspace.error") {
    fail(`attachment failed: ${frame.code}`);
  }
});
socket.on("error", (error) => fail(error.message));
socket.on("close", () => {
  clearTimeout(deadline);
  if (!tail.includes("LIVE-END")) fail("attachment closed before complete live output");
  console.log("bounded large and non-UTF-8 live output verified");
});
