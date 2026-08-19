import { useCallback, useEffect, useRef, useState, type FormEvent, type ReactNode } from "react";

import { ApiError, AuthenticationError, createApiClient, type ApiClient } from "./client";
import { PUBLIC_CONTRACT_VERSION } from "./generated/contracts";
import type {
  AuditEventSummary,
  CredentialSummary,
  DeploymentPresentation,
  MachineDetail,
  MachineSummary,
} from "./generated/contracts";
import { InteractiveWorkspace } from "./workspace";

interface WorkspaceTab {
  id: string;
  machine: MachineSummary;
  sessionTitle: string | null;
}

interface Confirmation {
  confirmLabel: string;
  danger?: boolean;
  description: string;
  onConfirm: () => Promise<void>;
  title: string;
}

interface TextRequest {
  initialValue: string;
  label: string;
  onSubmit: (value: string) => Promise<void>;
  submitLabel: string;
  title: string;
}

interface EnrollmentDisclosure {
  expiresIn: number;
  token: string;
}

const MAX_WORKSPACE_TABS = 16;

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
      window.history.replaceState(null, "", "/workspaces");
      setClient(next);
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
          <div className="brand-mark" aria-hidden="true">
            O
          </div>
          <p className="section-kicker">OwlMux · {PUBLIC_CONTRACT_VERSION}</p>
          <h1>Your target work stays where it belongs.</h1>
          <p className="login-intro">
            Open your saved Hosts and continue target-owned tmux sessions from this Browser.
          </p>
          <form className="login-form" onSubmit={login}>
            <label htmlFor="api-key">Deployment API key</label>
            <input
              autoComplete="off"
              autoFocus
              id="api-key"
              name="api-key"
              onChange={(event) => setCandidate(event.currentTarget.value)}
              required
              spellCheck={false}
              type="password"
              value={candidate}
            />
            <button className="button button-primary button-large" type="submit">
              Open OwlMux
            </button>
          </form>
          <p className="memory-notice">
            The key stays only in this page's memory. Reload, navigation away, authentication
            failure, and logout clear it.
          </p>
          {loginError && (
            <div className="error-banner" role="alert">
              {loginError}
            </div>
          )}
        </section>
      </main>
    );
  }

  return <AuthenticatedApp client={client} logout={logout} />;
}

