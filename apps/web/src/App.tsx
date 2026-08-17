import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { useCallback, useEffect, useRef, useState, type FormEvent } from "react";

import {
  ApiError,
  AuthenticationError,
  createApiClient,
  type ApiClient,
  type AttachmentFrame,
  type AttachmentPane,
  type AttachmentSession,
  type AttachmentSessionSummary,
  type AttachmentWindow,
} from "./client";
import {
  ATTACHMENT_MAX_DIMENSION,
  ATTACHMENT_MAX_INPUT_BYTES,
  PUBLIC_CONTRACT_VERSION,
} from "./generated/contracts";
import type {
  AuditEventSummary,
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
    window.history.replaceState(null, "", "/login");
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
      window.history.replaceState(null, "", "/machines");
    } catch (reason) {
      next.dispose();
      setLoginError(
        reason instanceof AuthenticationError
          ? "Authentication failed. Re-enter the current Deployment API key."
          : "The Deployment could not be reached. Verify its health and try again.",
      );
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
  const [auditEvents, setAuditEvents] = useState<Array<AuditEventSummary>>([]);
  const [credentials, setCredentials] = useState<Array<CredentialSummary>>([]);
  const [machines, setMachines] = useState<Array<MachineSummary>>([]);
  const [issuedToken, setIssuedToken] = useState<string | null>(null);
  const [workspaceMachine, setWorkspaceMachine] = useState<MachineSummary | null>(null);
  const [error, setError] = useState("");
  const [outcomeUnknown, setOutcomeUnknown] = useState(false);
  const [mutationPending, setMutationPending] = useState(false);
  const outcomeUnknownRef = useRef(false);
  const mutationPendingRef = useRef(false);
  const refreshGenerationRef = useRef(0);
  const [loading, setLoading] = useState(true);
  const [route, setRoute] = useState(window.location.pathname);

  const navigate = useCallback((path: string) => {
    window.history.pushState(null, "", path);
    setRoute(path);
  }, []);

  useEffect(() => {
    const onPopState = () => setRoute(window.location.pathname);
    window.addEventListener("popstate", onPopState);
    return () => window.removeEventListener("popstate", onPopState);
  }, []);

  const presentError = useCallback(
    (reason: unknown, fallback: string) => {
      if (reason instanceof AuthenticationError) {
        logout();
      } else if (reason instanceof ApiError && reason.outcomeUnknown) {
        refreshGenerationRef.current += 1;
        outcomeUnknownRef.current = true;
        setOutcomeUnknown(true);
        setError(
          "The durable outcome is unknown. Mutations are disabled until you refresh durable state.",
        );
      } else if (reason instanceof ApiError && reason.code === "owner_unreachable") {
        setError(
          "The valid Machine owner is unreachable. Fence or stop that node, wait for lease expiry, then retry.",
        );
      } else {
        setError(reason instanceof Error ? reason.message : fallback);
      }
    },
    [logout],
  );

  const refresh = useCallback(
    async (acknowledgeUnknown = false) => {
      const generation = ++refreshGenerationRef.current;
      try {
        const [nextDeployment, nextAuditEvents, nextCredentials, nextMachines] = await Promise.all([
          client.deployment(),
          client.auditEvents(),
          client.listCredentials(),
          client.listMachines(),
        ]);
        if (generation !== refreshGenerationRef.current) return;
        setDeployment(nextDeployment);
        setAuditEvents(nextAuditEvents);
        setCredentials(nextCredentials);
        setMachines(nextMachines);
        if (acknowledgeUnknown) {
          outcomeUnknownRef.current = false;
          setOutcomeUnknown(false);
          setError("");
        } else if (!outcomeUnknownRef.current) {
          setError("");
        }
      } catch (reason) {
        if (generation === refreshGenerationRef.current) {
          presentError(reason, "Control-plane refresh failed.");
        }
      } finally {
        if (generation === refreshGenerationRef.current) setLoading(false);
      }
    },
    [client, presentError],
  );

  const runMutation = useCallback(
    async (operation: () => Promise<void>, fallback: string) => {
      if (mutationPendingRef.current || outcomeUnknownRef.current) return;
      mutationPendingRef.current = true;
      setMutationPending(true);
      try {
        await operation();
      } catch (reason) {
        presentError(reason, fallback);
      } finally {
        mutationPendingRef.current = false;
        setMutationPending(false);
      }
    },
    [presentError],
  );

  useEffect(() => {
    const timer = window.setTimeout(() => void refresh(), 0);
    return () => window.clearTimeout(timer);
  }, [refresh]);

  async function createCredential(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const form = event.currentTarget;
    const name = String(new FormData(form).get("name") ?? "");
    await runMutation(async () => {
      await client.createCredential({ name });
      form.reset();
      await refresh();
    }, "Credential creation failed.");
  }

  async function changeCredential(
    credential: CredentialSummary | null,
    action: "rename" | "default" | "reset" | "retire",
  ) {
    await runMutation(async () => {
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
    }, "Credential update failed.");
  }

  async function createMachine(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const form = event.currentTarget;
    const data = new FormData(form);
    await runMutation(async () => {
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
    }, "Machine creation failed.");
  }

  async function reissue(machineId: string) {
    await runMutation(async () => {
      const issued = await client.issueEnrollment(machineId);
      setIssuedToken(issued.enrollment_token);
    }, "Token issuance failed.");
  }

  async function changeMachine(
    machine: MachineSummary,
    action: "rename" | "disable" | "revoke" | "re-enroll",
  ) {
    await runMutation(async () => {
      if (action === "rename") {
        const alias = window.prompt("New Machine alias", machine.alias);
        if (alias === null) return;
        await client.renameMachine(machine.machine_id, alias);
      } else {
        const verb = action === "revoke" ? "Revoke Relay access for" : `${action} `;
        if (
          !window.confirm(
            `${verb}${machine.alias}? OwlMux access will close; target tmux and its processes will not be stopped.`,
          )
        ) {
          return;
        }
        if (action === "disable") await client.disableMachine(machine.machine_id);
        else if (action === "revoke") await client.revokeRelay(machine.machine_id);
        else await client.reEnrollMachine(machine.machine_id);
      }
      await refresh();
    }, "Machine lifecycle update failed.");
  }

  async function cancelEnrollment(machine: MachineSummary) {
    await runMutation(async () => {
      if (!window.confirm(`Cancel the currently issued enrollment token for ${machine.alias}?`))
        return;
      await client.cancelEnrollment(machine.machine_id);
      setIssuedToken(null);
      await refresh();
    }, "Enrollment cancellation failed.");
  }

  async function rebindMachine(machine: MachineSummary, credentialId: string) {
    if (
      !window.confirm(
        `Use the selected credential for future SSH connections to ${machine.alias}? Install its public key first. Existing authenticated SSH children may continue with the old credential.`,
      )
    ) {
      return;
    }
    await runMutation(async () => {
      await client.rebindMachine(machine.machine_id, credentialId);
      await refresh();
    }, "Machine credential rebind failed.");
  }

  const machineDetailId = route.startsWith("/machines/") ? route.slice("/machines/".length) : null;
  const visibleMachines =
    machineDetailId === null
      ? machines
      : machines.filter((machine) => machine.machine_id === machineDetailId);

  if (workspaceMachine !== null) {
    return (
      <InteractiveWorkspace
        client={client}
        machine={workspaceMachine}
        onAuthenticationFailure={logout}
        onClose={() => setWorkspaceMachine(null)}
      />
    );
  }

  return (
    <main className="control-shell">
      <header className="control-header">
        <div>
          <p className="eyebrow">
            {deployment?.profile === "clustered" ? "Clustered control plane" : "Control plane"}
          </p>
          <h1>OwlMux</h1>
          <p className="muted">
            {deployment ? `Deployment ${deployment.deployment_id}` : "Loading deployment…"}
          </p>
        </div>
        <button className="secondary-action" onClick={logout} type="button">
          Log out and clear key
        </button>
      </header>

      <nav className="resource-actions" aria-label="Control plane">
        <button
          className={route === "/ssh-credentials" ? "" : "secondary-action"}
          onClick={() => navigate("/ssh-credentials")}
          type="button"
        >
          SSH credentials
        </button>
        <button
          className={route.startsWith("/machines") ? "" : "secondary-action"}
          onClick={() => navigate("/machines")}
          type="button"
        >
          Machines
        </button>
      </nav>

      {error && (
        <div className="error-banner" role="alert">
          <p>{error}</p>
          {outcomeUnknown && (
            <button className="secondary-action" onClick={() => void refresh(true)} type="button">
              Refresh durable state
            </button>
          )}
        </div>
      )}
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

      <fieldset className="control-grid" disabled={outcomeUnknown || mutationPending}>
        {route === "/ssh-credentials" && (
          <section className="panel">
            <h2>SSH credentials</h2>
            <p className="muted">Target administrators install these public keys externally.</p>
            {loading && <p className="muted">Loading credentials…</p>}
            {!loading && credentials.length === 0 && (
              <p className="error-banner">No SSH credential is available.</p>
            )}
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
        )}

        {route.startsWith("/machines") && (
          <section className="panel">
            <h2>{machineDetailId === null ? "Machines" : "Machine"}</h2>
            {loading && <p className="muted">Loading Machines…</p>}
            {!loading && machines.length === 0 && (
              <p className="muted">
                No Machines yet. Create one to issue its first enrollment token.
              </p>
            )}
            <ul className="resource-list">
              {visibleMachines.map((machine) => (
                <li key={machine.machine_id}>
                  <strong>{machine.alias}</strong>
                  <span>
                    {machine.lifecycle} · {machine.reachability}
                  </span>
                  <span>Credential {machine.ssh_credential_id}</span>
                  <div className="resource-actions">
                    {machineDetailId === null && (
                      <button
                        onClick={() => navigate(`/machines/${machine.machine_id}`)}
                        type="button"
                      >
                        Manage Machine
                      </button>
                    )}
                    <button
                      className="secondary-action"
                      onClick={() => void changeMachine(machine, "rename")}
                      type="button"
                    >
                      Rename
                    </button>
                    {machine.lifecycle === "pending" && (
                      <>
                        <button onClick={() => void reissue(machine.machine_id)} type="button">
                          Issue replacement token
                        </button>
                        <button
                          className="secondary-action"
                          onClick={() => void cancelEnrollment(machine)}
                          type="button"
                        >
                          Cancel issued token
                        </button>
                      </>
                    )}
                    {machine.lifecycle === "active" && (
                      <>
                        <button onClick={() => setWorkspaceMachine(machine)} type="button">
                          Open workspace
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
                          onClick={() => void changeMachine(machine, "revoke")}
                          type="button"
                        >
                          Revoke Relay
                        </button>
                        <button
                          className="secondary-action"
                          onClick={() => void changeMachine(machine, "disable")}
                          type="button"
                        >
                          Disable
                        </button>
                      </>
                    )}
                    {machine.lifecycle === "disabled" && (
                      <button
                        onClick={() => void changeMachine(machine, "re-enroll")}
                        type="button"
                      >
                        Re-enroll as pending
                      </button>
                    )}
                  </div>
                  {machine.lifecycle === "active" && (
                    <MachineCredentialControl
                      credentials={credentials}
                      key={`${machine.machine_id}:${machine.ssh_credential_id}`}
                      machine={machine}
                      onRebind={rebindMachine}
                    />
                  )}
                </li>
              ))}
            </ul>
            {machineDetailId === null && (
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
            )}
            {machineDetailId !== null && visibleMachines.length === 0 && !loading && (
              <p className="error-banner">Machine not found.</p>
            )}
          </section>
        )}
      </fieldset>

      <section className="panel">
        <h2>Recent audit events</h2>
        <p className="muted">
          The newest 200 safe durable events are shown. Credentials, terminal data, target
          diagnostics, and internal payloads are never audit fields.
        </p>
        {loading && <p className="muted">Loading audit events…</p>}
        {!loading && auditEvents.length === 0 && <p className="muted">No audit events yet.</p>}
        <ul className="resource-list">
          {auditEvents.map((event) => (
            <li key={event.audit_event_id}>
              <strong>{event.action}</strong>
              <span>
                {event.resource_kind} · {event.outcome_class}
              </span>
              <span>{event.occurred_at}</span>
            </li>
          ))}
        </ul>
      </section>
    </main>
  );
}

