# Implementation technology selections

This document records only the current technology selections. The concern specifications own product behavior, security, consistency, and failure semantics; a library choice cannot weaken them. Exact patch versions belong to lockfiles and release artifacts.

## 1. Selection register

| ID     | Concern                             | Current selection                                                                                                                                                                                                          | Requirement owner                                                                                          |
| ------ | ----------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------- |
| TS-001 | Runtime shape                       | One symmetric Rust Server modular-monolith binary, deployed as one or more nodes per Deployment, and one small Rust Relay binary                                                                                           | [02](02-domain-and-component-boundaries.md)                                                                |
| TS-002 | Server async/Web runtime            | Stable Rust 2024, Tokio multi-thread runtime, Axum, Rustls, and `tracing`                                                                                                                                                  | [02](02-domain-and-component-boundaries.md), [07](07-http-websocket-and-product-ui.md)                     |
| TS-003 | Target SSH client and remote entry  | System OpenSSH as a constrained owner-node-local subprocess with closed typed remote-entry rendering and no target wrapper                                                                                                 | [04](04-ssh-tmux-attachment-and-roaming.md)                                                                |
| TS-004 | tmux integration and compatibility  | Native owner-side tmux control-mode parser and closed command renderer; tmux 3.2a minimum, bounded runtime capability probe, known-bad denylist, and representative CI evidence                                            | [04](04-ssh-tmux-attachment-and-roaming.md)                                                                |
| TS-005 | Web application                     | React, TypeScript, Vite, and xterm.js under `apps/web`                                                                                                                                                                     | [07](07-http-websocket-and-product-ui.md)                                                                  |
| TS-006 | Product and cluster store           | One private PostgreSQL with SQLx migrations/transactions for durable product state plus low-churn node leases and Machine-owner epochs                                                                                     | [06](06-storage-consistency-and-private-key-encryption.md)                                                 |
| TS-007 | Ownership and fencing               | Public-LB connection placement, ingress-local Relay owner claim, PostgreSQL database-time node leases/serialized actual owners, and Linux `CLOCK_BOOTTIME` self-fencing; no ranking/scheduler/rebalance/Redis/queue        | [06](06-storage-consistency-and-private-key-encryption.md), [08](08-operations-security-and-resilience.md) |
| TS-008 | Owner-local disposable coordination | Bounded process memory for Relay/SSH/tmux live state, Browser writer coordination, admission, hints, queues, and advisory reachability                                                                                     | [06](06-storage-consistency-and-private-key-encryption.md)                                                 |
| TS-009 | Internal node transport             | Direct Rustls WSS endpoint for at most one Browser/Machine-affine API owner hop, distinct 32-byte cluster key, domain-separated HMAC-SHA-256 auth/config proofs, and no Relay/enrollment proxy or internal HTTPS auth mode | [05](05-deployment-access-and-authentication.md), [08](08-operations-security-and-resilience.md)           |
| TS-010 | SSH private-key encryption          | One built-in XChaCha20-Poly1305 envelope keyed by a fixed 32-byte environment key; no provider abstraction                                                                                                                 | [06](06-storage-consistency-and-private-key-encryption.md)                                                 |
| TS-011 | Relay transport                     | Outbound WebSocket over TLS to one Deployment origin, Ed25519 Machine authentication, and bounded owner-local multiplexed streams                                                                                          | [03](03-relay-enrollment-and-transport.md)                                                                 |
| TS-012 | Browser attachment protocol         | Exact-Origin WebSocket, one bounded first API-key auth frame at ingress, then versioned bounded JSON with Machine/Attachment epochs and base64 terminal bytes                                                              | [05](05-deployment-access-and-authentication.md), [07](07-http-websocket-and-product-ui.md)                |
| TS-013 | Documentation                       | One VitePress site deployed as Cloudflare Workers static assets                                                                                                                                                            | Public docs and repository boundary                                                                        |
| TS-014 | Server packaging                    | One multi-stage Debian-based unprivileged Server image with OpenSSH client; the same image runs every node                                                                                                                 | [08](08-operations-security-and-resilience.md)                                                             |
| TS-015 | Validation and release              | CI is source-validation authority and validates standalone source packages; release workflows publish CI-qualified archives, images, and crates.io packages without repeating tests                                        | Repository delivery boundary                                                                               |