function AuthenticatedApp({ client, logout }: { client: ApiClient; logout: () => void }) {
  const [deployment, setDeployment] = useState<DeploymentPresentation | null>(null);
  const [auditEvents, setAuditEvents] = useState<Array<AuditEventSummary>>([]);
  const [credentials, setCredentials] = useState<Array<CredentialSummary>>([]);
  const [machines, setMachines] = useState<Array<MachineSummary>>([]);
  const [workspaceTabs, setWorkspaceTabs] = useState<Array<WorkspaceTab>>([]);
  const [activeWorkspaceId, setActiveWorkspaceId] = useState<string | null>(null);
  const [error, setError] = useState("");
  const [outcomeUnknown, setOutcomeUnknown] = useState(false);
  const [mutationPending, setMutationPending] = useState(false);
  const [loading, setLoading] = useState(true);
  const [resourceRevision, setResourceRevision] = useState(0);
  const [route, setRoute] = useState(() => normalizeRoute(window.location.pathname));
  const [confirmation, setConfirmation] = useState<Confirmation | null>(null);
  const [textRequest, setTextRequest] = useState<TextRequest | null>(null);
  const [enrollmentDisclosure, setEnrollmentDisclosure] = useState<EnrollmentDisclosure | null>(
    null,
  );
  const outcomeUnknownRef = useRef(false);
  const mutationPendingRef = useRef(false);
  const refreshGenerationRef = useRef(0);
  const nextWorkspaceId = useRef(0);

  const navigate = useCallback((path: string) => {
    const normalized = normalizeRoute(path);
    window.history.pushState(null, "", normalized);
    setRoute(normalized);
  }, []);

  useEffect(() => {
    const onPopState = () => setRoute(normalizeRoute(window.location.pathname));
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
          "The durable outcome is unknown. Management mutations are disabled until durable state is refreshed.",
        );
      } else if (reason instanceof ApiError && reason.code === "owner_unreachable") {
        setError(
          "The valid Host owner is unreachable. Fence or stop that Server node, wait for lease expiry, then retry.",
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
        if (generation !== refreshGenerationRef.current) return false;
        setDeployment(nextDeployment);
        setAuditEvents(nextAuditEvents);
        setCredentials(nextCredentials);
        setMachines(nextMachines);
        setResourceRevision((revision) => revision + 1);
        if (acknowledgeUnknown) {
          outcomeUnknownRef.current = false;
          setOutcomeUnknown(false);
          setError("");
        } else if (!outcomeUnknownRef.current) {
          setError("");
        }
        return true;
      } catch (reason) {
        if (generation === refreshGenerationRef.current) {
          presentError(reason, "Deployment refresh failed.");
        }
        return false;
      } finally {
        if (generation === refreshGenerationRef.current) setLoading(false);
      }
    },
    [client, presentError],
  );

  const runMutation = useCallback(
    async (operation: () => Promise<void>, fallback: string) => {
      if (mutationPendingRef.current || outcomeUnknownRef.current) return false;
      mutationPendingRef.current = true;
      setMutationPending(true);
      try {
        await operation();
        return true;
      } catch (reason) {
        presentError(reason, fallback);
        return false;
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

  const openWorkspace = useCallback(
    (machine: MachineSummary) => {
      if (workspaceTabs.length >= MAX_WORKSPACE_TABS) {
        setError(
          `This page already has ${MAX_WORKSPACE_TABS} workspace tabs. Close one before opening another Host.`,
        );
        return;
      }
      const id = `workspace-${++nextWorkspaceId.current}`;
      setWorkspaceTabs((tabs) => [...tabs, { id, machine, sessionTitle: null }]);
      setActiveWorkspaceId(id);
      navigate("/workspaces");
    },
    [navigate, workspaceTabs.length],
  );

  const closeWorkspace = useCallback(
    (id: string) => {
      const index = workspaceTabs.findIndex((tab) => tab.id === id);
      if (index < 0) return;
      const remaining = workspaceTabs.filter((tab) => tab.id !== id);
      setWorkspaceTabs(remaining);
      if (activeWorkspaceId === id) {
        setActiveWorkspaceId(remaining[Math.min(index, remaining.length - 1)]?.id ?? null);
      }
    },
    [activeWorkspaceId, workspaceTabs],
  );

  const updateWorkspaceTitle = useCallback((id: string, sessionTitle: string | null) => {
    setWorkspaceTabs((tabs) => tabs.map((tab) => (tab.id === id ? { ...tab, sessionTitle } : tab)));
  }, []);

  async function createHost(input: {
    alias: string;
    host_identity: string;
    ssh_credential_id?: string;
    target_account: string;
    tmux_path: string;
    tmux_socket_identity: string;
  }) {
    let disclosure: EnrollmentDisclosure | null = null;
    let machineId = "";
    const completed = await runMutation(async () => {
      const created = await client.createMachine(input);
      machineId = created.machine.machine_id;
      disclosure = {
        expiresIn: created.enrollment_expires_in,
        token: created.enrollment_token,
      };
      await refresh();
    }, "Host creation failed.");
    if (completed && disclosure !== null) {
      setEnrollmentDisclosure(disclosure);
      navigate(`/hosts/${machineId}`);
    }
  }

  async function createCredential(name: string) {
    await runMutation(async () => {
      await client.createCredential({ name });
      await refresh();
    }, "Credential creation failed.");
  }

  async function updateCredential(
    credential: CredentialSummary | null,
    action: "rename" | "default" | "reset" | "retire",
    value?: string,
  ) {
    await runMutation(async () => {
      if (action === "rename" && credential !== null && value !== undefined) {
        await client.renameCredential(credential.ssh_credential_id, value);
      } else if (action === "default" && credential !== null) {
        await client.setDefaultCredential(credential.ssh_credential_id);
      } else if (action === "reset" && value !== undefined) {
        await client.resetDefaultCredential(value);
      } else if (action === "retire" && credential !== null) {
        await client.retireCredential(credential.ssh_credential_id);
      }
      await refresh();
    }, "Credential update failed.");
  }

  async function renameHost(machine: MachineSummary, alias: string) {
    await runMutation(async () => {
      await client.renameMachine(machine.machine_id, alias);
      await refresh();
    }, "Host rename failed.");
  }

  async function changeHostLifecycle(
    machine: MachineSummary,
    action: "disable" | "revoke" | "re-enroll",
  ) {
    await runMutation(async () => {
      if (action === "disable") await client.disableMachine(machine.machine_id);
      else if (action === "revoke") await client.revokeRelay(machine.machine_id);
      else await client.reEnrollMachine(machine.machine_id);
      await refresh();
    }, "Host lifecycle update failed.");
  }

  async function issueEnrollment(machine: MachineSummary) {
    let disclosure: EnrollmentDisclosure | null = null;
    const completed = await runMutation(async () => {
      const issued = await client.issueEnrollment(machine.machine_id);
      disclosure = {
        expiresIn: issued.enrollment_expires_in,
        token: issued.enrollment_token,
      };
      await refresh();
    }, "Enrollment token issuance failed.");
    if (completed && disclosure !== null) setEnrollmentDisclosure(disclosure);
  }

  async function cancelEnrollment(machine: MachineSummary) {
    await runMutation(async () => {
      await client.cancelEnrollment(machine.machine_id);
      await refresh();
    }, "Enrollment cancellation failed.");
  }

  async function rebindHost(machine: MachineSummary, credentialId: string) {
    await runMutation(async () => {
      await client.rebindMachine(machine.machine_id, credentialId);
      await refresh();
    }, "Host credential rebind failed.");
  }

  const managementDisabled = outcomeUnknown || mutationPending;
  const terminalVisible = route === "/workspaces" && activeWorkspaceId !== null;

  return (
    <div className={terminalVisible ? "authenticated-shell is-terminal" : "authenticated-shell"}>
      <AppHeader deployment={deployment} logout={logout} navigate={navigate} route={route} />

      {route === "/workspaces" && workspaceTabs.length > 0 && (
        <WorkspaceTabs
          activeId={activeWorkspaceId}
          onActivate={setActiveWorkspaceId}
          onClose={closeWorkspace}
          tabs={workspaceTabs}
        />
      )}

      {error && (
        <div className="global-error" role="alert">
          <span>{error}</span>
          {outcomeUnknown && (
            <button
              className="button button-secondary button-compact"
              onClick={() => void refresh(true)}
              type="button"
            >
              Refresh durable state
            </button>
          )}
          {!outcomeUnknown && (
            <button
              aria-label="Dismiss error"
              className="icon-button"
              onClick={() => setError("")}
              type="button"
            >
              ×
            </button>
          )}
        </div>
      )}

      {route === "/workspaces" && activeWorkspaceId === null && (
        <WorkspaceHome
          loading={loading}
          machines={machines}
          navigate={navigate}
          onOpen={openWorkspace}
          openTabs={workspaceTabs.length}
        />
      )}

      <div
        aria-hidden={route !== "/workspaces" || activeWorkspaceId === null}
        className={
          route === "/workspaces" && activeWorkspaceId !== null
            ? "workspace-stack"
            : "workspace-stack is-hidden"
        }
      >
        {workspaceTabs.map((tab) => (
          <div
            className={tab.id === activeWorkspaceId ? "workspace-slot is-active" : "workspace-slot"}
            key={tab.id}
          >
            <InteractiveWorkspace
              client={client}
              machine={tab.machine}
              onAuthenticationFailure={logout}
              onClose={() => closeWorkspace(tab.id)}
              onTitleChange={(title) => updateWorkspaceTitle(tab.id, title)}
              visible={route === "/workspaces" && tab.id === activeWorkspaceId}
            />
          </div>
        ))}
      </div>

      {route === "/hosts" && (
        <HostsPage
          disabled={managementDisabled}
          loading={loading}
          machines={machines}
          navigate={navigate}
          onOpen={openWorkspace}
        />
      )}

      {route === "/hosts/new" && (
        <HostCreatePage
          credentials={credentials}
          disabled={managementDisabled}
          navigate={navigate}
          onCreate={createHost}
        />
      )}

      {route.startsWith("/hosts/") && route !== "/hosts/new" && (
        <HostDetailPage
          client={client}
          credentials={credentials}
          disabled={managementDisabled}
          machineId={route.slice("/hosts/".length)}
          navigate={navigate}
          onCancelEnrollment={(machine) =>
            setConfirmation({
              confirmLabel: "Cancel token",
              description: `Cancel the currently issued enrollment token for ${machine.alias}?`,
              onConfirm: () => cancelEnrollment(machine),
              title: "Cancel enrollment token",
            })
          }
          onError={presentError}
          onIssueEnrollment={issueEnrollment}
          onLifecycle={(machine, action) =>
            setConfirmation(hostLifecycleConfirmation(machine, action, changeHostLifecycle))
          }
          onOpen={openWorkspace}
          onRebind={(machine, credentialId) =>
            setConfirmation({
              confirmLabel: "Rebind credential",
              description: `Use the selected credential for future SSH connections to ${machine.alias}? Install its public key first. Existing authenticated SSH children may continue with the old credential.`,
              onConfirm: () => rebindHost(machine, credentialId),
              title: "Rebind Host credential",
            })
          }
          onRename={(machine) =>
            setTextRequest({
              initialValue: machine.alias,
              label: "Host name",
              onSubmit: (alias) => renameHost(machine, alias),
              submitLabel: "Rename Host",
              title: `Rename ${machine.alias}`,
            })
          }
          resourceRevision={resourceRevision}
        />
      )}

      {route === "/ssh-credentials" && (
        <CredentialsPage
          credentials={credentials}
          disabled={managementDisabled}
          loading={loading}
          onCopyError={(reason) => presentError(reason, "Could not copy the public key.")}
          onCreate={createCredential}
          onDefault={(credential) => void updateCredential(credential, "default")}
          onRename={(credential) =>
            setTextRequest({
              initialValue: credential.name,
              label: "Credential name",
              onSubmit: (name) => updateCredential(credential, "rename", name),
              submitLabel: "Rename credential",
              title: `Rename ${credential.name}`,
            })
          }
          onReset={() =>
            setTextRequest({
              initialValue: "Reset default",
              label: "New generated default credential name",
              onSubmit: (name) => updateCredential(null, "reset", name),
              submitLabel: "Generate and reset default",
              title: "Reset default credential",
            })
          }
          onRetire={(credential) =>
            setConfirmation({
              confirmLabel: "Retire credential",
              danger: true,
              description: `Retire ${credential.name}? Retired credentials cannot be selected for future SSH connections.`,
              onConfirm: () => updateCredential(credential, "retire"),
              title: "Retire credential",
            })
          }
        />
      )}

      {route === "/audit" && <AuditPage events={auditEvents} loading={loading} />}

      {route === "/deployment" && (
        <DeploymentPage
          deployment={deployment}
          logout={logout}
          workspaceCount={workspaceTabs.length}
        />
      )}

      {confirmation !== null && (
        <ConfirmDialog
          confirmation={confirmation}
          disabled={mutationPending}
          onClose={() => setConfirmation(null)}
        />
      )}
      {textRequest !== null && (
        <TextDialog
          disabled={mutationPending}
          onClose={() => setTextRequest(null)}
          request={textRequest}
        />
      )}
      {enrollmentDisclosure !== null && (
        <EnrollmentDialog
          disclosure={enrollmentDisclosure}
          onClose={() => setEnrollmentDisclosure(null)}
          onCopyError={(reason) => presentError(reason, "Could not copy the enrollment token.")}
        />
      )}
    </div>
  );
}

function AppHeader({
  deployment,
  logout,
  navigate,
  route,
}: {
  deployment: DeploymentPresentation | null;
  logout: () => void;
  navigate: (path: string) => void;
  route: string;
}) {
  const items = [
    ["Workspaces", "/workspaces"],
    ["Hosts", "/hosts"],
    ["Credentials", "/ssh-credentials"],
    ["Audit", "/audit"],
  ] as const;
  return (
    <header className="app-header">
      <button className="brand" onClick={() => navigate("/workspaces")} type="button">
        <span className="brand-mark" aria-hidden="true">
          O
        </span>
        <span>OwlMux</span>
      </button>
      <nav className="primary-navigation" aria-label="OwlMux">
        {items.map(([label, path]) => (
          <button
            aria-current={
              route === path || (path === "/hosts" && route.startsWith("/hosts"))
                ? "page"
                : undefined
            }
            className="navigation-item"
            key={path}
            onClick={() => navigate(path)}
            type="button"
          >
            {label}
          </button>
        ))}
      </nav>
      <div className="header-meta">
        <button className="deployment-link" onClick={() => navigate("/deployment")} type="button">
          <span className="status-dot" />
          {deployment?.profile === "clustered" ? "Clustered" : "Deployment"}
        </button>
        <button className="button button-ghost button-compact" onClick={logout} type="button">
          Log out
        </button>
      </div>
    </header>
  );
}

function WorkspaceTabs({
  activeId,
  onActivate,
  onClose,
  tabs,
}: {
  activeId: string | null;
  onActivate: (id: string | null) => void;
  onClose: (id: string) => void;
  tabs: Array<WorkspaceTab>;
}) {
  return (
    <nav className="workspace-tabs" aria-label="Open workspaces">
      <button
        aria-current={activeId === null ? "page" : undefined}
        className={activeId === null ? "workspace-tab is-active" : "workspace-tab"}
        onClick={() => onActivate(null)}
        type="button"
      >
        Hosts
      </button>
      {tabs.map((tab) => (
        <div
          className={tab.id === activeId ? "workspace-tab is-active" : "workspace-tab"}
          key={tab.id}
        >
          <button className="workspace-tab-label" onClick={() => onActivate(tab.id)} type="button">
            {tab.machine.alias}
            {tab.sessionTitle !== null && <span>/ {tab.sessionTitle}</span>}
          </button>
          <button
            aria-label={`Close ${tab.machine.alias} workspace`}
            className="workspace-tab-close"
            onClick={() => onClose(tab.id)}
            type="button"
          >
            ×
          </button>
        </div>
      ))}
    </nav>
  );
}

function WorkspaceHome({
  loading,
  machines,
  navigate,
  onOpen,
  openTabs,
}: {
  loading: boolean;
  machines: Array<MachineSummary>;
  navigate: (path: string) => void;
  onOpen: (machine: MachineSummary) => void;
  openTabs: number;
}) {
  const [query, setQuery] = useState("");
  const searchInput = useRef<HTMLInputElement | null>(null);

  useEffect(() => {
    const focusSearch = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && !event.altKey && event.key.toLowerCase() === "k") {
        event.preventDefault();
        searchInput.current?.focus();
      }
    };
    window.addEventListener("keydown", focusSearch);
    return () => window.removeEventListener("keydown", focusSearch);
  }, []);

  const visibleMachines = machines.filter((machine) =>
    machine.alias.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase()),
  );
  return (
    <main className="page-shell workspace-home">
      <header className="page-heading">
        <div>
          <p className="section-kicker">Terminal workspaces</p>
          <h1>Continue on a saved Host</h1>
          <p>Open a current target, choose a tmux session, and keep target work on the target.</p>
        </div>
        <button
          className="button button-primary"
          onClick={() => navigate("/hosts/new")}
          type="button"
        >
          Add Host
        </button>
      </header>

      <label className="search-field">
        <span className="visually-hidden">Search Hosts</span>
        <input
          onChange={(event) => setQuery(event.currentTarget.value)}
          placeholder="Search Hosts"
          ref={searchInput}
          type="search"
          value={query}
        />
        <kbd>Ctrl/⌘ K</kbd>
      </label>

      {loading && <LoadingCards label="Loading saved Hosts…" />}
      {!loading && machines.length === 0 && (
        <div className="empty-state">
          <h2>No saved Hosts yet</h2>
          <p>Add one fixed target identity, enroll its Relay, then open target tmux here.</p>
          <button
            className="button button-primary"
            onClick={() => navigate("/hosts/new")}
            type="button"
          >
            Add your first Host
          </button>
        </div>
      )}
      {!loading && machines.length > 0 && visibleMachines.length === 0 && (
        <div className="empty-state compact">
          <h2>No matching Hosts</h2>
          <p>Try another name.</p>
        </div>
      )}
      <div className="host-card-grid">
        {visibleMachines.map((machine) => (
          <HostCard
            key={machine.machine_id}
            machine={machine}
            navigate={navigate}
            onOpen={onOpen}
          />
        ))}
      </div>

      <section className="page-memory-card">
        <span className="status-pill is-neutral">
          {openTabs} open workspace{openTabs === 1 ? "" : "s"}
        </span>
        <p>
          Workspace tabs exist only in this page. Closing or reloading it detaches OwlMux while
          target tmux sessions and processes continue.
        </p>
      </section>
    </main>
  );
}

