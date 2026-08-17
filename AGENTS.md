# OwlMux repository guide

## Language

- Communicate with the user in Chinese.
- Code, public APIs, file names, commit messages, and documentation are English.
- Do not use emoji in responses, code, comments, documentation, commits, or release notes.

## Product boundary

OwlMux is a self-hosted terminal roaming gateway built on SSH and target-owned tmux.

The non-negotiable invariant is:

```text
Server node, Relay, PostgreSQL, browser, or network failure
    => OwlMux attachment or reachability loss only
    != target tmux session loss
    != target process cleanup
```

- Target tmux alone owns sessions, windows, panes, PTYs, scrollback, layouts, and child-process lifetime.
- Target sshd owns SSH host identity and Unix-account authentication.
- `owlmux-server` is one symmetric modular-monolith binary. One Deployment runs one or more Server-node processes from the exact same Server build and Deployment-critical configuration; every node may accept public Web/API, Browser WebSocket, and Relay ingress and may own Machine-affine SSH/tmux/attachment work.
- Each Server node is one Tokio multi-threaded process and may use its assigned host cores. Deployment capacity is not limited to one process or host; horizontal scale repeats the same binary rather than splitting Gateway/Worker roles.
- `owlmux-relay` is a target-side outbound reverse-connection client. It forwards bounded SSH streams only to enrolled loopback sshd and never starts a shell, creates a PTY, invokes tmux, manages a process, or modifies target accounts, sshd configuration, `authorized_keys`, `AuthorizedKeysCommand`, or another authorization store.
- One Deployment has one identity, one public origin, one private PostgreSQL database, one `OWLMUX_API_KEY`, one `OWLMUX_SSH_KEY_ENCRYPTION_KEY`, and, in clustered mode, one distinct `OWLMUX_CLUSTER_KEY` plus internal TLS trust. All nodes share the same Deployment-critical configuration.
- The public load balancer places each new Relay connection using ordinary connection-level policy. The accepting/authenticating Server incarnation is the only permitted owner claimant; Relay enrollment, tunnel, SSH/tmux, projection, writer, and queues stay local. OwlMux has no placement hash, node-ranking policy, balance guarantee, automatic/manual rebalance, migration API, weight, or bucket.
- Browser and Machine-affine API ingress may use at most one internal owner WSS hop. After WSS establishment the destination sends a fresh one-use challenge and ingress returns one cluster-HMAC response with verified context. One-shot API control uses the same typed WSS request/result/close mode, not internal HTTPS. Relay/enrollment and raw external credentials never cross this hop.
- Node lease fencing uses Linux `CLOCK_BOOTTIME`, one Deployment-wide conservative safety margin `S`, and direct pre-I/O checks. For lease TTL `L`, a successful response maps to `local_hard_deadline = b0 + L - S` from the pre-request clock sample. `S` covers the supported PostgreSQL forward adjustment plus bounded local clock-read, scheduling, dispatch, and fence overhead. Startup validates only clock availability and `0 < S < L`. Operators must keep the platform within that margin and must not resume/clone/live-migrate the same process snapshot; a hard-fenced incarnation never revives.
- Node registration/renewal, enrollment token acceptance, Relay activation, Machine owner claim, and configuration transition lock the single `DEPLOYMENT` row first and recheck exact epoch/proof/build/protocol before more specific rows. An old-config transaction cannot commit new membership, durable Relay trust, or owner authority after a configuration transition.
- Machine `route_revision` fences Relay/owner/internal paths. Independent `credential_revision` changes only which generated key a new OpenSSH child pins; rebind leaves current owner and existing authenticated child/Attachment valid. A Relay ID or Ed25519 public key may appear in at most one active binding. Active re-enrollment fences the owner, invalidates Relay trust, increments route revision, and returns the same fixed-scope Machine to tokenless `Pending` before explicit new-token issuance.
- PostgreSQL HA/failover/backup/restore is operator-owned. OwlMux assumes one configured endpoint exposes a linearizable single-writer non-rollback history and preserves acknowledged commits. It does not discover/promote/fence replicas or repair rollback; lease, revocation, enrollment, epoch, and credential guarantees are unsupported across a rolled-back history.
- Node join serves only later public connections. Drain/failure closes OwlMux connections; a new Relay ingress may claim a higher epoch only after old owner release/lease expiry. A valid unreachable owner yields `owner_unreachable`; the operator fences/stops/isolates that node, waits for lease expiry, and retries. No live state is transferred or replayed.
- Owner relinquish always closes the local dispatch barrier, rejects new writes, fences routes/children/writers/queues/results, and only then CAS-releases the exact owner. Each node uses one small bounded PostgreSQL pool for lease/config/fencing work and one ordinary bounded pool for enrollment and public work.
- PostgreSQL is the only durable product authority and stores only low-churn expiring coordination in addition to product state. Node-local admission/hints and owner-local live state are disposable. Terminal bytes never enter PostgreSQL, audit, a message queue, Redis, or another broker.
- Do not add a Gateway/Worker split, scheduler/rebalance service, virtual-bucket coordinator, Redis, message queue, terminal broker, distributed writer lock, PostgreSQL HA orchestrator, or live-state migration layer.
- Separate Deployments remain mutually independent trust domains. Each has its own origin, identity, all secrets, PostgreSQL, membership/owner registry, credentials, Machines, Relays, and live state; OwlMux provides no cross-Deployment global view, routing, migration, failover, or continuity.