## 2. Repository shape

```text
apps/
  web/                  # React/Vite application source
crates/
  owlmux-server/        # symmetric public/internal node, ingress-local Relay owner, SSH/tmux client
  owlmux-relay/         # target-side enrollment and outbound stream bridge
dev/
  compose.yml           # local PostgreSQL infrastructure
docs/                   # one VitePress public documentation site
spec/                   # normative architecture
```

Exactly two runtime crates are justified because Server and Relay are independent deployed processes with opposite network placement and trust boundaries. Horizontal Server scale repeats the same `owlmux-server` artifact; it does not justify Gateway, Worker, scheduler, cluster-protocol, or coordination crates.

Do not pre-create `core`, `domain`, `protocol`, `storage`, `cluster`, `scheduler`, `gateway`, `worker`, `SDK`, `CLI`, or key provider crates. Shared code is extracted only after real use in both runtime crates proves a stable ownership boundary. A native client requires a separate product decision.

## 3. Rust baseline

The workspace uses:

- stable Rust through `rust-toolchain.toml`;
- Rust edition 2024 and Cargo resolver 3;
- workspace-wide `unsafe_code = "forbid"`;
- Tokio's multi-thread scheduler for asynchronous public/internal network, process, signal, timer, and bounded queue ownership across the host's configured CPU budget;
- Axum for public HTTP, internal WSS, WebSocket, health, and Web asset delivery;
- Rustls for Relay/public where Server terminates TLS and for mandatory clustered-mode internal TLS;
- `tracing` for structured, redacted diagnostics;
- Rustfmt and Clippy `all` plus `pedantic` in CI.

One Server node is one Tokio multi-threaded process and may use the available cores assigned to it; the Deployment is not limited to one process or one host. Operators add equivalent nodes for horizontal scale. Domain and application boundaries remain ordinary modules inside `owlmux-server`; framework types stop at adapters. This selection does not justify a generic dependency-injection framework, actor framework, dynamic plugin system, or a second binary role.

## 4. OpenSSH subprocess

OwlMux uses the system OpenSSH client for mature SSH protocol behavior, host-key verification, identities, and host certificates. Initial target connectivity is exclusively through the enrolled Relay's fixed loopback sshd endpoint.

The selection is valid only with the constrained boundary in [04](04-ssh-tmux-attachment-and-roaming.md): dedicated Server configuration, explicit host inputs and host-key alias, exact Machine-selected Deployment credential, `IdentitiesOnly`, no ambient agent for target authentication, disabled forwarding/prompts/PTY, cleaned environment, no local shell, bounded diagnostics, and local-only teardown. Relay streams reach OpenSSH through a bounded Server-owned fixed bridge rather than ambient SSH configuration or a browser-selected `ProxyCommand`.

OpenSSH ordinary remote execution is a shell-interpreted command string, not a remote argv API. The entry boundary has only typed `VerifySshAccess`, `Probe`, `CreateSession`, and `AttachSession` variants, fixed command structure, and one qualified shell-literal renderer for complete arguments. Enrollment verification runs one fixed non-mutating no-tmux command that emits one constant bounded marker and exits zero; the other variants use an operator-configured validated absolute tmux path and `tmux -C`. Every variant uses `ssh -T` and `RequestTTY=no`, rejects banner/rc/stdout pollution, and never exposes a raw remote command. OwlMux installs no target wrapper and uses no Browser SSH stack, ambient operator SSH configuration, or ambient agent authority.

## 5. tmux control integration

Server implements the supported tmux control-mode parser, projection adapter, and closed operation renderer directly. The minimum target baseline is tmux 3.2a because it retains the Ubuntu 22.04 LTS and EL9 package floor while providing the 3.2 control-mode generation with bounded/fair output, `pause-after`/resume, format subscriptions, independent client flags, and detach notification on which OwlMux qualification relies. tmux 3.1c and older are not supported; accepting them would preserve older distribution generations at the cost of a second, weaker protocol profile. Requiring 3.3a or later would exclude maintained 3.2a targets without providing a necessary architecture capability.

The minimum is not a promise that every later tmux build behaves correctly. Before opening a writable workspace, Server parses the configured client and running server versions, rejects versions below 3.2a or on a small release-maintained known-bad denylist, and runs bounded capability probes for the control-mode behavior OwlMux actually uses. Package provenance is neither inferable nor required.