function HostCard({
  machine,
  navigate,
  onOpen,
}: {
  machine: MachineSummary;
  navigate: (path: string) => void;
  onOpen: (machine: MachineSummary) => void;
}) {
  const canOpen = machine.lifecycle === "active";
  return (
    <article className="host-card">
      <header>
        <div>
          <h2>{machine.alias}</h2>
          <p>{hostLifecycleLabel(machine.lifecycle)}</p>
        </div>
        <StatusPill machine={machine} />
      </header>
      <div className="host-card-detail">
        <span>OwlMux Host</span>
        <code>{shortId(machine.machine_id)}</code>
      </div>
      <footer>
        {canOpen && (
          <button className="button button-primary" onClick={() => onOpen(machine)} type="button">
            Open
          </button>
        )}
        <button
          className="button button-secondary"
          onClick={() => navigate(`/hosts/${machine.machine_id}`)}
          type="button"
        >
          Manage
        </button>
      </footer>
    </article>
  );
}

function HostsPage({
  disabled,
  loading,
  machines,
  navigate,
  onOpen,
}: {
  disabled: boolean;
  loading: boolean;
  machines: Array<MachineSummary>;
  navigate: (path: string) => void;
  onOpen: (machine: MachineSummary) => void;
}) {
  const [query, setQuery] = useState("");
  const visible = machines.filter((machine) =>
    machine.alias.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase()),
  );
  return (
    <main className="page-shell">
      <header className="page-heading">
        <div>
          <p className="section-kicker">Management</p>
          <h1>Hosts</h1>
          <p>Saved fixed SSH identities and tmux scopes reached through enrolled Relays.</p>
        </div>
        <button
          className="button button-primary"
          disabled={disabled}
          onClick={() => navigate("/hosts/new")}
          type="button"
        >
          Add Host
        </button>
      </header>
      <label className="search-field narrow">
        <span className="visually-hidden">Search Hosts</span>
        <input
          onChange={(event) => setQuery(event.currentTarget.value)}
          placeholder="Search Hosts"
          type="search"
          value={query}
        />
      </label>
      {loading && <LoadingCards label="Loading Hosts…" />}
      {!loading && visible.length === 0 && (
        <div className="empty-state compact">
          <h2>{machines.length === 0 ? "No Hosts yet" : "No matching Hosts"}</h2>
          <p>
            {machines.length === 0
              ? "Add a fixed target access scope to begin."
              : "Try another name."}
          </p>
        </div>
      )}
      <div className="host-table" role="list">
        {visible.map((machine) => (
          <article className="host-table-row" key={machine.machine_id} role="listitem">
            <div className="host-table-name">
              <strong>{machine.alias}</strong>
              <code>{shortId(machine.machine_id)}</code>
            </div>
            <StatusPill machine={machine} />
            <span>{hostLifecycleLabel(machine.lifecycle)}</span>
            <div className="row-actions">
              {machine.lifecycle === "active" && (
                <button
                  className="button button-primary button-compact"
                  onClick={() => onOpen(machine)}
                  type="button"
                >
                  Open
                </button>
              )}
              <button
                className="button button-secondary button-compact"
                onClick={() => navigate(`/hosts/${machine.machine_id}`)}
                type="button"
              >
                Manage
              </button>
            </div>
          </article>
        ))}
      </div>
    </main>
  );
}