function MachineCredentialControl({
  credentials,
  machine,
  onRebind,
}: {
  credentials: Array<CredentialSummary>;
  machine: MachineSummary;
  onRebind: (machine: MachineSummary, credentialId: string) => Promise<void>;
}) {
  const [credentialId, setCredentialId] = useState(machine.ssh_credential_id);
  return (
    <div className="compact-form">
      <label>
        Credential for future SSH connections
        <select
          onChange={(event) => setCredentialId(event.currentTarget.value)}
          value={credentialId}
        >
          {credentials
            .filter((credential) => credential.status === "active")
            .map((credential) => (
              <option key={credential.ssh_credential_id} value={credential.ssh_credential_id}>
                {credential.name}
              </option>
            ))}
        </select>
      </label>
      <p className="muted">
        Install the selected public key first. Rebind has no SSH preflight and affects only future
        SSH children; an existing authenticated child may continue.
      </p>
      <button
        disabled={credentialId === machine.ssh_credential_id}
        onClick={() => void onRebind(machine, credentialId)}
        type="button"
      >
        Rebind credential
      </button>
    </div>
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

function InteractiveWorkspace({
  client,
  machine,
  onAuthenticationFailure,
  onClose,
}: {
  client: ApiClient;
  machine: MachineSummary;
  onAuthenticationFailure: () => void;
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
  const [machineConnectionEpoch, setMachineConnectionEpoch] = useState("");
  const [selectionEpoch, setSelectionEpoch] = useState("");
  const [sessions, setSessions] = useState<Array<AttachmentSessionSummary>>([]);
  const [projection, setProjection] = useState<InstalledProjection | null>(null);
  const [writerRole, setWriterRole] = useState<"observer" | "writer">("observer");
  const [writerAvailable, setWriterAvailable] = useState(false);
  const [operationStatus, setOperationStatus] = useState("");
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
        setMachineConnectionEpoch(frame.machine_connection_epoch);
        setSelectionEpoch(frame.selection_epoch);
        setSessions(frame.sessions);
        setError("");
      } else if (frame.type === "writer.state") {
        setWriterRole(frame.role);
        setWriterAvailable(frame.writer_available);
      } else if (frame.type === "operation.result") {
        setOperationStatus(`${frame.outcome}: ${frame.message}`);
        if (frame.outcome === "ambiguous") {
          setError(
            "The target effect is unknown. OwlMux did not retry it; inspect the fresh chooser before acting again.",
          );
        } else if (frame.outcome === "failed") {
          setError(frame.message);
        } else {
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
        setError(frame.message);
      }
    };
    const active = client.openAttachment(
      machine.machine_id,
      onFrame,
      () => {
        resetProjection();
        setPhase("failed");
        setWriterRole("observer");
      },
      onAuthenticationFailure,
    );
    attachment.current = active;
    return () => {
      active.dispose();
      attachment.current = null;
      resetProjection();
    };
  }, [client, failRenderer, machine.machine_id, onAuthenticationFailure]);

  function attachmentEpoch(): string {
    return projection?.metadata.workspace_epoch ?? selectionEpoch;
  }

  function preferredSize(): { columns: number; rows: number } {
    return {
      columns: projection?.metadata.window.width ?? 120,
      rows: projection?.metadata.window.height ?? 36,
    };
  }

  function changeWriter(takeover: boolean) {
    const epoch = attachmentEpoch();
    if (!machineConnectionEpoch || !epoch) return;
    const size = preferredSize();
    try {
      if (takeover) {
        attachment.current?.takeOverWriter(machineConnectionEpoch, epoch, size.columns, size.rows);
      } else {
        attachment.current?.claimWriter(machineConnectionEpoch, epoch, size.columns, size.rows);
      }
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : "Writer request was not queued.");
    }
  }

  function createSession(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const form = event.currentTarget;
    const name = String(new FormData(form).get("session-name") ?? "");
    try {
      attachment.current?.createSession(machineConnectionEpoch, selectionEpoch, name);
      form.reset();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : "Session creation was not queued.");
    }
  }

  function resizeClient(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (projection === null) return;
    const data = new FormData(event.currentTarget);
    try {
      attachment.current?.resize(
        machineConnectionEpoch,
        projection.metadata.workspace_epoch,
        Number(data.get("columns")),
        Number(data.get("rows")),
      );
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : "Resize was not queued.");
    }
  }

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
          <p className="eyebrow">Target-owned tmux · {writerRole}</p>
          <h1>{machine.alias}</h1>
          <p className="muted">
            {phase} {tmuxVersion && `· ${tmuxVersion}`}
          </p>
        </div>
        <div className="workspace-actions">
          {writerRole === "observer" && phase !== "connecting" && (
            <button onClick={() => changeWriter(!writerAvailable)} type="button">
              {writerAvailable ? "Claim writer" : "Take over writer"}
            </button>
          )}
          {phase === "ready" && projection !== null && (
            <button
              className="secondary-action"
              onClick={() =>
                attachment.current?.returnToChooser(
                  machineConnectionEpoch,
                  projection.metadata.workspace_epoch,
                )
              }
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
      {operationStatus && (
        <p aria-live="polite" className="muted">
          {operationStatus}
        </p>
      )}
      {phase === "selecting" && (
        <section className="session-chooser">
          <h2>Choose a current target session</h2>
          <p className="muted">
            Selection is always explicit. Creating a session requires current writer access.
          </p>
          <button
            className="secondary-action"
            onClick={() =>
              attachment.current?.refreshSessions(machineConnectionEpoch, selectionEpoch)
            }
            type="button"
          >
            Refresh sessions
          </button>
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
                      machineConnectionEpoch,
                      selectionEpoch,
                      session.session_id,
                      session.session_created,
                    )
                  }
                  type="button"
                >
                  Open
                </button>
              </li>
            ))}
          </ul>
          <form className="compact-form" onSubmit={createSession}>
            <label htmlFor="session-name">New target tmux session</label>
            <input
              disabled={writerRole !== "writer"}
              id="session-name"
              maxLength={64}
              name="session-name"
              required
            />
            <button disabled={writerRole !== "writer"} type="submit">
              Create session
            </button>
          </form>
        </section>
      )}
      {phase === "ready" && projection !== null && (
        <section className="terminal-panel">
          <header className="target-window-header">
            <div>
              <strong>{projection.metadata.window.name}</strong>
              <span>
                {projection.metadata.window.window_id} · {projection.metadata.window.width}×
                {projection.metadata.window.height} · {projection.metadata.panes.length} visible
                panes
              </span>
            </div>
            <div className="resource-actions">
              {projection.metadata.windows.map((windowSummary) => (
                <button
                  className={windowSummary.active ? "" : "secondary-action"}
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
              <button
                className="secondary-action"
                disabled={writerRole !== "writer"}
                onClick={() =>
                  attachment.current?.refresh(
                    machineConnectionEpoch,
                    projection.metadata.workspace_epoch,
                  )
                }
                type="button"
              >
                Refresh projection
              </button>
            </div>
          </header>
          {writerRole === "writer" && (
            <form className="resize-form" onSubmit={resizeClient}>
              <label>
                Columns
                <input
                  defaultValue={projection.metadata.window.width}
                  key={`columns-${projection.metadata.workspace_epoch}`}
                  max={ATTACHMENT_MAX_DIMENSION}
                  min={1}
                  name="columns"
                  type="number"
                />
              </label>
              <label>
                Rows
                <input
                  defaultValue={projection.metadata.window.height}
                  key={`rows-${projection.metadata.workspace_epoch}`}
                  max={ATTACHMENT_MAX_DIMENSION}
                  min={1}
                  name="rows"
                  type="number"
                />
              </label>
              <button type="submit">Resize target client</button>
            </form>
          )}
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
                onInput={sendInput}
                onSelect={selectPane}
                onSink={registerSink}
                pane={pane}
                selectable={writerRole === "writer"}
                window={projection.metadata.window}
                writable={writerRole === "writer" && pane.active}
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
  onInput,
  onSelect,
  onSink,
  pane,
  selectable,
  window,
  writable,
}: {
  chunks: Array<Uint8Array>;
  onInput: (paneId: string, data: Uint8Array) => void;
  onSelect: (paneId: string) => void;
  onSink: (paneId: string, sink: PaneSink | null) => void;
  pane: AttachmentPane;
  selectable: boolean;
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
      cursorBlink: false,
      disableStdin: !writableRef.current,
      fontFamily: '"SFMono-Regular", Consolas, "Liberation Mono", monospace',
      fontSize: 12,
      rows: pane.height,
      scrollback: 0,
      theme: { background: "#111827", foreground: "#e5e7eb" },
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
        {!pane.active && (
          <button disabled={!selectable} onClick={() => onSelect(pane.pane_id)} type="button">
            Select pane
          </button>
        )}
      </header>
      <div
        aria-label={`${writable ? "Writable" : "Observer"} terminal ${pane.pane_id}`}
        className="tmux-pane-terminal"
        ref={element}
      />
    </article>
  );
}
