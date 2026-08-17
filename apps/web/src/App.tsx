import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { useCallback, useEffect, useRef, useState, type FormEvent } from "react";

import {
  AuthenticationError,
  createApiClient,
  type ApiClient,
  type AttachmentFrame,
  type AttachmentPane,
  type AttachmentSession,
  type AttachmentSessionSummary,
  type AttachmentWindow,
} from "./client";
import { PUBLIC_CONTRACT_VERSION } from "./generated/contracts";
import type {
  CredentialSummary,
  DeploymentPresentation,
  MachineSummary,
} from "./generated/contracts";

export function App() {
  const [client, setClient] = useState<ApiClient | null>(null);
  const [candidate, setCandidate] = useState("");
  const [loginError, setLoginError] = useState("");

  const logout = useCallback(() => {
    client?.dispose();
    setClient(null);
    setCandidate("");
    setLoginError("");
  }, [client]);

  useEffect(() => {
    const dispose = () => client?.dispose();
    const rejectRestoredPage = (event: PageTransitionEvent) => {
      if (event.persisted) logout();
    };
    window.addEventListener("pagehide", dispose);
    window.addEventListener("pageshow", rejectRestoredPage);
    return () => {
      window.removeEventListener("pagehide", dispose);
      window.removeEventListener("pageshow", rejectRestoredPage);
    };
  }, [client, logout]);

  async function login(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const next = createApiClient(candidate);
    setCandidate("");
    try {
      await next.deployment();
      setLoginError("");
      setClient(next);
    } catch {
      next.dispose();
      setLoginError("Authentication failed. Re-enter the current Deployment API key.");
    }
  }

  if (client === null) {
    return (
      <main className="login-shell">
        <section className="login-card">
          <p className="eyebrow">OwlMux · {PUBLIC_CONTRACT_VERSION}</p>
          <h1>Your tmux sessions stay where they belong.</h1>
          <p>
            Enter the single Deployment API key. It stays only in this page's memory and is cleared
            on reload, navigation, authentication failure, or logout.
          </p>
          <form onSubmit={login}>
            <label htmlFor="api-key">Deployment API key</label>
            <input
              autoComplete="off"
              id="api-key"
              name="api-key"
              onChange={(event) => setCandidate(event.currentTarget.value)}
              required
              spellCheck={false}
              type="password"
              value={candidate}
            />
            <button type="submit">Open deployment</button>
          </form>
          {loginError && <p className="error-banner">{loginError}</p>}
        </section>
      </main>
    );
  }

  return <ControlPlane client={client} logout={logout} />;
}