function HostCreatePage({
  credentials,
  disabled,
  navigate,
  onCreate,
}: {
  credentials: Array<CredentialSummary>;
  disabled: boolean;
  navigate: (path: string) => void;
  onCreate: (input: {
    alias: string;
    host_identity: string;
    ssh_credential_id?: string;
    target_account: string;
    tmux_path: string;
    tmux_socket_identity: string;
  }) => Promise<void>;
}) {
  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const form = event.currentTarget;
    const data = new FormData(form);
    const credentialId = String(data.get("ssh_credential_id") ?? "");
    await onCreate({
      alias: String(data.get("alias") ?? ""),
      host_identity: String(data.get("host_identity") ?? ""),
      ...(credentialId.length === 0 ? {} : { ssh_credential_id: credentialId }),
      target_account: String(data.get("target_account") ?? ""),
      tmux_path: String(data.get("tmux_path") ?? "/usr/bin/tmux"),
      tmux_socket_identity: String(data.get("tmux_socket_identity") ?? "default"),
    });
  }
  return (
    <main className="page-shell narrow-page">
      <button className="back-link" onClick={() => navigate("/hosts")} type="button">
        Back to Hosts
      </button>
      <header className="page-heading">
        <div>
          <p className="section-kicker">Host setup</p>
          <h1>Add a saved Host</h1>
          <p>Create one immutable target identity and issue its first one-use Relay token.</p>
        </div>
      </header>
      <form className="host-setup-form" onSubmit={submit}>
        <fieldset disabled={disabled}>
          <section className="settings-card">
            <header>
              <span className="step-number">1</span>
              <div>
                <h2>Target identity</h2>
                <p>These fields fix the SSH account and verified target host boundary.</p>
              </div>
            </header>
            <div className="form-grid two-columns">
              <label>
                Host name
                <input maxLength={64} name="alias" placeholder="Production" required />
              </label>
              <label>
                Target account
                <input maxLength={64} name="target_account" placeholder="deploy" required />
              </label>
            </div>
            <label>
              Expected SSH host public key
              <textarea
                name="host_identity"
                placeholder="ssh-ed25519 AAAA…"
                required
                rows={4}
                spellCheck={false}
              />
              <span className="field-help">
                OwlMux always verifies this exact identity. A host-key change requires a new Host.
              </span>
            </label>
          </section>

          <section className="settings-card">
            <header>
              <span className="step-number">2</span>
              <div>
                <h2>SSH credential</h2>
                <p>
                  Select a generated Deployment credential and install its public key externally.
                </p>
              </div>
            </header>
            <label>
              Credential
              <select defaultValue="" name="ssh_credential_id">
                <option value="">Deployment default</option>
                {credentials
                  .filter((credential) => credential.status === "active")
                  .map((credential) => (
                    <option key={credential.ssh_credential_id} value={credential.ssh_credential_id}>
                      {credential.name}
                      {credential.is_default ? " · default" : ""}
                    </option>
                  ))}
              </select>
            </label>
            <p className="callout neutral">
              OwlMux never edits authorized_keys. Install the selected public key on the target
              account before enrolling Relay.
            </p>
          </section>

          <details className="settings-card advanced-settings">
            <summary>Advanced tmux scope</summary>
            <p>The defaults cover a standard target-owned tmux installation.</p>
            <div className="form-grid two-columns">
              <label>
                tmux path
                <input defaultValue="/usr/bin/tmux" name="tmux_path" required />
              </label>
              <label>
                tmux socket identity
                <input defaultValue="default" name="tmux_socket_identity" required />
              </label>
            </div>
          </details>

          <footer className="form-actions">
            <button
              className="button button-secondary"
              onClick={() => navigate("/hosts")}
              type="button"
            >
              Cancel
            </button>
            <button className="button button-primary" type="submit">
              Create Host and issue token
            </button>
          </footer>
        </fieldset>
      </form>
    </main>
  );
}