The current repository implements and qualifies the pre-release single-node and clustered profiles: Deployment/API-key and credential custody, complete Machine/Relay lifecycle control, active-Machine credential rebind for future SSH children, Relay enrollment/tunnel/actual ownership, read-only and writable SSH/tmux projection, owner-local Browser writer coordination, symmetric clustered remote-owner/internal-WSS routing, safe audit/metrics, repeatable recovery evidence, the documented Linux/tmux/login-shell matrix, and the production Server image. Publication remains tag-driven and CI-owned, and no production-supported version has been released; do not extend the qualification claim beyond the documented profiles.

## Deployment access

- One explicit `OWLMUX_API_KEY` with prefix `owlmux_sk_v1_` and the canonical unpadded base64url encoding of exactly 32 operator-generated random bytes grants complete access to every Web/API resource and attachment in the deployment. Deployment is the sole human/API trust boundary; do not subdivide it into identities, delegated grants, per-resource authorization, alternate login methods, or persistent Browser authentication state.
- Browser keeps the API key only in current page memory and sends it as Bearer on every protected HTTP request. It MUST NOT persist it in cookies, Web Storage, IndexedDB, Cache Storage, service workers, URLs, logs, analytics, or serialized state. Reload/logout requires re-entry.
- Attachment WebSocket upgrades use exact Origin and one bounded `auth.api_key` first frame under a short deadline. Before it succeeds, allocate no Machine query, owner resolution, internal owner-WSS, route, SSH/tmux, projection, writer pointer, or Attachment state. After success, clear raw key bytes. A remote owner receives only a short-lived cluster-authenticated context bound to exact node/config/Machine epochs; do not forward or retain an API-key copy per terminal frame. Never transport the key through URL, query, cookie, or WebSocket subprotocol.
- API-key rotation replaces the sole Deployment value through a controlled all-node drain/stop, waits for prior leases to expire, increments the configuration epoch/proof, and restarts coherent nodes. The old key, nodes, and connections immediately cease to work; a still-open page clears the old candidate on fresh authentication failure. Ordinary unchanged-key node restart may reuse only an existing page-memory candidate. Rotation has no grace key, online mutation, per-node transition, or durable authentication state.
- Clustered mode uses a distinct canonical 32-byte `OWLMUX_CLUSTER_KEY` plus TLS for Server-to-Server authentication. It never substitutes for API, enrollment, Relay, SSH, or encryption credentials. Raw API keys, enrollment tokens, Relay proof material, SSH keys, and encryption keys never cross internal owner WSS; Relay/enrollment never uses that path.

