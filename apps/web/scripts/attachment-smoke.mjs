import WebSocket from "ws";

const server = process.env.OWLMUX_E2E_SERVER;
const machineId = process.env.OWLMUX_E2E_MACHINE_ID;
const apiKey = process.env.OWLMUX_E2E_API_KEY;
if (!server || !machineId || !apiKey) throw new Error("missing attachment smoke configuration");

const origin = server.replace(/^ws/, "http");
const socket = new WebSocket(`${server}/attachment/v1/machines/${machineId}`, { origin });
const deadline = setTimeout(() => fail("attachment smoke timed out"), 30_000);
let pendingProjection = null;
let projected = false;

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
    assert(frame.sessions.length > 0, "target fixture has no tmux session");
    assert(frame.tmux_client_version.startsWith("tmux "), "missing client version");
    assert(frame.tmux_server_version?.startsWith("tmux "), "missing running server version");
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
    assert(frame.panes.length === 2, "target-authoritative projection does not contain two panes");
    assert(new Set(frame.panes.map((pane) => pane.pane_id)).size === 2, "duplicate pane identity");
    assert(
      frame.panes.filter((pane) => pane.active).length === 1,
      "invalid active pane cardinality",
    );
    for (const pane of frame.panes) {
      assert(pane.left + pane.width <= frame.window.width, "pane exceeds target window width");
      assert(pane.top + pane.height <= frame.window.height, "pane exceeds target window height");
    }
    pendingProjection = {
      epoch: frame.workspace_epoch,
      paneIds: new Set(frame.panes.map((pane) => pane.pane_id)),
      chunks: new Map(frame.panes.map((pane) => [pane.pane_id, []])),
      complete: new Set(),
    };
  } else if (frame.type === "workspace.pane_snapshot") {
    assert(pendingProjection !== null, "snapshot arrived without projection metadata");
    assert(frame.workspace_epoch === pendingProjection.epoch, "stale snapshot epoch");
    assert(pendingProjection.paneIds.has(frame.pane_id), "snapshot has unknown pane");
    pendingProjection.chunks.get(frame.pane_id).push(Buffer.from(frame.data_base64, "base64url"));
    if (frame.final) pendingProjection.complete.add(frame.pane_id);
  } else if (frame.type === "workspace.phase" && frame.phase === "ready") {
    assert(pendingProjection !== null, "ready arrived without projection");
    assert(
      pendingProjection.complete.size === pendingProjection.paneIds.size,
      "ready arrived before complete pane snapshots",
    );
    const contents = [...pendingProjection.chunks.values()].map((chunks) => Buffer.concat(chunks));
    assert(
      contents.some((content) => content.includes("primary-ready")),
      "primary pane marker missing",
    );
    assert(
      contents.some((content) => content.includes("secondary-ready")),
      "secondary pane marker missing",
    );
    projected = true;
    socket.send(JSON.stringify({ type: "workspace.detach" }));
    socket.close(1000, "complete");
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
  if (!projected) fail("attachment closed before projection");
  console.log("two-pane read-only attachment projection verified");
});
