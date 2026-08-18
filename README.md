# OwlMux

OwlMux is a self-hosted terminal roaming gateway built on SSH and target-owned tmux. Target tmux owns every session, pane PTY, scrollback buffer, and child process. OwlMux provides a graphical Web client and an outbound Relay so users can reconnect without moving session ownership to public Server nodes.

> **Current implementation:** The pre-release single-node and clustered product is implemented and Docker-qualified: Deployment API-key access, generated encrypted SSH credentials, complete Machine/Relay lifecycle controls, explicit active-Machine credential rebind, one-use Relay enrollment, signed Relay tunnels, accepting-ingress ownership, constrained OpenSSH, an explicit tmux session chooser, target-authoritative multi-pane xterm.js projection, the closed interactive writer surface, safe audit/metrics, and symmetric clustered routing. Browser attachments and Machine-affine invalidations may enter any coherent node and use at most one fresh challenge-authenticated internal TLS/WSS hop to the Relay-owning node. Node join, owner loss, stale endpoints, local TLS-identity/configuration rejection, exact-owner invalidation under cross-node Relay reconnect, lease-expiry recovery, cold API/configuration rotation, and target tmux survival have real acceptance evidence. Release qualification covers the documented Linux x86_64, tmux 3.2a/3.3a/3.5a/3.7b, `bash`/`dash`, single-node, clustered, Browser, dependency-audit, and production-image paths. Version `0.0.1` is the initial tag-driven, CI-published evaluation release. No version is supported for production terminal access, and no broader platform claim is implied.

The following diagram shows the implemented single-node and clustered topology.

```mermaid
flowchart LR
    browser["Browser"] --> origin["One Deployment origin"]
    relay["Target OwlMux Relay"] -->|"outbound connection"| origin
    origin --> ingress["TLS ingress"]
    ingress --> nodeA["Symmetric Server node A"]
    ingress --> nodeB["Symmetric Server node B"]
    nodeA --> postgres[("Deployment PostgreSQL")]
    nodeB --> postgres
    nodeA <-->|"Browser/API owner WSS only"| nodeB
    nodeB -->|"owner-local SSH through Relay"| relay
    relay --> sshd["Target sshd"]
    sshd --> tmux["Target-owned tmux"]
    tmux --> process["Shell or coding agent"]
```

## Target product boundary

- One self-hosted Deployment contains one public origin, one private PostgreSQL database, one Deployment API key, and one or more symmetric `owlmux-server` nodes.
- Each node is one Tokio multi-threaded process that can use its assigned host cores. Horizontal scale adds symmetric nodes from the exact same Server artifact and Deployment-critical configuration; OwlMux does not split Gateway and Worker binaries.
- The API key grants complete Deployment access; Deployment is the sole human/API trust and authorization boundary.
- Browser holds the API key only in page memory, sends Bearer on every protected HTTP request, and authenticates an attachment WebSocket with one bounded first frame. Reload requires re-entry; an ordinary unchanged-key node restart may reuse only a still-open page-memory candidate.
- PostgreSQL is the only durable product authority and stores low-churn node leases plus actual Machine-owner epochs. Terminal bytes, projections, Browser writer state, and live sockets remain outside PostgreSQL.
- The public load balancer places each new Relay connection using ordinary connection-level policy. Its accepting/authenticating Server incarnation alone may claim the Machine and keeps the Relay tunnel plus all Machine-affine state local. OwlMux promises no even distribution and has no placement hash, candidate ranking, or rebalance.
- Browser and Machine-affine API traffic may enter any node and use at most one internal owner WSS hop. Destination challenge plus cluster HMAC carries only verified context; one-shot API control uses the same WSS mode, and Relay/enrollment is never proxied.
- Node lease fencing uses Linux `CLOCK_BOOTTIME`, one conservative Deployment-wide safety margin, and direct pre-I/O deadline checks. Startup validates only clock availability and `0 < safety margin < lease TTL`; operators keep PostgreSQL forward adjustment and bounded local overhead within that margin and never resume the same process snapshot. Node drain/failure closes OwlMux connections only, and a new ingress may claim after old owner release/lease expiry. A valid unreachable owner returns `owner_unreachable` until the operator fences that node and waits for lease expiry. No live state migrates or replays.
- Clustered mode uses one distinct `OWLMUX_CLUSTER_KEY` plus internal TLS/WSS. It does not add Redis, a message queue, scheduler/rebalance service, terminal broker, or distributed writer lock.
- PostgreSQL HA/failover/backup/restore is operator-owned. OwlMux assumes one configured endpoint exposes a linearizable single-writer non-rollback history that preserves acknowledged commits; it does not repair history rollback.
- The initial Relay protocol accepts one exact version with no negotiation or compatibility manifest; policy for older versions is deferred until a second protocol version exists. Server nodes always require one exact build/configuration.
- Separate Deployments remain independent trust domains with separate origins, databases, secrets, membership, resources, and live state. OwlMux provides no cross-Deployment routing, migration, failover, or global view.
- Relay forwards SSH bytes to enrolled loopback sshd and never owns tmux, a process, or target SSH authorization stores.
- Browser, Server node, Relay, PostgreSQL, and network failure may interrupt an attachment but must not terminate target tmux.
- Deployment initialization generates one default Ed25519 key pair. The current profile supports generation, rename, default selection/reset, replacement rotation, active-Machine rebind for future SSH children, and retirement of an unreferenced non-default credential. OwlMux accepts no private-key upload or alternate SSH key algorithm. Target administrators exclusively install and remove public keys through external operational tooling; OwlMux and Relay never mutate `authorized_keys` or equivalent authorization stores.
- The target administrator installs and operates tmux. The target design uses tmux 3.2a as its minimum, performs bounded runtime capability probes, maintains a small known-bad denylist, and uses representative CI evidence rather than a package allowlist or Cartesian profile manifest; OwlMux detects incompatibility but never installs, upgrades, downgrades, configures, patches, or repairs tmux.