CI keeps representative evidence across tmux 3.2a, selected maintained distribution packages, one current upstream release, qualified login shells, real control transcripts, backpressure, remote-entry escaping, target-process survival, and Relay-backed Browser E2E. This is evidence for the supported behavior, not a Cartesian product or a runtime allowlist. A newly observed incompatibility becomes a focused denylist entry with a regression fixture.

OwlMux treats tmux as target-administrator-owned software. Server and Relay may detect its absence, version, path, socket access, or missing capabilities and return bounded guidance, but they never invoke a target package manager or install, upgrade, downgrade, patch, configure, or repair tmux.

The architecture rejects:

- normal terminal screen scraping;
- a tmux server implementation;
- a generic terminal-multiplexer abstraction;
- a custom Server-owned PTY/session runtime;
- durable target shadow state;
- a server-side terminal emulator when no measured protocol need exists.

xterm.js renders bounded current tmux cell snapshots and subsequent pane bytes in Browser. It is not authoritative for target state or scrollback, and the selected integration does not claim complete terminal-checkpoint equivalence or byte-exact disconnected replay.

## 6. Web application

The Web baseline uses the locked repository toolchain:

- Node.js 24;
- pnpm workspace and pinned pnpm version;
- React 19;
- TypeScript;
- Vite;
- Vitest;
- ESLint and Prettier;
- xterm.js for terminal pane rendering.

Source lives only under `apps/web`. Production assets are built once and embedded or served by `owlmux-server`; generated assets are not a second source tree.

No framework state store, data-fetching library, component system, or router is an architectural requirement without a concrete product need. Browser security and state boundaries remain those in [07](07-http-websocket-and-product-ui.md) regardless of libraries.

## 7. PostgreSQL, SQLx, leases, and ownership

PostgreSQL is the only durable database and the only cluster coordination store. SQLx is selected for explicit migrations, checked SQL, short transactions, bounded pools, Rustls-compatible connectivity, and database-time lease operations.

The product schema is closed to Deployment, SSH credential, Machine, Relay binding/enrollment, and audit state. The expiring coordination schema adds only Server-node incarnation/lease/configuration rows and one retained Machine-owner/connection-epoch row per Machine. It contains no terminal payload, persistent writer state, projection, queue, socket resume data, or per-Relay heartbeat.

The Deployment API key, cluster key, and SSH encryption key remain runtime configuration. The accepting node verifies the API key directly on every protected HTTP request and first Browser-attachment WebSocket frame. The cluster key is used only for fixed domain-separated HMAC configuration proofs and internal authentication transcripts; it is never stored in PostgreSQL.

Implementation MUST use:

