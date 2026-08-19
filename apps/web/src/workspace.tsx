import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type FormEvent,
  type PointerEvent as ReactPointerEvent,
} from "react";

import type {
  ApiClient,
  AttachmentFrame,
  AttachmentPane,
  AttachmentSession,
  AttachmentSessionSummary,
  AttachmentWindow,
} from "./client";
import {
  ATTACHMENT_MAX_DIMENSION,
  ATTACHMENT_MAX_GRID_CELLS,
  ATTACHMENT_MAX_INPUT_BYTES,
} from "./generated/contracts";
import type { MachineSummary } from "./generated/contracts";

type WorkspacePhase = Extract<AttachmentFrame, { type: "workspace.phase" }>["phase"];
type WorkspaceErrorCode = Extract<AttachmentFrame, { type: "workspace.error" }>["code"];
type ProjectionMetadata = Extract<AttachmentFrame, { type: "workspace.projection" }>;

interface InstalledProjection {
  metadata: ProjectionMetadata;
  snapshots: Map<string, Array<Uint8Array>>;
}

interface PendingProjection extends InstalledProjection {
  completed: Set<string>;
}

interface PaneSink {
  enqueue(data: Uint8Array): boolean;
}

interface CellSize {
  width: number;
  height: number;
}

interface PreviousSession {
  sessionCreated: number;
  sessionId: string;
  name: string;
}

export interface InteractiveWorkspaceProps {
  client: ApiClient;
  machine: MachineSummary;
  onAuthenticationFailure: () => void;
  onClose: () => void;
  onTitleChange: (title: string | null) => void;
  visible: boolean;
}

const MAX_RENDER_QUEUE_BYTES = 1024 * 1024;
const MIN_TERMINAL_COLUMNS = 20;
const MIN_TERMINAL_ROWS = 5;
const RESIZE_DEBOUNCE_MS = 180;