function HostDetailPage({
  client,
  credentials,
  disabled,
  machineId,
  navigate,
  onCancelEnrollment,
  onError,
  onIssueEnrollment,
  onLifecycle,
  onOpen,
  onRebind,
  onRename,
  resourceRevision,
}: {
  client: ApiClient;
  credentials: Array<CredentialSummary>;
  disabled: boolean;
  machineId: string;
  navigate: (path: string) => void;
  onCancelEnrollment: (machine: MachineSummary) => void;
  onError: (reason: unknown, fallback: string) => void;
  onIssueEnrollment: (machine: MachineSummary) => Promise<void>;
  onLifecycle: (machine: MachineSummary, action: "disable" | "revoke" | "re-enroll") => void;
  onOpen: (machine: MachineSummary) => void;
  onRebind: (machine: MachineSummary, credentialId: string) => void;
  onRename: (machine: MachineSummary) => void;
  resourceRevision: number;
}) {
  const [detail, setDetail] = useState<MachineDetail | null>(null);
  const [loading, setLoading] = useState(true);
  useEffect(() => {
    let current = true;
    void client
      .getMachine(machineId)
      .then((next) => {
        if (current) setDetail(next);
      })
      .catch((reason: unknown) => {
        if (current) onError(reason, "Host detail could not be loaded.");
      })
      .finally(() => {
        if (current) setLoading(false);
      });
    return () => {
      current = false;
    };
  }, [client, machineId, onError, resourceRevision]);

  if (loading && detail === null) {
    return (
      <main className="page-shell">
        <LoadingCards label="Loading Host…" />
      </main>
    );
  }
  if (detail === null) {
    return (
      <main className="page-shell narrow-page">
        <div className="empty-state">
          <h1>Host not found</h1>
          <button
            className="button button-primary"
            onClick={() => navigate("/hosts")}
            type="button"
          >
            Back to Hosts
          </button>
        </div>
      </main>
    );
  }
  const machine: MachineSummary = detail;
  const credential = credentials.find(
    (candidate) => candidate.ssh_credential_id === detail.ssh_credential_id,
  );
  return (
    <main className="page-shell host-detail-page">
      <button className="back-link" onClick={() => navigate("/hosts")} type="button">
        Back to Hosts
      </button>
      <header className="host-detail-heading">
        <div>
          <div className="heading-with-status">
            <h1>{detail.alias}</h1>
            <StatusPill machine={detail} />
          </div>
          <p>
            {detail.target_account} · {hostLifecycleLabel(detail.lifecycle)} ·{" "}
            {shortId(detail.machine_id)}
          </p>
        </div>
        <div className="row-actions">
          <button
            className="button button-secondary"
            disabled={disabled}
            onClick={() => onRename(machine)}
            type="button"
          >
            Rename
          </button>
          {detail.lifecycle === "active" && (
            <button className="button button-primary" onClick={() => onOpen(machine)} type="button">
              Open workspace
            </button>
          )}
        </div>
      </header>

      <nav className="section-navigation" aria-label="Host sections">
        <a href="#overview">Overview</a>
        <a href="#ssh-identity">SSH identity</a>
        <a href="#tmux">tmux</a>
        <a href="#relay">Relay</a>
        <a href="#danger-zone">Danger zone</a>
      </nav>

      <fieldset className="settings-sections" disabled={disabled}>
        <section className="settings-card" id="overview">
          <header>
            <div>
              <h2>Overview</h2>
              <p>Durable Host identity and current advisory reachability.</p>
            </div>
          </header>
          <dl className="detail-grid">
            <Detail label="Lifecycle" value={hostLifecycleLabel(detail.lifecycle)} />
            <Detail label="Reachability" value={reachabilityLabel(detail.reachability)} />
            <Detail label="Machine ID" value={detail.machine_id} code />
            <Detail label="Target account" value={detail.target_account} code />
          </dl>
        </section>

        <section className="settings-card" id="ssh-identity">
          <header>
            <div>
              <h2>SSH identity</h2>
              <p>The target administrator controls sshd and public-key authorization.</p>
            </div>
          </header>
          <dl className="detail-grid">
            <Detail label="Bound credential" value={credential?.name ?? detail.ssh_credential_id} />
            <Detail
              label="Credential fingerprint"
              value={credential?.public_fingerprint_sha256 ?? "Unavailable"}
              code
            />
          </dl>
          <label>
            Expected target SSH host public key
            <textarea readOnly rows={4} spellCheck={false} value={detail.host_identity} />
          </label>
          <MachineCredentialControl
            credentials={credentials}
            key={`${detail.machine_id}:${detail.ssh_credential_id}`}
            machine={detail}
            onRebind={onRebind}
          />
        </section>

        <section className="settings-card" id="tmux">
          <header>
            <div>
              <h2>Target-owned tmux scope</h2>
              <p>These immutable values define the only tmux client scope OwlMux may enter.</p>
            </div>
          </header>
          <dl className="detail-grid">
            <Detail label="tmux path" value={detail.tmux_path} code />
            <Detail label="Socket identity" value={detail.tmux_socket_identity} code />
          </dl>
          <p className="callout neutral">
            Changing the SSH host identity, target account, tmux path, or socket scope requires a
            new Host registration.
          </p>
        </section>

        <section className="settings-card" id="relay">
          <header>
            <div>
              <h2>Relay enrollment</h2>
              <p>The target Relay establishes the outbound route to its fixed loopback sshd.</p>
            </div>
          </header>
          {detail.lifecycle === "pending" && (
            <div className="action-block">
              <div>
                <strong>Waiting for enrollment</strong>
                <p>Issue a one-use token and configure it on the target Relay.</p>
              </div>
              <div className="row-actions">
                <button
                  className="button button-primary"
                  onClick={() => void onIssueEnrollment(machine)}
                  type="button"
                >
                  Issue replacement token
                </button>
                <button
                  className="button button-secondary"
                  onClick={() => onCancelEnrollment(machine)}
                  type="button"
                >
                  Cancel issued token
                </button>
              </div>
            </div>
          )}
          {detail.lifecycle === "verifying" && (
            <p className="callout warning">
              Relay is completing bounded SSH access verification. A failed or expired attempt
              returns this Host to pending without a reusable token.
            </p>
          )}
          {detail.lifecycle === "active" && (
            <div className="action-block">
              <div>
                <strong>Relay active</strong>
                <p>Re-enrollment closes current OwlMux access and returns this Host to pending.</p>
              </div>
              <button
                className="button button-secondary"
                onClick={() => onLifecycle(machine, "re-enroll")}
                type="button"
              >
                Re-enroll Relay
              </button>
            </div>
          )}
          {detail.lifecycle === "disabled" && (
            <div className="action-block">
              <div>
                <strong>Host disabled</strong>
                <p>Start explicit re-enrollment to make OwlMux access possible again.</p>
              </div>
              <button
                className="button button-primary"
                onClick={() => onLifecycle(machine, "re-enroll")}
                type="button"
              >
                Re-enroll as pending
              </button>
            </div>
          )}
        </section>

        <section className="settings-card danger-zone" id="danger-zone">
          <header>
            <div>
              <h2>Danger zone</h2>
              <p>
                These actions close OwlMux access only. They never stop target tmux or processes.
              </p>
            </div>
          </header>
          {detail.lifecycle === "active" && (
            <div className="danger-actions">
              <button
                className="button button-danger"
                onClick={() => onLifecycle(machine, "revoke")}
                type="button"
              >
                Revoke Relay access
              </button>
              <button
                className="button button-danger"
                onClick={() => onLifecycle(machine, "disable")}
                type="button"
              >
                Disable Host
              </button>
            </div>
          )}
          {detail.lifecycle !== "active" && (
            <p>No active OwlMux route is available for this Host.</p>
          )}
        </section>
      </fieldset>
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
  onRebind: (machine: MachineSummary, credentialId: string) => void;
}) {
  const [credentialId, setCredentialId] = useState(machine.ssh_credential_id);
  return (
    <div className="credential-rebind">
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
                {credential.is_default ? " · default" : ""}
              </option>
            ))}
        </select>
      </label>
      <p>
        Install the selected public key first. Rebind has no SSH preflight and affects only future
        SSH children.
      </p>
      <button
        className="button button-secondary"
        disabled={credentialId === machine.ssh_credential_id}
        onClick={() => onRebind(machine, credentialId)}
        type="button"
      >
        Rebind credential
      </button>
    </div>
  );
}