## SSH private keys

- The Deployment owns reusable generated Ed25519 SSH credentials. Initialization generates one default. Credential creation accepts only a bounded name; Server generates the key in memory, derives all public metadata, encrypts before persistence, and never accepts a private-key upload, imported key, passphrase, or algorithm selector. Rename changes metadata only.
- Reset creates a new generated Ed25519 credential and makes it the Deployment default. Rotation creates an ordinary replacement. Neither action automatically rebinds Machines, retires old credentials, installs public keys, or removes old target authorization. Active-Machine rebind is an explicit no-preflight control-plane switch for future SSH children and may be switched back to a previous still-active credential; it does not revoke an already authenticated child.
- Each Machine binds one active Deployment credential. Private keys are never exported, downloadable, sent to Relay, or accepted from a caller.
- Use one built-in encryption path only: `OWLMUX_SSH_KEY_ENCRYPTION_KEY` is canonical unpadded base64url for exactly 32 random bytes and directly keys fixed versioned XChaCha20-Poly1305 envelopes with fresh nonces and fixed v1 domain/Deployment UUID/credential UUID associated data. The envelope leading byte is the sole version authority; open failure is a bounded diagnostic and never mutates durable lifecycle/default/binding state. Do not add a KDF, provider interface, custom backend seam, KMS/HSM integration, plugin, remote protocol, multiple encryption keys, online rewrap/encryption-key rotation, or encryption-key-management UI.
- Non-recoverable enrollment values are stored as digests, not encrypted. Browser authentication state is never persisted.
- Every Server node has a non-shared node-local private SSH runtime root containing one exclusive startup-instance directory and one exclusive child-instance directory per owner-local OpenSSH child, preferably on tmpfs. Materialize the decrypted identity only as an exclusive `0600` file in that child directory; keep it through spawn/TCP/banner/host verification; unlink only after the first valid authenticated remote-protocol record proves OpenSSH loaded it. Child cleanup never removes siblings; each node scavenges only its own root under no-link/type/owner/mode checks and fails closed on ambiguity. Accept bounded crash residue; do not add shared/network runtime roots, cross-node cleanup, ambient ssh-agent, `/proc` fd tricks, patched OpenSSH, or persistent identity files.
- Target administrators exclusively install, rotate, and remove public keys through external operational tooling; OwlMux and Relay never mutate or reconcile target authorization stores.

## Source map

- `spec/` — normative target product and architecture;
- `docs/` — public VitePress guidance; clearly separate current from planned;
- `crates/owlmux-server/` — symmetric public/internal Server-node runtime;
- `crates/owlmux-relay/` — target-side Relay runtime;
- `apps/web/` — React/Vite Web source;
- `dev/` — local PostgreSQL infrastructure;
- `.github/workflows/` — CI, docs deployment, image publication, and release.

Do not create extra crates until implemented sharing proves a concrete ownership boundary. In particular, do not pre-create core, protocol, domain, storage, cluster, scheduler, gateway, worker, SDK, CLI, or key-provider abstractions.

## Documentation workflow

`spec/` is normative and may lead implementation. Public `docs/` may describe accepted direction only with an explicit target-design/foundation label.

When changing product boundaries, repository structure, commands, CI, deployment, or planned component responsibilities, review and update:

- `spec/*`;
- `docs/*`;
- `README.md`;
- `AGENTS.md`;
- `Cargo.toml` and crate manifests;
- `package.json`, workspace config, and lockfiles;
- `Makefile`, Dockerfile, Compose, and workflows.

Format Markdown, including this guide and `spec/`, with sentence-case headings and without hard-wrapping prose or ordinary list items. Do not introduce formatter-generated hanging indentation or early line breaks; keep indentation only where Markdown structure, code, tables, or Mermaid requires it. Use Mermaid for diagrams. Do not add mdBook or duplicate public docs systems.