export function InteractiveWorkspace({
  client,
  machine,
  onAuthenticationFailure,
  onClose,
  onTitleChange,
  visible,
}: InteractiveWorkspaceProps) {
  const attachment = useRef<AttachmentSession | null>(null);
  const pendingProjection = useRef<PendingProjection | null>(null);
  const currentWorkspaceEpoch = useRef<string | null>(null);
  const paneSinks = useRef(new Map<string, PaneSink>());
  const pendingOutput = useRef(new Map<string, Array<Uint8Array>>());
  const pendingOutputBytes = useRef(0);
  const resizeSurface = useRef<HTMLDivElement | null>(null);
  const lastResize = useRef("");
  const terminalDiagnostic = useRef<string | null>(null);
  const [connectionAttempt, setConnectionAttempt] = useState(0);
  const [phase, setPhase] = useState<WorkspacePhase>("connecting");
  const [tmuxVersion, setTmuxVersion] = useState("");
  const [machineConnectionEpoch, setMachineConnectionEpoch] = useState("");
  const [selectionEpoch, setSelectionEpoch] = useState("");
  const [sessions, setSessions] = useState<Array<AttachmentSessionSummary>>([]);
  const [projection, setProjection] = useState<InstalledProjection | null>(null);
  const [writerRole, setWriterRole] = useState<"observer" | "writer">("observer");
  const [writerAvailable, setWriterAvailable] = useState(false);
  const [operationStatus, setOperationStatus] = useState("");
  const [error, setError] = useState("");
  const [cellSize, setCellSize] = useState<CellSize | null>(null);
  const [viewport, setViewport] = useState({ height: 0, width: 0 });
  const [previousSession, setPreviousSession] = useState<PreviousSession | null>(null);

  const resetProjection = useCallback(() => {
    pendingProjection.current = null;
    currentWorkspaceEpoch.current = null;
    paneSinks.current.clear();
    pendingOutput.current.clear();
    pendingOutputBytes.current = 0;
    setProjection(null);
    setCellSize(null);
  }, []);

  const failRenderer = useCallback(() => {
    const message = "The Browser renderer could not keep up with bounded terminal output.";
    terminalDiagnostic.current = message;
    setError(message);
    setPhase("failed");
    setWriterRole("observer");
    setWriterAvailable(false);
    resetProjection();
    const active = attachment.current;
    attachment.current = null;
    active?.dispose();
  }, [resetProjection]);

  const registerSink = useCallback(
    (paneId: string, sink: PaneSink | null) => {
      if (sink === null) {
        paneSinks.current.delete(paneId);
        return;
      }
      paneSinks.current.set(paneId, sink);
      const queued = pendingOutput.current.get(paneId) ?? [];
      pendingOutput.current.delete(paneId);
      for (const data of queued) {
        pendingOutputBytes.current -= data.length;
        if (!sink.enqueue(data)) {
          failRenderer();
          return;
        }
      }
    },
    [failRenderer],
  );

  const reportCellSize = useCallback((next: CellSize) => {
    if (!Number.isFinite(next.width) || !Number.isFinite(next.height)) return;
    if (next.width <= 0 || next.height <= 0) return;
    setCellSize((current) => {
      if (
        current !== null &&
        Math.abs(current.width - next.width) < 0.05 &&
        Math.abs(current.height - next.height) < 0.05
      ) {
        return current;
      }
      return next;
    });
  }, []);

  useEffect(() => {
    lastResize.current = "";
    terminalDiagnostic.current = null;
    let active: AttachmentSession | null = null;
    let disposed = false;
    let fatalPhaseReceived = false;
    const isCurrent = () => isCurrentAttachmentAttempt(disposed, active, attachment.current);

    const onFrame = (frame: AttachmentFrame) => {
      if (!isCurrent()) return;
      if (frame.type === "workspace.phase") {
        fatalPhaseReceived = frame.phase === "failed";
        setPhase(frame.phase);
        if (
          frame.phase === "connecting" ||
          frame.phase === "selecting" ||
          frame.phase === "failed"
        ) {
          resetProjection();
        } else {
          const pending = pendingProjection.current;
          if (pending === null || pending.completed.size !== pending.metadata.panes.length) {
            throw new Error("incomplete Browser projection");
          }
          currentWorkspaceEpoch.current = pending.metadata.workspace_epoch;
          pendingProjection.current = null;
          setProjection({ metadata: pending.metadata, snapshots: pending.snapshots });
        }
      } else if (frame.type === "session.list") {
        setTmuxVersion(
          frame.tmux_server_version === null
            ? `${frame.tmux_client_version} · no running server`
            : `${frame.tmux_client_version} · server ${frame.tmux_server_version}`,
        );
        setMachineConnectionEpoch(frame.machine_connection_epoch);
        setSelectionEpoch(frame.selection_epoch);
        setSessions(frame.sessions);
        setError("");
      } else if (frame.type === "writer.state") {
        setWriterRole(frame.role);
        setWriterAvailable(frame.writer_available);
        if (frame.role !== "writer") lastResize.current = "";
      } else if (frame.type === "operation.result") {
        if (frame.operation !== "pane.input" && frame.operation !== "client.resize") {
          setOperationStatus(`${frame.outcome}: ${frame.message}`);
        }
        if (frame.outcome === "ambiguous") {
          setError(
            "The target effect is unknown. OwlMux did not retry it; inspect fresh target state before acting again.",
          );
        } else if (frame.outcome === "failed") {
          setError(frame.message);
          if (frame.operation === "client.resize") lastResize.current = "";
        } else if (frame.operation !== "pane.input" && frame.operation !== "client.resize") {
          setError("");
        }
      } else if (frame.type === "workspace.projection") {
        setMachineConnectionEpoch(frame.machine_connection_epoch);
        pendingProjection.current = {
          metadata: frame,
          snapshots: new Map(frame.panes.map((pane) => [pane.pane_id, []])),
          completed: new Set(),
        };
      } else if (frame.type === "workspace.pane_snapshot") {
        const pending = pendingProjection.current;
        if (pending === null) throw new Error("snapshot without projection");
        pending.snapshots.get(frame.pane_id)?.push(frame.data);
        if (frame.final) pending.completed.add(frame.pane_id);
      } else if (frame.type === "workspace.output") {
        if (frame.workspace_epoch !== currentWorkspaceEpoch.current) return;
        const sink = paneSinks.current.get(frame.pane_id);
        if (sink !== undefined) {
          if (!sink.enqueue(frame.data)) failRenderer();
        } else {
          pendingOutputBytes.current += frame.data.length;
          if (pendingOutputBytes.current > MAX_RENDER_QUEUE_BYTES) {
            failRenderer();
            return;
          }
          const queued = pendingOutput.current.get(frame.pane_id) ?? [];
          queued.push(frame.data);
          pendingOutput.current.set(frame.pane_id, queued);
        }
      } else if (frame.type === "workspace.error") {
        const message = attachmentErrorMessage(frame.code, frame.message);
        if (fatalPhaseReceived) terminalDiagnostic.current = message;
        setError(message);
      }
    };

    const opened = client.openAttachment(
      machine.machine_id,
      onFrame,
      () => {
        if (!isCurrent()) return;
        attachment.current = null;
        resetProjection();
        setWriterRole("observer");
        setWriterAvailable(false);
        setPhase("failed");
        setError(attachmentCloseMessage(terminalDiagnostic.current));
      },
      () => {
        if (isCurrent()) onAuthenticationFailure();
      },
    );
    active = opened;
    attachment.current = opened;
    return () => {
      disposed = true;
      if (attachment.current === opened) {
        attachment.current = null;
        resetProjection();
      }
      opened.dispose();
    };
  }, [
    client,
    connectionAttempt,
    failRenderer,
    machine.machine_id,
    onAuthenticationFailure,
    resetProjection,
  ]);

  useEffect(() => {
    const surface = resizeSurface.current;
    if (surface === null) return;
    const observe = () => {
      const bounds = surface.getBoundingClientRect();
      setViewport({ height: bounds.height, width: bounds.width });
    };
    observe();
    const observer = new ResizeObserver(observe);
    observer.observe(surface);
    return () => observer.disconnect();
  }, [phase, projection, visible]);

  useEffect(() => {
    if (
      !visible ||
      phase !== "ready" ||
      projection === null ||
      writerRole !== "writer" ||
      cellSize === null
    ) {
      return;
    }
    let columns = Math.floor(viewport.width / cellSize.width);
    let rows = Math.floor(viewport.height / cellSize.height);
    columns = Math.min(ATTACHMENT_MAX_DIMENSION, columns);
    rows = Math.min(ATTACHMENT_MAX_DIMENSION, rows);
    if (columns < MIN_TERMINAL_COLUMNS || rows < MIN_TERMINAL_ROWS) return;
    if (columns * rows > ATTACHMENT_MAX_GRID_CELLS) {
      rows = Math.floor(ATTACHMENT_MAX_GRID_CELLS / columns);
    }
    if (rows < MIN_TERMINAL_ROWS) return;
    if (
      columns === projection.metadata.window.width &&
      rows === projection.metadata.window.height
    ) {
      lastResize.current = `${columns}x${rows}`;
      return;
    }
    const desired = `${columns}x${rows}`;
    if (desired === lastResize.current) return;
    const timer = window.setTimeout(() => {
      try {
        attachment.current?.resize(
          machineConnectionEpoch,
          projection.metadata.workspace_epoch,
          columns,
          rows,
        );
        lastResize.current = desired;
      } catch (reason) {
        setError(reason instanceof Error ? reason.message : "Automatic resize was not queued.");
        lastResize.current = "";
      }
    }, RESIZE_DEBOUNCE_MS);
    return () => window.clearTimeout(timer);
  }, [
    cellSize,
    machineConnectionEpoch,
    phase,
    projection,
    viewport.height,
    viewport.width,
    visible,
    writerRole,
  ]);

  function attachmentEpoch(): string {
    return projection?.metadata.workspace_epoch ?? selectionEpoch;
  }

  function preferredSize(): { columns: number; rows: number } {
    if (cellSize !== null && viewport.width > 0 && viewport.height > 0) {
      const columns = Math.max(
        MIN_TERMINAL_COLUMNS,
        Math.min(ATTACHMENT_MAX_DIMENSION, Math.floor(viewport.width / cellSize.width)),
      );
      const rows = Math.max(
        MIN_TERMINAL_ROWS,
        Math.min(
          ATTACHMENT_MAX_DIMENSION,
          Math.floor(viewport.height / cellSize.height),
          Math.floor(ATTACHMENT_MAX_GRID_CELLS / columns),
        ),
      );
      return { columns, rows };
    }
    return {
      columns: projection?.metadata.window.width ?? 120,
      rows: projection?.metadata.window.height ?? 36,
    };
  }

  function changeWriter(takeover: boolean) {
    const epoch = attachmentEpoch();
    if (!machineConnectionEpoch || !epoch) return;
    const size = preferredSize();
    setOperationStatus(takeover ? "Taking control…" : "Requesting control…");
    try {
      if (takeover) {
        attachment.current?.takeOverWriter(machineConnectionEpoch, epoch, size.columns, size.rows);
      } else {
        attachment.current?.claimWriter(machineConnectionEpoch, epoch, size.columns, size.rows);
      }
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : "Control request was not queued.");
    }
  }

  function createSession(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const form = event.currentTarget;
    const name = String(new FormData(form).get("session-name") ?? "");
    try {
      attachment.current?.createSession(machineConnectionEpoch, selectionEpoch, name);
      setPreviousSession(null);
      onTitleChange(name);
      form.reset();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : "Session creation was not queued.");
    }
  }

  const selectSession = useCallback(
    (session: AttachmentSessionSummary) => {
      setPreviousSession({
        name: session.name,
        sessionCreated: session.session_created,
        sessionId: session.session_id,
      });
      onTitleChange(session.name);
      attachment.current?.selectSession(
        machineConnectionEpoch,
        selectionEpoch,
        session.session_id,
        session.session_created,
      );
    },
    [machineConnectionEpoch, onTitleChange, selectionEpoch],
  );

  const sendInput = useCallback(
    (paneId: string, data: Uint8Array) => {
      if (projection === null) return;
      if (data.length > ATTACHMENT_MAX_INPUT_BYTES) {
        setError(
          `Paste is larger than the ${ATTACHMENT_MAX_INPUT_BYTES}-byte input bound. Paste a smaller chunk explicitly.`,
        );
        return;
      }
      if (data.length > 256 && !window.confirm(`Send ${data.length} bytes to target tmux?`)) return;
      try {
        attachment.current?.sendPaneInput(
          machineConnectionEpoch,
          projection.metadata.workspace_epoch,
          paneId,
          data,
        );
      } catch (reason) {
        setError(reason instanceof Error ? reason.message : "Pane input was rejected.");
      }
    },
    [machineConnectionEpoch, projection],
  );

  const selectPane = useCallback(
    (paneId: string) => {
      if (projection === null) return;
      try {
        attachment.current?.selectPane(
          machineConnectionEpoch,
          projection.metadata.workspace_epoch,
          paneId,
        );
      } catch (reason) {
        setError(reason instanceof Error ? reason.message : "Pane selection was not queued.");
      }
    },
    [machineConnectionEpoch, projection],
  );

  function returnToChooser() {
    if (projection === null) return;
    try {
      attachment.current?.returnToChooser(
        machineConnectionEpoch,
        projection.metadata.workspace_epoch,
      );
      onTitleChange(null);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : "Could not return to the chooser.");
    }
  }

  function detach() {
    const active = attachment.current;
    attachment.current = null;
    try {
      active?.detach();
    } finally {
      onClose();
    }
  }

  function reconnect() {
    const active = attachment.current;
    attachment.current = null;
    active?.dispose();
    terminalDiagnostic.current = null;
    resetProjection();
    setPhase("connecting");
    setTmuxVersion("");
    setMachineConnectionEpoch("");
    setSelectionEpoch("");
    setSessions([]);
    setWriterRole("observer");
    setWriterAvailable(false);
    setOperationStatus("");
    setError("");
    lastResize.current = "";
    setConnectionAttempt((attempt) => attempt + 1);
  }

  const controlLabel =
    writerRole === "writer"
      ? "You have control"
      : writerAvailable
        ? "View only · control available"
        : "View only · controlled elsewhere";

  return (
    <section className="workspace-view" aria-label={`${machine.alias} workspace`}>
      <header className="workspace-command-bar">
        <div className="workspace-identity">
          <strong>{machine.alias}</strong>
          {previousSession !== null && <span>/ {previousSession.name}</span>}
          <span className={`connection-indicator is-${phase}`}>{phaseLabel(phase)}</span>
        </div>
        <div className="workspace-actions">
          <span className={`control-state is-${writerRole}`}>{controlLabel}</span>
          {writerRole === "observer" && phase !== "connecting" && phase !== "failed" && (
            <button
              className="button button-primary button-compact"
              onClick={() => changeWriter(!writerAvailable)}
              type="button"
            >
              {writerAvailable ? "Take control" : "Take over"}
            </button>
          )}
          {phase === "ready" && projection !== null && (
            <button
              className="button button-ghost button-compact"
              onClick={returnToChooser}
              type="button"
            >
              Sessions
            </button>
          )}
          <button className="button button-ghost button-compact" onClick={detach} type="button">
            Detach
          </button>
        </div>
      </header>

      {error && (
        <div className="workspace-alert" role="alert">
          <span>{error}</span>
          {phase === "failed" && (
            <button
              className="button button-secondary button-compact"
              onClick={reconnect}
              type="button"
            >
              Reconnect
            </button>
          )}
        </div>
      )}

      {phase === "connecting" && (
        <div className="workspace-centered-state" aria-live="polite">
          <span className="activity-dot" />
          <h2>Connecting to {machine.alias}</h2>
          <p>Resolving the current Machine owner and discovering target tmux.</p>
        </div>
      )}

      {phase === "failed" && !error && (
        <div className="workspace-centered-state">
          <h2>Connection ended</h2>
          <p>Target tmux and its processes continue on the Host.</p>
          <button className="button button-primary" onClick={reconnect} type="button">
            Reconnect
          </button>
        </div>
      )}

      {phase === "selecting" && (
        <SessionChooser
          machine={machine}
          onCreate={createSession}
          onRefresh={() =>
            attachment.current?.refreshSessions(machineConnectionEpoch, selectionEpoch)
          }
          onSelect={selectSession}
          previousSession={previousSession}
          sessions={sessions}
          tmuxVersion={tmuxVersion}
          writerRole={writerRole}
        />
      )}

      {phase === "ready" && projection !== null && (
        <section className="terminal-workspace">
          <nav className="window-tabs" aria-label="Target tmux windows">
            {projection.metadata.windows.map((windowSummary) => (
              <button
                aria-current={windowSummary.active ? "page" : undefined}
                className={windowSummary.active ? "window-tab is-active" : "window-tab"}
                disabled={writerRole !== "writer" || windowSummary.active}
                key={windowSummary.window_id}
                onClick={() =>
                  attachment.current?.selectWindow(
                    machineConnectionEpoch,
                    projection.metadata.workspace_epoch,
                    windowSummary.window_id,
                  )
                }
                type="button"
              >
                {windowSummary.name}
              </button>
            ))}
            <span className="window-tabs-spacer" />
            <button
              className="window-tab"
              disabled={writerRole !== "writer"}
              onClick={() =>
                attachment.current?.refresh(
                  machineConnectionEpoch,
                  projection.metadata.workspace_epoch,
                )
              }
              type="button"
            >
              Refresh
            </button>
          </nav>
          <div
            aria-label="Target-authoritative tmux pane layout"
            className="pane-layout"
            ref={resizeSurface}
            title={projection.metadata.window.layout}
          >
            {projection.metadata.panes.map((pane) => (
              <PaneTerminal
                chunks={projection.snapshots.get(pane.pane_id) ?? []}
                key={`${projection.metadata.workspace_epoch}:${pane.pane_id}`}
                onCellSize={reportCellSize}
                onInput={sendInput}
                onSelect={selectPane}
                onSink={registerSink}
                pane={pane}
                selectable={writerRole === "writer"}
                visible={visible}
                window={projection.metadata.window}
                writable={writerRole === "writer" && pane.active}
              />
            ))}
          </div>
          <footer className="workspace-status-bar">
            <span>
              {machine.alias} · {previousSession?.name ?? projection.metadata.session_id} ·{" "}
              {projection.metadata.window.name}
            </span>
            <span>
              {projection.metadata.window.width}×{projection.metadata.window.height} ·{" "}
              {projection.metadata.panes.length} pane
              {projection.metadata.panes.length === 1 ? "" : "s"}
            </span>
            <span>{writerRole === "writer" ? "Writable" : "View only"}</span>
          </footer>
        </section>
      )}

      {operationStatus && phase !== "ready" && (
        <p aria-live="polite" className="operation-status">
          {operationStatus}
        </p>
      )}
    </section>
  );
}