function CredentialsPage({
  credentials,
  disabled,
  loading,
  onCopyError,
  onCreate,
  onDefault,
  onRename,
  onReset,
  onRetire,
}: {
  credentials: Array<CredentialSummary>;
  disabled: boolean;
  loading: boolean;
  onCopyError: (reason: unknown) => void;
  onCreate: (name: string) => Promise<void>;
  onDefault: (credential: CredentialSummary) => void;
  onRename: (credential: CredentialSummary) => void;
  onReset: () => void;
  onRetire: (credential: CredentialSummary) => void;
}) {
  async function create(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const form = event.currentTarget;
    const name = String(new FormData(form).get("name") ?? "");
    await onCreate(name);
    form.reset();
  }
  async function copy(value: string) {
    try {
      await navigator.clipboard.writeText(value);
    } catch (reason) {
      onCopyError(reason);
    }
  }
  return (
    <main className="page-shell">
      <header className="page-heading">
        <div>
          <p className="section-kicker">Management</p>
          <h1>SSH credentials</h1>
          <p>Deployment-owned generated Ed25519 keys for future constrained SSH connections.</p>
        </div>
        <button
          className="button button-secondary"
          disabled={disabled}
          onClick={onReset}
          type="button"
        >
          Reset default
        </button>
      </header>
      <fieldset disabled={disabled}>
        <form className="inline-create-form" onSubmit={create}>
          <label>
            New generated credential name
            <input maxLength={64} name="name" placeholder="Production credential" required />
          </label>
          <button className="button button-primary" type="submit">
            Generate credential
          </button>
        </form>
        {loading && <LoadingCards label="Loading credentials…" />}
        <div className="credential-list">
          {credentials.map((credential) => (
            <article className="credential-card" key={credential.ssh_credential_id}>
              <header>
                <div>
                  <h2>{credential.name}</h2>
                  <span>
                    {credential.status} · {credential.bound_machine_count} bound Host
                    {credential.bound_machine_count === 1 ? "" : "s"}
                  </span>
                </div>
                {credential.is_default && <span className="status-pill is-good">Default</span>}
              </header>
              <code>{credential.public_fingerprint_sha256}</code>
              <div className="row-actions">
                <button
                  className="button button-primary button-compact"
                  onClick={() => void copy(credential.public_key)}
                  type="button"
                >
                  Copy public key
                </button>
                <button
                  className="button button-secondary button-compact"
                  onClick={() => onRename(credential)}
                  type="button"
                >
                  Rename
                </button>
                {!credential.is_default && credential.status === "active" && (
                  <button
                    className="button button-secondary button-compact"
                    onClick={() => onDefault(credential)}
                    type="button"
                  >
                    Make default
                  </button>
                )}
                {!credential.is_default &&
                  credential.status === "active" &&
                  credential.bound_machine_count === 0 && (
                    <button
                      className="button button-danger button-compact"
                      onClick={() => onRetire(credential)}
                      type="button"
                    >
                      Retire
                    </button>
                  )}
              </div>
            </article>
          ))}
        </div>
      </fieldset>
    </main>
  );
}