## Development workflow

Install locked dependencies:

```bash
make install
```

After code or configuration changes, run:

```bash
make check
make test
make build
```

For the Server image:

```bash
make docker-build
```

For the PostgreSQL development service:

```bash
make dev-up
make dev-status
make dev-down
```

## Coding conventions

- Keep the foundation minimal and honest; do not add fake product surfaces.
- Work bottom-up from durable deployment/Machine constraints through Relay/SSH, tmux projection, public protocol, UI, and real end-to-end tests.
- Treat OpenSSH as the initial constrained SSH client. Use a dedicated Server-owned config, explicit host-key inputs, `IdentitiesOnly`, and no ambient agent for target authentication. Browser input never chooses SSH options, destinations, identities, remote commands, or tmux syntax. Enrollment verification uses one fixed no-tmux `VerifySshAccess` that emits one constant bounded marker and exits zero; probe/create/attach use their own closed typed entry operations. All share one reviewed shell-literal renderer across a qualified login-shell matrix, never a generic command or target wrapper.
- Treat tmux 3.2a as the minimum target compatibility baseline, not as a promise that every newer version works. Before a writable workspace, run bounded capability probes and reject a small release-maintained known-bad denylist. Use representative CI coverage across the minimum, maintained distribution packages, one current release, qualified login shells, real target behavior, and Browser E2E rather than a runtime package allowlist or Cartesian target manifest. Detect and explain missing or incompatible target tmux, but never invoke a package manager or install, upgrade, downgrade, patch, configure, or repair it.
- The initial Relay protocol accepts one exact version with no negotiation or compatibility manifest. Decide compatibility policy only when a second protocol version exists; Server nodes still require one exact build/configuration.
- Keep tmux operations closed and typed; pane input is the only literal byte path. The initial operations are session list/select/create, return to chooser, select an observed window/pane, pane input, Browser resize, projection refresh, and detach. Exactly one OwlMux Browser attachment per Machine connection epoch/socket incarnation is the owner-local current writer pointer. Observer clients are read-only plus `ignore-size`; takeover orders old/new client flags, pointer replacement, writer resize, authoritative layout, and fresh capture. Native tmux clients remain outside this coordination.
- Do not persist terminal input, output, scrollback, tmux projection, Browser writer state, or attachment generations.
- Do not retry ambiguous terminal input or mutating tmux operations.
- API-key failure, node drain/fence, owner change, Machine disablement, Relay revocation, or database failure closes OwlMux access only; it never cleans up target processes. Credential rebind affects future SSH authentication and is not immediate revocation.
- Relay enrollment uses a token-only bounded first frame. Successful bounded digest resolution atomically consumes the token and creates one deadline-bounded durable `Verifying` attempt before setup. Relay persists its candidate ID/key before enrollment and the returned Deployment/Machine IDs before setup. Setup, fresh challenge, and verified proof stay only in bounded memory on that same live accepting connection; there is no persisted coordinator/challenge/proof or resume on another connection. Final activation locks `DEPLOYMENT` then the attempt/Machine/credential/executing node and, under partial unique constraints, rechecks post-lock PostgreSQL time, exact Serving lease/config/build/protocol, and that no other active binding uses the Relay ID/key before insert. Raw token material never crosses a Server hop. Failure returns to tokenless `Pending` and requires explicit issuance.
- A public or internal capability is not implemented until its reviewed versioned schemas, error/status mappings, and WebSocket close codes are committed as artifacts consumed by Browser, Server, and tests as applicable.
- CI is the validation authority. Release workflows publish CI-qualified commits and must not duplicate lint or tests.
- Use function-style tests and explicit, bounded failure semantics.

## AnyCap

AnyCap is available for current web research and multimodal work. Before use, read the installed `anycap-cli` skill and verify the CLI with `anycap status`; report capability failures with their request or trace identifiers.