function SessionChooser({
  machine,
  onCreate,
  onRefresh,
  onSelect,
  previousSession,
  sessions,
  tmuxVersion,
  writerRole,
}: {
  machine: MachineSummary;
  onCreate: (event: FormEvent<HTMLFormElement>) => void;
  onRefresh: () => void;
  onSelect: (session: AttachmentSessionSummary) => void;
  previousSession: PreviousSession | null;
  sessions: Array<AttachmentSessionSummary>;
  tmuxVersion: string;
  writerRole: "observer" | "writer";
}) {
  return (
    <section className="session-chooser">
      <header className="session-chooser-header">
        <div>
          <p className="section-kicker">{machine.alias}</p>
          <h2>Choose a tmux session</h2>
          <p>Selection stays explicit after every new or replacement OwlMux connection.</p>
        </div>
        <button className="button button-secondary" onClick={onRefresh} type="button">
          Refresh
        </button>
      </header>

      <div className="session-list">
        {sessions.length === 0 && (
          <div className="empty-state compact">
            <h3>No running sessions</h3>
            <p>Create a target tmux session after taking control.</p>
          </div>
        )}
        {sessions.map((session) => {
          const previouslyViewed =
            previousSession?.sessionId === session.session_id &&
            previousSession.sessionCreated === session.session_created;
          return (
            <article
              className="session-row"
              key={`${session.session_id}:${session.session_created}`}
            >
              <div>
                <strong>{session.name}</strong>
                <span>
                  {session.window_count} window{session.window_count === 1 ? "" : "s"} ·{" "}
                  {session.attached_client_count} attached client
                  {session.attached_client_count === 1 ? "" : "s"}
                </span>
              </div>
              {previouslyViewed && (
                <span className="status-pill is-neutral">Previously viewed</span>
              )}
              <button
                className="button button-primary"
                onClick={() => onSelect(session)}
                type="button"
              >
                Open
              </button>
            </article>
          );
        })}
      </div>

      <form className="new-session-form" onSubmit={onCreate}>
        <div>
          <label htmlFor={`session-name-${machine.machine_id}`}>New target tmux session</label>
          <span>Creates one session through the closed OwlMux operation.</span>
        </div>
        <input
          disabled={writerRole !== "writer"}
          id={`session-name-${machine.machine_id}`}
          maxLength={64}
          name="session-name"
          placeholder="session-name"
          required
        />
        <button className="button button-primary" disabled={writerRole !== "writer"} type="submit">
          Create and open
        </button>
      </form>

      <footer className="chooser-diagnostics">
        <span>{tmuxVersion || "Discovering target tmux version…"}</span>
        <span>
          {writerRole === "writer" ? "You have control" : "View only until control is claimed"}
        </span>
      </footer>
    </section>
  );
}