function AuditPage({ events, loading }: { events: Array<AuditEventSummary>; loading: boolean }) {
  return (
    <main className="page-shell">
      <header className="page-heading">
        <div>
          <p className="section-kicker">Management</p>
          <h1>Audit</h1>
          <p>
            Newest safe durable control events. Terminal data and internal payloads never appear.
          </p>
        </div>
      </header>
      {loading && <LoadingCards label="Loading audit events…" />}
      {!loading && events.length === 0 && (
        <div className="empty-state compact">
          <h2>No audit events yet</h2>
        </div>
      )}
      <div className="audit-list" role="list">
        {events.map((event) => (
          <article className="audit-row" key={event.audit_event_id} role="listitem">
            <span className={`audit-outcome is-${event.outcome_class}`} />
            <div>
              <strong>{event.action}</strong>
              <span>
                {event.resource_kind} · {event.outcome_class}
              </span>
            </div>
            <time dateTime={event.occurred_at}>{event.occurred_at}</time>
          </article>
        ))}
      </div>
    </main>
  );
}

function DeploymentPage({
  deployment,
  logout,
  workspaceCount,
}: {
  deployment: DeploymentPresentation | null;
  logout: () => void;
  workspaceCount: number;
}) {
  return (
    <main className="page-shell narrow-page">
      <header className="page-heading">
        <div>
          <p className="section-kicker">Management</p>
          <h1>Deployment</h1>
          <p>Safe current Deployment and Browser-session information.</p>
        </div>
      </header>
      <section className="settings-card">
        <dl className="detail-grid">
          <Detail label="Deployment ID" value={deployment?.deployment_id ?? "Loading…"} code />
          <Detail label="Profile" value={deployment?.profile ?? "Loading…"} />
          <Detail
            label="Configuration epoch"
            value={String(deployment?.config_epoch ?? "Loading…")}
          />
          <Detail label="Server build" value={deployment?.server_build_id ?? "Loading…"} code />
          <Detail label="Open workspace tabs" value={String(workspaceCount)} />
        </dl>
      </section>
      <section className="settings-card">
        <h2>Browser authentication</h2>
        <p>
          The API key and workspace tabs exist only in this page. Logout closes OwlMux connections
          and clears the key without stopping target work.
        </p>
        <button className="button button-danger" onClick={logout} type="button">
          Log out and clear key
        </button>
      </section>
    </main>
  );
}