function ControlPlane({ client, logout }: { client: ApiClient; logout: () => void }) {
  const [deployment, setDeployment] = useState<DeploymentPresentation | null>(null);
  const [credentials, setCredentials] = useState<Array<CredentialSummary>>([]);
  const [machines, setMachines] = useState<Array<MachineSummary>>([]);
  const [issuedToken, setIssuedToken] = useState<string | null>(null);
  const [workspaceMachine, setWorkspaceMachine] = useState<MachineSummary | null>(null);
  const [error, setError] = useState("");

  const refresh = useCallback(async () => {
    try {
      const [nextDeployment, nextCredentials, nextMachines] = await Promise.all([
        client.deployment(),
        client.listCredentials(),
        client.listMachines(),
      ]);
      setDeployment(nextDeployment);
      setCredentials(nextCredentials);
      setMachines(nextMachines);
      setError("");
    } catch (reason) {
      if (reason instanceof AuthenticationError) logout();
      else setError(reason instanceof Error ? reason.message : "Control-plane refresh failed.");
    }
  }, [client, logout]);

  useEffect(() => {
    const timer = window.setTimeout(() => void refresh(), 0);
    return () => window.clearTimeout(timer);
  }, [refresh]);

  async function createCredential(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const form = event.currentTarget;
    const name = String(new FormData(form).get("name") ?? "");
    try {
      await client.createCredential({ name });
      form.reset();
      await refresh();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : "Credential creation failed.");
    }
  }

  async function changeCredential(
    credential: CredentialSummary | null,
    action: "rename" | "default" | "reset" | "retire",
  ) {
    try {
      if (action === "rename" && credential !== null) {
        const name = window.prompt("New credential name", credential.name);
        if (name === null) return;
        await client.renameCredential(credential.ssh_credential_id, name);
      } else if (action === "default" && credential !== null) {
        await client.setDefaultCredential(credential.ssh_credential_id);
      } else if (action === "reset") {
        const name = window.prompt(
          "Name for the new generated default credential",
          "Reset default",
        );
        if (name === null) return;
        await client.resetDefaultCredential(name);
      } else if (action === "retire" && credential !== null) {
        if (!window.confirm(`Retire ${credential.name}?`)) return;
        await client.retireCredential(credential.ssh_credential_id);
      }
      await refresh();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : "Credential update failed.");
    }
  }

  async function createMachine(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const form = event.currentTarget;
    const data = new FormData(form);
    try {
      const created = await client.createMachine({
        alias: String(data.get("alias") ?? ""),
        host_identity: String(data.get("host_identity") ?? ""),
        target_account: String(data.get("target_account") ?? ""),
        tmux_path: String(data.get("tmux_path") ?? "/usr/bin/tmux"),
        tmux_socket_identity: String(data.get("tmux_socket_identity") ?? "default"),
      });
      setIssuedToken(created.enrollment_token);
      form.reset();
      await refresh();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : "Machine creation failed.");
    }
  }

  async function reissue(machineId: string) {
    try {
      const issued = await client.issueEnrollment(machineId);
      setIssuedToken(issued.enrollment_token);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : "Token issuance failed.");
    }
  }

  async function changeMachine(
    machine: MachineSummary,
    action: "disable" | "enable" | "re-enroll",
  ) {
    if (
      action !== "enable" &&
      !window.confirm(
        `${action === "disable" ? "Disable" : "Re-enroll"} ${machine.alias}? Current OwlMux attachments will close; target tmux will not be stopped.`,
      )
    ) {
      return;
    }
    try {
      if (action === "disable") await client.disableMachine(machine.machine_id);
      else if (action === "enable") await client.enableMachine(machine.machine_id);
      else await client.reEnrollMachine(machine.machine_id);
      await refresh();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : "Machine lifecycle update failed.");
    }
  }

  if (workspaceMachine !== null) {
    return (
      <ReadOnlyWorkspace
        client={client}
        machine={workspaceMachine}
        onClose={() => setWorkspaceMachine(null)}
      />
    );
  }

  return (
    <main className="control-shell">
      <header className="control-header">
        <div>
          <p className="eyebrow">Single-node control plane</p>
          <h1>OwlMux</h1>
          <p className="muted">
            {deployment ? `Deployment ${deployment.deployment_id}` : "Loading deployment…"}
          </p>
        </div>
        <button className="secondary-action" onClick={logout} type="button">
          Log out and clear key
        </button>
      </header>

      {error && <p className="error-banner">{error}</p>}
      {issuedToken && (
        <section className="token-card" aria-live="polite">
          <div>
            <h2>One-use enrollment token</h2>
            <p>Copy it now. It is not available from later reads.</p>
          </div>
          <code>{issuedToken}</code>
          <button onClick={() => setIssuedToken(null)} type="button">
            Clear token
          </button>
        </section>
      )}

      <div className="control-grid">
        <section className="panel">
          <h2>SSH credentials</h2>
          <p className="muted">Target administrators install these public keys externally.</p>
          <ul className="resource-list">
            {credentials.map((credential) => (
              <li key={credential.ssh_credential_id}>
                <strong>{credential.name}</strong>
                <span>{credential.public_fingerprint_sha256}</span>
                <span>
                  {credential.is_default ? "Default · " : ""}
                  {credential.bound_machine_count} bound Machines
                </span>
                <div className="resource-actions">
                  <button
                    onClick={() => void navigator.clipboard.writeText(credential.public_key)}
                    type="button"
                  >
                    Copy public key
                  </button>
                  <button
                    className="secondary-action"
                    onClick={() => void changeCredential(credential, "rename")}
                    type="button"
                  >
                    Rename
                  </button>
                  {!credential.is_default && credential.status === "active" && (
                    <button
                      className="secondary-action"
                      onClick={() => void changeCredential(credential, "default")}
                      type="button"
                    >
                      Make default
                    </button>
                  )}
                  {!credential.is_default &&
                    credential.status === "active" &&
                    credential.bound_machine_count === 0 && (
                      <button
                        className="secondary-action"
                        onClick={() => void changeCredential(credential, "retire")}
                        type="button"
                      >
                        Retire
                      </button>
                    )}
                </div>
              </li>
            ))}
          </ul>
          <form className="compact-form" onSubmit={createCredential}>
            <label htmlFor="credential-name">New generated credential name</label>
            <input id="credential-name" maxLength={64} name="name" required />
            <button type="submit">Generate credential</button>
            <button
              className="secondary-action"
              onClick={() => void changeCredential(null, "reset")}
              type="button"
            >
              Generate and reset default
            </button>
          </form>
        </section>

        <section className="panel">
          <h2>Machines</h2>
          <ul className="resource-list">
            {machines.map((machine) => (
              <li key={machine.machine_id}>
                <strong>{machine.alias}</strong>
                <span>
                  {machine.lifecycle} · {machine.reachability}
                </span>
                <span>
                  {machine.target_account} · {machine.tmux_socket_identity}
                </span>
                {machine.lifecycle === "pending" && (
                  <button onClick={() => void reissue(machine.machine_id)} type="button">
                    Issue replacement token
                  </button>
                )}
                {machine.lifecycle === "active" && (
                  <div className="resource-actions">
                    <button onClick={() => setWorkspaceMachine(machine)} type="button">
                      Open read-only workspace
                    </button>
                    <button
                      className="secondary-action"
                      onClick={() => void changeMachine(machine, "re-enroll")}
                      type="button"
                    >
                      Re-enroll Relay
                    </button>
                    <button
                      className="secondary-action"
                      onClick={() => void changeMachine(machine, "disable")}
                      type="button"
                    >
                      Disable
                    </button>
                  </div>
                )}
                {machine.lifecycle === "disabled" && (
                  <button onClick={() => void changeMachine(machine, "enable")} type="button">
                    Enable as pending
                  </button>
                )}
              </li>
            ))}
          </ul>
          <form className="compact-form" onSubmit={createMachine}>
            <label>
              Alias
              <input maxLength={64} name="alias" required />
            </label>
            <label>
              Target account
              <input maxLength={64} name="target_account" required />
            </label>
            <label>
              tmux path
              <input defaultValue="/usr/bin/tmux" name="tmux_path" required />
            </label>
            <label>
              tmux socket identity
              <input defaultValue="default" name="tmux_socket_identity" required />
            </label>
            <label>
              Expected SSH host public key
              <textarea name="host_identity" required rows={3} />
            </label>
            <button type="submit">Create pending Machine</button>
          </form>
        </section>
      </div>
    </main>
  );
}

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