function PaneTerminal({
  chunks,
  onCellSize,
  onInput,
  onSelect,
  onSink,
  pane,
  selectable,
  visible,
  window: targetWindow,
  writable,
}: {
  chunks: Array<Uint8Array>;
  onCellSize: (size: CellSize) => void;
  onInput: (paneId: string, data: Uint8Array) => void;
  onSelect: (paneId: string) => void;
  onSink: (paneId: string, sink: PaneSink | null) => void;
  pane: AttachmentPane;
  selectable: boolean;
  visible: boolean;
  window: AttachmentWindow;
  writable: boolean;
}) {
  const element = useRef<HTMLDivElement | null>(null);
  const instanceRef = useRef<Terminal | null>(null);
  const writableRef = useRef(writable);

  useEffect(() => {
    writableRef.current = writable;
    if (instanceRef.current !== null) {
      instanceRef.current.options.disableStdin = !writable;
      if (writable) instanceRef.current.focus();
    }
  }, [writable]);

  useEffect(() => {
    if (element.current === null) return;
    const instance = new Terminal({
      allowProposedApi: false,
      cols: pane.width,
      convertEol: false,
      cursorBlink: true,
      disableStdin: !writableRef.current,
      fontFamily: '"SFMono-Regular", Consolas, "Liberation Mono", monospace',
      fontSize: 13,
      lineHeight: 1,
      rows: pane.height,
      scrollback: 0,
      theme: {
        background: "#080d14",
        cursor: "#d4f4e7",
        foreground: "#d8e2eb",
        selectionBackground: "#345b5066",
      },
      windowOptions: {},
    });
    instance.open(element.current);
    instanceRef.current = instance;
    let disposed = false;
    let writing = false;
    let queuedBytes = 0;
    const queue: Array<Uint8Array> = [];
    const drain = () => {
      if (disposed || writing) return;
      const data = queue.shift();
      if (data === undefined) return;
      writing = true;
      instance.write(data, () => {
        writing = false;
        queuedBytes -= data.length;
        drain();
      });
    };
    const sink: PaneSink = {
      enqueue(data) {
        if (disposed || queuedBytes + data.length > MAX_RENDER_QUEUE_BYTES) return false;
        const copy = data.slice();
        queue.push(copy);
        queuedBytes += copy.length;
        drain();
        return true;
      },
    };
    for (const chunk of chunks) {
      if (!sink.enqueue(chunk)) break;
    }
    const input = instance.onData((data) => {
      if (writableRef.current) onInput(pane.pane_id, new TextEncoder().encode(data));
    });
    onSink(pane.pane_id, sink);
    if (writableRef.current) instance.focus();
    return () => {
      disposed = true;
      input.dispose();
      onSink(pane.pane_id, null);
      if (instanceRef.current === instance) instanceRef.current = null;
      instance.dispose();
    };
  }, [chunks, onInput, onSink, pane]);

  useEffect(() => {
    if (!visible) return;
    const frame = window.requestAnimationFrame(() => {
      const screen = element.current?.querySelector<HTMLElement>(".xterm-screen");
      if (screen === null || screen === undefined) return;
      const bounds = screen.getBoundingClientRect();
      if (bounds.width > 0 && bounds.height > 0) {
        onCellSize({ height: bounds.height / pane.height, width: bounds.width / pane.width });
      }
    });
    return () => window.cancelAnimationFrame(frame);
  }, [onCellSize, pane.height, pane.width, visible]);

  function select(event: ReactPointerEvent<HTMLElement>) {
    if (!selectable || pane.active) return;
    event.preventDefault();
    onSelect(pane.pane_id);
  }

  return (
    <article
      aria-label={`${pane.active ? "Active" : "Inactive"} tmux pane ${pane.pane_id}`}
      className={pane.active ? "tmux-pane is-active" : "tmux-pane"}
      onPointerDown={select}
      style={{
        height: `${(pane.height / targetWindow.height) * 100}%`,
        left: `${(pane.left / targetWindow.width) * 100}%`,
        top: `${(pane.top / targetWindow.height) * 100}%`,
        width: `${(pane.width / targetWindow.width) * 100}%`,
      }}
    >
      <div
        aria-label={`${writable ? "Writable" : "Observer"} terminal ${pane.pane_id}`}
        className="tmux-pane-terminal"
        ref={element}
      />
    </article>
  );
}

export function isCurrentAttachmentAttempt<T>(
  disposed: boolean,
  attempt: T | null,
  current: T | null,
): boolean {
  return !disposed && attempt !== null && current === attempt;
}

export function attachmentCloseMessage(diagnostic: string | null): string {
  return (
    diagnostic ?? "The OwlMux connection ended. Target tmux and its processes continue on the Host."
  );
}

export function attachmentErrorMessage(code: WorkspaceErrorCode, message: string): string {
  if (code === "owner_unreachable") {
    return "The valid Host owner is unreachable. Fence or stop that Server node, wait for lease expiry, then reconnect.";
  }
  return message;
}

function phaseLabel(phase: WorkspacePhase): string {
  switch (phase) {
    case "connecting":
      return "Connecting";
    case "selecting":
      return "Choose a session";
    case "ready":
      return "Connected";
    case "failed":
      return "Disconnected";
  }
}