function ConfirmDialog({
  confirmation,
  disabled,
  onClose,
}: {
  confirmation: Confirmation;
  disabled: boolean;
  onClose: () => void;
}) {
  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    await confirmation.onConfirm();
    onClose();
  }
  return (
    <Dialog onClose={onClose} title={confirmation.title}>
      <form className="dialog-form" onSubmit={submit}>
        <p>{confirmation.description}</p>
        <div className="dialog-actions">
          <button
            className="button button-secondary"
            disabled={disabled}
            onClick={onClose}
            type="button"
          >
            Cancel
          </button>
          <button
            className={confirmation.danger ? "button button-danger" : "button button-primary"}
            disabled={disabled}
            type="submit"
          >
            {confirmation.confirmLabel}
          </button>
        </div>
      </form>
    </Dialog>
  );
}

function TextDialog({
  disabled,
  onClose,
  request,
}: {
  disabled: boolean;
  onClose: () => void;
  request: TextRequest;
}) {
  const [value, setValue] = useState(request.initialValue);
  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    await request.onSubmit(value);
    onClose();
  }
  return (
    <Dialog onClose={onClose} title={request.title}>
      <form className="dialog-form" onSubmit={submit}>
        <label>
          {request.label}
          <input
            autoFocus
            maxLength={64}
            onChange={(event) => setValue(event.currentTarget.value)}
            required
            value={value}
          />
        </label>
        <div className="dialog-actions">
          <button
            className="button button-secondary"
            disabled={disabled}
            onClick={onClose}
            type="button"
          >
            Cancel
          </button>
          <button className="button button-primary" disabled={disabled} type="submit">
            {request.submitLabel}
          </button>
        </div>
      </form>
    </Dialog>
  );
}

function EnrollmentDialog({
  disclosure,
  onClose,
  onCopyError,
}: {
  disclosure: EnrollmentDisclosure;
  onClose: () => void;
  onCopyError: (reason: unknown) => void;
}) {
  async function copy() {
    try {
      await navigator.clipboard.writeText(disclosure.token);
    } catch (reason) {
      onCopyError(reason);
    }
  }
  return (
    <Dialog onClose={onClose} title="One-use Relay enrollment token">
      <div className="dialog-form">
        <p>
          Copy it now. It expires in {disclosure.expiresIn} seconds and is never returned by a later
          read.
        </p>
        <code className="token-disclosure">{disclosure.token}</code>
        <div className="dialog-actions">
          <button className="button button-secondary" onClick={onClose} type="button">
            Clear token
          </button>
          <button className="button button-primary" onClick={() => void copy()} type="button">
            Copy token
          </button>
        </div>
      </div>
    </Dialog>
  );
}

function Dialog({
  children,
  onClose,
  title,
}: {
  children: ReactNode;
  onClose: () => void;
  title: string;
}) {
  return (
    <div className="dialog-backdrop" role="presentation">
      <section aria-labelledby="dialog-title" aria-modal="true" className="dialog" role="dialog">
        <header>
          <h2 id="dialog-title">{title}</h2>
          <button aria-label="Close dialog" className="icon-button" onClick={onClose} type="button">
            ×
          </button>
        </header>
        {children}
      </section>
    </div>
  );
}

function Detail({ code = false, label, value }: { code?: boolean; label: string; value: string }) {
  return (
    <div>
      <dt>{label}</dt>
      <dd>{code ? <code>{value}</code> : value}</dd>
    </div>
  );
}

function StatusPill({ machine }: { machine: MachineSummary }) {
  return (
    <span className={`status-pill ${reachabilityClass(machine.reachability)}`}>
      {reachabilityLabel(machine.reachability)}
    </span>
  );
}

function LoadingCards({ label }: { label: string }) {
  return (
    <div className="loading-state" aria-live="polite">
      <span className="activity-dot" />
      {label}
    </div>
  );
}

function hostLifecycleConfirmation(
  machine: MachineSummary,
  action: "disable" | "revoke" | "re-enroll",
  execute: (machine: MachineSummary, action: "disable" | "revoke" | "re-enroll") => Promise<void>,
): Confirmation {
  const labels = {
    disable: ["Disable Host", "Disable Host"],
    revoke: ["Revoke Relay access", "Revoke Relay"],
    "re-enroll": ["Re-enroll Relay", "Start re-enrollment"],
  } as const;
  return {
    confirmLabel: labels[action][1],
    danger: action !== "re-enroll",
    description: `${labels[action][0]} for ${machine.alias}? Current OwlMux access will close. Target tmux and its processes will not be stopped.`,
    onConfirm: () => execute(machine, action),
    title: labels[action][0],
  };
}

function normalizeRoute(path: string): string {
  if (path === "/workspaces") return path;
  if (path === "/hosts" || path === "/hosts/new") return path;
  if (path.startsWith("/hosts/") && path.length > "/hosts/".length) return path;
  if (["/ssh-credentials", "/audit", "/deployment"].includes(path)) return path;
  return "/workspaces";
}

function shortId(value: string): string {
  return value.length <= 12 ? value : `${value.slice(0, 8)}…${value.slice(-4)}`;
}

function reachabilityClass(value: MachineSummary["reachability"]): string {
  switch (value) {
    case "reachable":
      return "is-good";
    case "connecting":
      return "is-warning";
    case "temporarily_unavailable":
    case "owner_unreachable":
      return "is-danger";
    case "unknown":
      return "is-neutral";
  }
}

function reachabilityLabel(value: MachineSummary["reachability"]): string {
  switch (value) {
    case "reachable":
      return "Reachable";
    case "connecting":
      return "Connecting";
    case "temporarily_unavailable":
      return "Unavailable";
    case "owner_unreachable":
      return "Owner unreachable";
    case "unknown":
      return "Not connected";
  }
}

function hostLifecycleLabel(value: MachineSummary["lifecycle"]): string {
  switch (value) {
    case "pending":
      return "Pending Relay enrollment";
    case "verifying":
      return "Verifying target access";
    case "active":
      return "Active Host";
    case "disabled":
      return "Disabled Host";
  }
}