The normative design is under [`spec/`](spec/README.md).

## Repository layout

- `crates/owlmux-server` — symmetric Deployment, public/internal ingress, constrained SSH/tmux, API, and attachment runtime;
- `crates/owlmux-relay` — target-side enrolled reverse-connection runtime;
- `apps/web` — React control plane and interactive xterm.js workspace;
- `dev` — PostgreSQL plus opt-in loopback sshd/tmux target fixtures;
- `docs` — VitePress documentation and Cloudflare Workers configuration;
- `spec` — accepted target product and architecture specifications.

## Development

Prerequisites are stable Rust, Node.js 24, pnpm 11.20.0, and Docker with Compose v2.

```bash
make install
make check
make test
make build
```

Run the single-node Server with disposable development configuration:

```bash
make dev
```

Then open `http://127.0.0.1:8080`, enter the disposable API key from `dev/server.env`, and use the control plane. A target administrator must install the selected generated public key before Relay enrollment can prove SSH access.

Start PostgreSQL or the full target fixture when needed:

```bash
make dev-up
make dev-status
make dev-target-up
make dev-target-status
make dev-down
```

Run the isolated real PostgreSQL/Relay/OpenSSH/tmux acceptance path with versioned Node attachment-WebSocket clients and headless Chromium. It covers enrollment recovery, credential locking and active rebind, safe lifecycle presentation, audit/metrics, multi-pane projection and xterm rendering under the product CSP, continuous snapshot/live cutover, binary live output, concurrent writer claim, explicit session creation, literal input, observer geometry isolation, takeover without renderer rollback, target-observed writer/observer client flags, stale-writer rejection, authoritative resize, projection refresh, route replacement, active re-enrollment, zero-session recovery, hard fencing, reload key clearing, and target tmux survival. The matrix command repeats the complete path with Ubuntu 22.04 tmux 3.2a, Debian 12 tmux 3.3a under `dash`, Debian 13 tmux 3.5a, and checksum-pinned current upstream tmux 3.7b:

```bash
make test-e2e
make test-e2e-clustered
make test-e2e-matrix
make test-recovery
```

`make test-e2e-clustered` additionally runs two coherent Server nodes with private-CA internal TLS and proves remote attachment and invalidation routing, unreachable-owner behavior, lease recovery after owner loss, no join/restart remap, exact configuration rejection, remote Relay revocation, disabled re-enrollment, enrollment cancellation, cold API/configuration rotation, and target tmux survival. `make test-recovery` runs the complete single-node and clustered failure/recovery evidence.

Build and smoke-test the production image:

```bash
make docker-build
```

## Documentation

- [Documentation site](https://owlmux-docs.owlfoundry.org)
- [Getting started](docs/guide/getting-started.md)
- [Architecture](docs/guide/architecture.md)
- [Deployment access and credentials](docs/guide/authentication.md)
- [Relay and roaming](docs/guide/relay.md)
- [Security](docs/guide/security.md)
- [Recovery and incident response](docs/guide/recovery.md)
- [Specifications](spec/README.md)

## License

OwlMux is distributed under the BSD 3-Clause License. See [LICENSE](LICENSE).