- append-only reviewed migrations;
- database constraints for durable shape invariants;
- explicit row-lock/compare-and-set transaction boundaries matching [06](06-storage-consistency-and-private-key-encryption.md), including one exclusive lock-`DEPLOYMENT`-first order shared by node registration, renewal, enrollment token acceptance, Relay activation, Machine owner claim, and configuration transition;
- owner relinquish that closes the local dispatch barrier, rejects writes, and fences routes/children/writers/queues/results before exact owner CAS release, with the old epoch kept fenced through rollback or ambiguity;
- PostgreSQL time for node lease creation, renewal, and actual-owner validity, never host wall-clock comparison;
- Linux `CLOCK_BOOTTIME` through a safe Rust wrapper, pre-request sampling, `local_hard_deadline = b0 + L - S`, and direct pre-I/O checks from [06](06-storage-consistency-and-private-key-encryption.md#33-server-node-leases);
- exact incarnation/Server-build/configuration generation and Machine owner/connection epoch predicates;
- bounded queries, transactions, retries, timeouts, and result cardinality;
- one small bounded pool for node lease/config/fencing work and one ordinary bounded pool for enrollment and public work;
- typed conversion that prevents SQL rows from becoming domain or public DTOs.

There is no OwlMux placement function. The ordinary public load balancer selects the accepting node for each new Relay connection, and that exact incarnation alone may claim itself in one PostgreSQL transaction that increments the connection epoch. A valid old owner causes bounded duplicate/recovering rejection until safe release or lease expiry. Node join affects only later load-balancer decisions; no automatic/manual rebalance, migration API, weight, bucket, or even-distribution guarantee exists.

The initial Server platform uses Linux `CLOCK_BOOTTIME`, which continues through host suspend, rather than assuming Rust `Instant`, Tokio timers, or `CLOCK_MONOTONIC` include suspend. Startup checks clock availability and `0 < S < L` only. The single conservative margin `S` covers the supported PostgreSQL forward adjustment and bounded local clock-read, scheduling, dispatch, and fence overhead. The operator keeps the platform within that documented margin and never resumes, clones, or live-migrates the same process snapshot; a fresh process incarnation is required. PostgreSQL HA/failover/backup/restore is outside OwlMux and the endpoint must expose one linearizable single-writer non-rollback history preserving acknowledged commits. The initial selection has no virtual buckets, rebalance coordinator, consistent-hash ring, advisory-lock session owner, Redis, etcd, message queue, or database polling per terminal frame. `LISTEN/NOTIFY` MAY accelerate cache invalidation or drain wakeups but cannot grant or preserve authority.

## 8. Node-local and owner-local coordination

Bounded memory in every Server node is selected for its admission token buckets, source/concurrency gates, negative/revocation hints, public/internal owner-WSS connection state, and owner-dial budgets.

For each Machine it currently owns, the same process additionally holds the Relay tunnel/router, OpenSSH children, tmux parsers/clients, projections, one current Browser writer attachment pointer, ordered dispatch state, reachability, and bounded queues. All Browser attachments for that Machine route to that owner, so no distributed writer-lock implementation is needed.

Every collection has explicit cardinality, payload, and expiry bounds where applicable. A hint may reject or require PostgreSQL refresh but cannot authorize. Node restart begins with empty local coordination; owner change increments Machine connection epoch and begins with no writer holder, projection, or resumed stream. This is a deliberate reset of DoS defense and Browser write ordering, not a loss of credential strength, durable authority, or target state.

Live state remains in exactly one fenced owner process and is never transferred. A node join handles only new connections the public load balancer sends to it. A node drain/failure closes owners so Relays/Browsers reconnect and reconstruct from target tmux; there is no rebalance. Separate Deployments remain available for additional trust isolation or external sharding, but are not the only horizontal-capacity mechanism.

## 9. Internal cluster transport and authentication

Clustered mode uses one Rustls-protected internal Axum WSS listener per Server node. A connection carries either one already externally authenticated Browser stream or one bounded typed Machine-affine API request/result followed by close. Relay enrollment and tunnel connections never use the internal listener. The local-owner path calls the same application service without loopback serialization.

`OWLMUX_CLUSTER_KEY` is canonical unpadded base64url for exactly 32 operator-generated random bytes. HMAC-SHA-256 with separate fixed domain labels is selected for:

- the cluster configuration proof over the canonical inputs defined by [06](06-storage-consistency-and-private-key-encryption.md#31-deployment-identity-and-configuration-epoch);
- a fresh WSS request/stream challenge-response bound to a destination-generated random challenge, source nonce, exact source/destination incarnations, config epoch, connection class, Machine route revision/connection epoch, and a destination-local `CLOCK_BOOTTIME` first-auth deadline.

The implementation has no generic signer/provider interface, alternate algorithm negotiation, or reuse of API/Relay/SSH/encryption credentials. Internal TLS/WSS remains mandatory; HMAC does not provide a plaintext fallback. TLS deployment may use direct certificates under a private Deployment CA or a release-qualified mutually authenticated mesh boundary.

After WSS establishment, the destination sends the first challenge and ingress returns one HMAC response; no sender wall time or cross-node monotonic value is compared and no reusable bearer assertion exists. One reviewed versioned bounded framing artifact covers Browser streaming and typed one-shot API request/result/close. It propagates backpressure, has fixed per-peer/per-node/Machine connection and byte limits, permits at most one Server hop, and retains no durable buffer. Raw API keys and all Relay/enrollment/SSH/encryption credentials are removed before routing. Once semantic live bytes may have been accepted, disconnect closes both ends and never triggers transparent reconnect/replay.

This WSS-only selection avoids two internal authentication modes and is deliberately simpler than typed HTTPS plus WebSocket, gRPC, QUIC, a service-mesh API dependency, a custom multiplexed node fabric, or a message broker. Measurement may later justify connection pooling or multiplexing without changing owner/fencing/no-replay semantics.

## 10. SSH private-key handling

Every Server node has the same local SSH key module inside `owlmux-server`. It generates Ed25519 credentials in memory, derives the public key and SHA-256 fingerprint, and encrypts/decrypts the fixed v1 envelope. It accepts no private-key upload, imported key, passphrase, or algorithm selector. The module is internal, not an object-safe provider interface, plugin, separate crate, remote service, or generic encryption API.

`OWLMUX_SSH_KEY_ENCRYPTION_KEY` decodes from canonical unpadded base64url to exactly 32 bytes and directly keys XChaCha20-Poly1305. Envelope v1 starts with `0x01`, one fresh 24-byte random nonce, and ciphertext/tag; that leading byte is the sole version authority. Associated data is the fixed `owlmux:ssh-private-key:v1\0` domain bytes followed by the 16-byte Deployment UUID and 16-byte credential UUID. Envelope-open failure remains an operation diagnostic and does not mutate credential lifecycle, default selection, or Machine binding. There is no KDF, KMS/HSM integration, multiple-encryption-key fallback, online rewrap/rotation, encryption-key UI, or custom provider contract.

All credentials use the same immutable Ed25519 persistence form. Reset generates a new credential and changes only the Deployment default. Rotation creates a replacement; Machine rebind and target-administered public-key installation/removal remain separate explicit operations. Active-Machine rebind is a no-preflight control-plane switch for future SSH children and may explicitly return to a previous still-active credential; it does not revoke an already authenticated child.

Each Server node's non-shared node-local private runtime root holds one exclusively created startup-instance directory and one exclusive child-instance directory per owner-local OpenSSH child, preferably on tmpfs. The owner writes the decrypted identity to an exclusive `0600` file in that child directory, keeps the pathname through spawn/TCP/banner/host verification, and unlinks only after the first valid authenticated remote-protocol record proves OpenSSH loaded the key. Child cleanup cannot remove siblings; pre-readiness scavenging removes only fully owner/type/mode/no-link-validated OwlMux startup trees from that node's own root and fails closed on ambiguity. A crash may leave bounded plaintext until mount/container teardown or next startup. Key handoff uses no ambient agent, `/proc` fd path, patched OpenSSH, or persistent identity file.

## 11. Relay transport

Relay has no target-account or sshd-authorization mutation adapter; public-key installation, rotation, and removal remain external target-administrator operations. Relay uses:

- outbound WebSocket over TLS on TCP 443 to one Deployment origin, with no Server-node discovery;
- Rustls for target-side TLS;
- Ed25519 for machine transcript signatures;
- one small named/versioned frame representation;
- bounded owner-local multiplexing over one tunnel and one Machine connection epoch.

The semantic frame and limit contract is fixed by [03](03-relay-enrollment-and-transport.md). The implementation selects compact JSON or a small binary representation only when introducing the reviewed shared protocol artifact; measurement and implementation simplicity decide between them without changing those semantics.

The first implementation accepts one exact Relay protocol version and rejects every other version with bounded upgrade guidance. It has no version negotiation or compatibility manifest. Compatibility policy is deferred until a second protocol version actually exists.

Relay values remain private to their runtime modules unless a stable contract shared by Server and Relay justifies extraction.

## 12. Browser protocol

Protected HTTP uses the Deployment API key as Bearer on every request at whichever node ingress accepts it. Browser retains it only in current page memory. Because native WebSocket cannot set arbitrary Authorization, the attachment WebSocket uses one size-bounded `auth.api_key` first frame under a short deadline and allocates no Machine lookup, owner resolution, internal owner-WSS, or Attachment resources before success. URL/query/cookie/subprotocol/storage transport is forbidden.

Tagged JSON plus an opaque Machine connection epoch, a narrower attachment epoch, and base64 terminal bytes is selected after authentication for debuggability and implementation simplicity. The protocol remains strictly bounded and versioned; measurement must justify any encoding change. The implementation change for each surface commits one reviewed generated schema/error/status/close-code artifact consumed by Browser, Server, and tests before that capability counts as implemented.

No alternate binary framing or negotiation path is part of the initial selection. A future measured need would be a new protocol decision rather than dormant implementation structure.

## 13. Documentation

Public documentation uses one VitePress source tree under `docs/`, local Mermaid rendering, local search, and static Cloudflare Workers assets. The Worker name is `owlmux-docs`; canonical URL and repository links come from reviewed docs configuration.

There is no mdBook or duplicate docs site. `spec/` remains normative; `docs/` translates the architecture into operator and user guidance without becoming a second authority.

## 14. Container

One multi-stage Dockerfile:

1. installs locked pnpm dependencies and builds the Web artifact;
2. builds `owlmux-server --release --locked` with that exact artifact;
3. creates a minimal Debian runtime with CA certificates, OpenSSH client, curl, and tini;
4. runs as a fixed unprivileged `owlmux` user;
5. exposes one public HTTP port and, in clustered mode, one separately protected internal WSS-over-TLS port, and uses `/health` for process health;
6. includes OCI source, version, revision, and license labels.

The runtime image contains no Rust/Node toolchain, package manager, source tree, SSH private key, API key, cluster key/TLS private identity, baked-in private-key encryption key, or Relay private key. PostgreSQL is the only external product/coordination state service. The same image runs single-node and clustered profiles without a Gateway/Worker variant.

## 15. CI and release

CI on pull requests and `main` pushes is the source-validation authority. It installs and audits locked dependencies; checks formatting and lint; tests and builds Rust, Web, docs, and repository invariants; builds and test-compiles the standalone Server and Relay source packages; dry-runs docs deployment; verifies the qualified Relay glibc baseline; and builds and smoke-tests the Server image. A successful exact-`main` CI run is the only trigger accepted by documentation and development-image publication.

Tag creation is an operator-controlled release boundary: the operator creates one immutable version tag only after the exact `main` commit completes CI successfully. The tag-driven workflow intentionally does not query the Actions API; it trusts that pushed CI-qualified tag and performs artifact construction, source-revision metadata, checksums, checksum-safe crates.io source-package publication, GHCR upload, GitHub Release publication, and exact repository-version/tag validation. It does not repeat CI qualification or the source lint/test suite as a second authority.

Release deliverables are:

- `owlmux-server` archives for supported Server targets;
- `owlmux-relay` archives for qualified target platforms;
- `ghcr.io/owlfoundry/owlmux` Server image tags;
- `owlmux-server` and `owlmux-relay` crates.io source packages;
- checksums and release notes;
- the independently deployed documentation Worker.

Server and Relay remain application artifacts rather than reusable library APIs. The Server source package contains embedded migrations and generated protocol bindings but not the Web build, so the complete qualified Server deployment artifact remains the fixed-version image or release archive.

## 16. Change threshold

Change a current selection only when concrete implementation evidence shows that it cannot satisfy its requirement owner. The same change updates this register and all affected owning specifications directly so they always describe only the resulting current selection.

Mature and replaceable dependencies do not need a separate architecture record merely because they are dependencies. A material cross-boundary change must state its requirement owner, new selection, rationale, migration impact, and required evidence in the implementing change.

## 17. Required validation

Current technology selections remain valid only while implementation proves:

- a clean checkout builds with locked toolchains and no hidden local dependency;
- constrained OpenSSH and tmux fixtures satisfy their security and compatibility contracts;
- PostgreSQL product transactions, shared-`DEPLOYMENT`-row serialization of registration/renewal/config transition, the dedicated small lease/config/fencing pool, database-time node leases, the single `CLOCK_BOOTTIME` safety margin, fresh-incarnation no-snapshot-resume policy, ingress-local concurrent owner claims/epochs, no rebalance, empty node/owner-local coordination after restart, non-rollback database-history contract, separate-Deployment isolation, and fixed SSH private-key envelopes satisfy their owning specifications;
- Relay, Browser, and internal owner-WSS protocols remain bounded under malformed data/load; Relay/enrollment stay on their accepting node; WSS is Browser/API-only and at most one hop; internal TLS/HMAC/config/epoch checks reject stale or confused peers; no Relay, internal, or public API path can mutate or reconcile a target SSH authorization store;
- the production image runs unprivileged with the exact Web artifact and required OpenSSH runtime only;
- docs and release artifacts originate from the intended CI-qualified commit;
- no selected library introduces a second datastore, ambient credential path, terminal-state store, live-state transfer, scheduler/rebalance service, cross-Deployment coordinator, PostgreSQL HA/restore orchestrator, or target-process owner.