const MAX_RENDER_QUEUE_BYTES = 1024 * 1024;

function ReadOnlyWorkspace({
  client,
  machine,
  onClose,
}: {
  client: ApiClient;
  machine: MachineSummary;
  onClose: () => void;
}) {
  const attachment = useRef<AttachmentSession | null>(null);
  const pendingProjection = useRef<PendingProjection | null>(null);
  const currentWorkspaceEpoch = useRef<string | null>(null);
  const paneSinks = useRef(new Map<string, PaneSink>());
  const pendingOutput = useRef(new Map<string, Array<Uint8Array>>());
  const pendingOutputBytes = useRef(0);
  const [phase, setPhase] = useState("connecting");
  const [tmuxVersion, setTmuxVersion] = useState("");
  const [selectionEpoch, setSelectionEpoch] = useState("");
  const [sessions, setSessions] = useState<Array<AttachmentSessionSummary>>([]);
  const [projection, setProjection] = useState<InstalledProjection | null>(null);
  const [error, setError] = useState("");

  const failRenderer = useCallback(() => {
    setError("The Browser renderer could not keep up with bounded terminal output.");
    setPhase("failed");
    setProjection(null);
    currentWorkspaceEpoch.current = null;
    attachment.current?.dispose();
  }, []);

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

  useEffect(() => {
    const resetProjection = () => {
      pendingProjection.current = null;
      currentWorkspaceEpoch.current = null;
      paneSinks.current.clear();
      pendingOutput.current.clear();
      pendingOutputBytes.current = 0;
      setProjection(null);
    };
    const onFrame = (frame: AttachmentFrame) => {
      if (frame.type === "workspace.phase") {
        setPhase(frame.phase);
        if (
          frame.phase === "connecting" ||
          frame.phase === "selecting" ||
          frame.phase === "failed"
        ) {
          resetProjection();
        } else {
          const pending = pendingProjection.current;
          if (pending === null || pending.completed.size !== pending.metadata.panes.length)
            throw new Error("incomplete Browser projection");
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
        setSelectionEpoch(frame.selection_epoch);
        setSessions(frame.sessions);
        setError("");
      } else if (frame.type === "workspace.projection") {
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
        setError(frame.message);
      }
    };
    const active = client.openAttachment(machine.machine_id, onFrame, () => {
      resetProjection();
      setPhase("failed");
    });
    attachment.current = active;
    return () => {
      active.dispose();
      attachment.current = null;
      resetProjection();
    };
  }, [client, failRenderer, machine.machine_id]);

  function detach() {
    try {
      attachment.current?.detach();
    } finally {
      onClose();
    }
  }

  return (
    <main className="workspace-shell">
      <header className="workspace-header">
        <div>
          <p className="eyebrow">Read-only target-owned tmux</p>
          <h1>{machine.alias}</h1>
          <p className="muted">
            {phase} {tmuxVersion && `· ${tmuxVersion}`}
          </p>
        </div>
        <div className="workspace-actions">
          {phase === "ready" && (
            <button
              className="secondary-action"
              onClick={() => attachment.current?.returnToChooser()}
              type="button"
            >
              Return to chooser
            </button>
          )}
          <button onClick={detach} type="button">
            Detach
          </button>
        </div>
      </header>

      {error && <p className="error-banner">{error}</p>}
      {phase === "selecting" && (
        <section className="session-chooser">
          <h2>Choose a current target session</h2>
          <p className="muted">Selection is always explicit, even when only one session exists.</p>
          {sessions.length === 0 && <p>No target tmux sessions are currently running.</p>}
          <ul className="resource-list">
            {sessions.map((session) => (
              <li key={`${session.session_id}:${session.session_created}`}>
                <strong>{session.name}</strong>
                <span>
                  {session.session_id} · created {session.session_created} · {session.window_count}{" "}
                  windows · {session.attached_client_count} attached clients
                </span>
                <button
                  onClick={() =>
                    attachment.current?.selectSession(
                      selectionEpoch,
                      session.session_id,
                      session.session_created,
                    )
                  }
                  type="button"
                >
                  Open read-only
                </button>
              </li>
            ))}
          </ul>
        </section>
      )}
      {phase === "ready" && projection !== null && (
        <section className="terminal-panel">
          <header className="target-window-header">
            <strong>{projection.metadata.window.name}</strong>
            <span>
              {projection.metadata.window.window_id} · {projection.metadata.window.width}×
              {projection.metadata.window.height} · {projection.metadata.panes.length} visible panes
            </span>
          </header>
          <div
            aria-label="Target-authoritative tmux pane layout"
            className="pane-layout"
            style={{
              aspectRatio: `${projection.metadata.window.width} / ${projection.metadata.window.height}`,
            }}
            title={projection.metadata.window.layout}
          >
            {projection.metadata.panes.map((pane) => (
              <PaneTerminal
                chunks={projection.snapshots.get(pane.pane_id) ?? []}
                key={`${projection.metadata.workspace_epoch}:${pane.pane_id}`}
                onSink={registerSink}
                pane={pane}
                window={projection.metadata.window}
              />
            ))}
          </div>
        </section>
      )}
    </main>
  );
}

function PaneTerminal({
  chunks,
  onSink,
  pane,
  window,
}: {
  chunks: Array<Uint8Array>;
  onSink: (paneId: string, sink: PaneSink | null) => void;
  pane: AttachmentPane;
  window: AttachmentWindow;
}) {
  const element = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (element.current === null) return;
    const instance = new Terminal({
      allowProposedApi: false,
      cols: pane.width,
      convertEol: false,
      cursorBlink: false,
      disableStdin: true,
      fontFamily: '"SFMono-Regular", Consolas, "Liberation Mono", monospace',
      fontSize: 12,
      rows: pane.height,
      scrollback: 0,
      theme: { background: "#111827", foreground: "#e5e7eb" },
      windowOptions: {},
    });
    instance.open(element.current);
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
    onSink(pane.pane_id, sink);
    return () => {
      disposed = true;
      onSink(pane.pane_id, null);
      instance.dispose();
    };
  }, [chunks, onSink, pane]);

  return (
    <article
      className={pane.active ? "tmux-pane is-active" : "tmux-pane"}
      style={{
        height: `${(pane.height / window.height) * 100}%`,
        left: `${(pane.left / window.width) * 100}%`,
        top: `${(pane.top / window.height) * 100}%`,
        width: `${(pane.width / window.width) * 100}%`,
      }}
    >
      <header className="tmux-pane-header">
        <span>{pane.pane_id}</span>
        <span>{pane.title || pane.current_command || "terminal"}</span>
      </header>
      <div
        aria-label={`Read-only terminal ${pane.pane_id}`}
        className="tmux-pane-terminal"
        ref={element}
      />
    </article>
  );
}
