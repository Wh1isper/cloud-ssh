# OwlMux specification

`spec/` is the normative authority for the OwlMux product and target architecture.

OwlMux is a self-hosted terminal roaming gateway built on SSH and target-owned tmux. Its governing invariant is:

```text
Server node, Relay, PostgreSQL, browser, or network failure
    => OwlMux attachment or reachability loss only
    != target tmux session loss
    != target process cleanup
```

## Specification map

| Specification                                                                                                 | Owns                                                                                                                                      | Does not own                                                                 |
| ------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------- |
| [01 — System context and goals](01-system-context-and-goals.md)                                               | Product boundary, Deployment trust domain, clustered topology, core concepts, goals/non-goals                                             | Internal modules or wire formats                                             |
| [02 — Domain and component boundaries](02-domain-and-component-boundaries.md)                                 | Runtime responsibility, dependency rules, node/owner boundaries, ports, request paths, errors                                             | Persistence schema or protocol grammar                                       |
| [03 — Relay enrollment and transport](03-relay-enrollment-and-transport.md)                                   | Machine enrollment, Relay identity, ingress-as-owner claim, tunnel, streams, route identity/failure                                       | SSH login or tmux operations                                                 |
| [04 — SSH, tmux attachment, and roaming](04-ssh-tmux-attachment-and-roaming.md)                               | Constrained SSH, tmux control, attachment, projection, hydration, owner-local Browser writer selection, typed operations, ambiguity       | API-key transport or HTTP route design                                       |
| [05 — Deployment access and authentication](05-deployment-access-and-authentication.md)                       | One Deployment API key, HTTP/WebSocket authentication, full-access semantics, at-most-one Browser/API owner-WSS routing                   | Database mechanics or Browser message grammar                                |
| [06 — Storage, consistency, and private-key encryption](06-storage-consistency-and-private-key-encryption.md) | Durable model, node/owner registry, fencing, transactions, fixed encryption, OpenSSH key materialization, and database-history contract   | HTTP presentation, target operations, or PostgreSQL HA/restore orchestration |
| [07 — HTTP, WebSocket, and product UI](07-http-websocket-and-product-ui.md)                                   | Public routes, first-frame WebSocket auth, owner routing, messages, Browser state/UI/security                                             | SSH/tmux command semantics                                                   |
| [08 — Operations, security, and resilience](08-operations-security-and-resilience.md)                         | One-or-more-node topology, ordinary ingress balancing, internal WSS/auth, readiness, drain, threat model, limits, and operator boundaries | Domain or wire grammar                                                       |
| [09 — Implementation technology selections](09-implementation-technology-selections.md)                       | Technology choices, repository boundaries, replacement conditions                                                                         | Product scope or delivery promises                                           |

## Authority map

| Concern                                                                                      | Authority                                                                                                                                | Required consequence                                                                                                              |
| -------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------- |
| Sessions, windows, panes, layouts, PTYs, scrollback, child processes                         | Target tmux and target OS                                                                                                                | OwlMux reconstructs and never claims ownership                                                                                    |
| SSH host identity and Unix-account authentication                                            | Target sshd and OS                                                                                                                       | Every route verifies the same target boundary                                                                                     |
| Deployment identity, default credential, Machines, Relay trust, encrypted credentials, audit | One configured PostgreSQL endpoint exposing one linearizable, single-writer, non-rolled-back history that preserves acknowledged commits | Any Server node uses current committed state; PostgreSQL HA, backup, restore, and history integrity are operator responsibilities |
| Human/API access                                                                             | One configured `OWLMUX_API_KEY` shared by every Server node                                                                              | Exact key grants complete Deployment access within one trust domain                                                               |
| Private-key encryption                                                                       | One configured `OWLMUX_SSH_KEY_ENCRYPTION_KEY` shared by every Server node                                                               | Fixed local envelope only; never network authentication                                                                           |
| Server-to-Server access                                                                      | Internal WSS plus one distinct shared `OWLMUX_CLUSTER_KEY` in clustered mode                                                             | Carries at most one owner-bound Browser/API stream or request; never substitutes for public, Relay, or SSH credentials            |
| Node membership and Machine ownership                                                        | PostgreSQL node leases and Machine-owner epochs                                                                                          | At most one valid owner node/incarnation coordinates one Machine at a time                                                        |
| Relay Machine identity                                                                       | Relay ID/key appears in at most one active Machine binding                                                                               | Relay auth never substitutes for API-key, cluster, or SSH auth                                                                    |
| Machine-affine live state                                                                    | Current fenced Machine owner process                                                                                                     | Relay tunnel, SSH/tmux children, projections, writer coordination, and queues are local and disposable                            |
| Rate limits, concurrency gates, negative hints                                               | Individual Server process memory                                                                                                         | Bounded, disposable, never target authority                                                                                       |
| Browser rendering, in-memory API key, local preferences                                      | Browser memory                                                                                                                           | Never durable target truth or Server authority                                                                                    |

## Cross-cutting invariants

01. **Target ownership.** Only target tmux owns terminal/process lifetime. OwlMux liveness, revocation, overload, fencing, failover, and shutdown never imply target cleanup.
02. **Replaceable attachments.** Browser, WebSocket, internal owner hop, Relay stream, SSH, tmux control client, and projection are ephemeral. No live socket or parser state transfers between owners.
03. **One Deployment trust domain.** One Deployment has one identity, private PostgreSQL, public origin, API key, SSH encryption key, credential/Machine/Relay authority, and one or more symmetric Server nodes.
04. **Independent Deployment isolation.** Separate Deployments share no identity, database, secrets, resources, routing, migration, or continuity. External sharding remains optional rather than the only scaling mechanism.
05. **Ingress-as-owner Relay placement.** The node that accepts and authenticates a new Relay connection is the only node allowed to claim that Machine. Its Relay tunnel and all Machine-affine live state stay local. Browser and Machine-affine API ingress may authenticate on another node and use at most one internal owner WSS hop.
06. **Connection-lifetime placement without rebalance.** The public load balancer distributes new Relay connections using ordinary connection-level policy; OwlMux promises no even distribution. PostgreSQL records the actual owner. Node join never moves an owner, and OwlMux exposes no owner migration or rebalance mechanism.
07. **Fenced failure.** A Server node may own or mutate live Machine state only before its database-time lease mapped through Linux `CLOCK_BOOTTIME` and one conservative safety margin expires. Clock/lease uncertainty makes it unready; expiry irreversibly fences that incarnation before another node may claim.
08. **Full Deployment API-key authority.** One configured key controls all resources; the Deployment does not partition human authority below that boundary.
09. **Memory-only Browser key.** Protected HTTP carries Bearer every request; Browser WebSocket authenticates only with one bounded first frame; no URL/cookie/subprotocol/storage credential transport exists.
10. **One authenticated internal WSS hop.** Only already authenticated Browser connections and typed Machine-affine API requests may cross from ingress to a remote owner. After WSS establishment the destination sends a fresh one-use challenge; ingress returns a bounded cluster-HMAC transcript carrying only verified context, with no raw credential, reusable assertion, or cross-node timestamp. Relay and enrollment connections are never forwarded internally.
11. **Relay containment.** Relay forwards bounded SSH streams only to its enrolled fixed loopback sshd endpoint and never mutates target authorization.
12. **SSH end-to-end identity.** Target sshd terminates SSH and supplies host/account identity; Relay identity is only route identity.
13. **Closed control surface.** Browser selects only typed operations, never SSH/shell/tmux syntax, destinations, package actions, Relay endpoints, or Server nodes.
14. **Literal input exception.** Pane input is the sole opaque byte path, bounded and never command interpolation.
15. **No ambiguous replay.** Terminal input, mutating tmux operations, and internal owner-hop bytes are never automatically replayed after unknown outcome.
16. **Credential separation.** Deployment API, cluster, enrollment, Relay, SSH, and private-key-encryption credentials are structurally distinct.
17. **Storage exclusion.** Terminal input/output/scrollback/projection/process state never enters PostgreSQL, audit, a message queue, or telemetry.
18. **Generated Ed25519 key custody.** OwlMux generates Ed25519 Deployment SSH credentials and encrypts them with one built-in fixed XChaCha20-Poly1305 envelope; it accepts no private-key upload or alternate key algorithm. OpenSSH receives a short-lived private `0600` identity file with post-load unlink and bounded node-local crash residue.
19. **Target-administered access/software.** Target administrators install/remove public keys and operate tmux. OwlMux may detect incompatibility but never installs, upgrades, patches, configures, or repairs target software.
20. **One configuration linearization root.** Node registration, lease renewal, and configuration transition lock the same single `DEPLOYMENT` row and recheck epoch/proof under that lock.
21. **Fence before release.** Every owner relinquish first closes its local dispatch barrier, rejects new writes, and fences old-epoch routes/children/writers/queues/results; only then may it compare-and-set release the exact owner.
22. **Operator fencing for unreachable owner.** A valid owner unreachable over WSS yields `owner_unreachable`; no node bypasses or remotely evicts it. The operator fences/stops/isolates that node, waits for lease expiry, and retries.

## Core vocabulary

- **Deployment** — one independent OwlMux trust domain with one immutable deployment ID, one public origin, one private PostgreSQL, one API key, one SSH encryption key, and one or more Server nodes.
- **Server node** — one `owlmux-server` process identified authoritatively by a fresh random startup incarnation ID, with an optional operator-facing display name and an exact registered internal endpoint. Every node runs the same Server build/configuration generation and may accept public HTTP, Browser WebSocket, and Relay ingress.
- **Node lease** — a renewable PostgreSQL record authorizing one exact node incarnation to serve and own Machines until a database-time deadline.
- **Machine owner** — the one valid Server node incarnation recorded for a Machine and authorized to hold its Relay route and all Machine-affine live state.
- **Connection epoch** — a monotonically increasing Machine-owner generation that fences stale internal streams, attachments, and operation results.
- **Route revision** — a monotonically increasing Machine Relay-trust generation bound into Relay authentication, owner claim, and internal owner paths; credential rebind does not change it.
- **Credential revision** — a monotonically increasing Machine credential-selection generation read and pinned only when a new OpenSSH child is created; existing authenticated children remain on their snapshots.
- **Deployment origin** — the one public HTTPS/WebSocket origin clients use. Clients never discover or choose individual Server nodes.
- **Internal owner hop** — one bounded, backpressured, non-durable WSS connection carrying an already authenticated Browser stream or one typed Machine-affine API request from ingress to the current owner.
- **Relay** — `owlmux-relay`, a target-side outbound reverse-connection client.
- **Target** — the host whose sshd/tmux own the interactive environment.
- **Machine** — one fixed target SSH host/account/tmux-socket scope with replaceable Relay and credential bindings. The product UI calls this saved resource a **Host**; API, schema, database, owner, and protocol vocabulary remains Machine.
- **SSH credential** — one reusable Deployment-owned generated Ed25519 key pair.
- **Attachment** — one ephemeral Browser-to-Machine path that starts at session selection and opens control only after explicit choice.
- **Workspace tab** — one non-persistent current-page UI entry with its own Attachment lifecycle, chooser/projection/renderers, and session-title hint. Multiple tabs may target one Machine but share that Machine route's single writer pointer.
- **Projection** — one reconstructible owner-process view of observed tmux state.

OwlMux models no finer-grained human identity or authorization aggregate beneath Deployment. Target terminal/process state and process leases are also outside the OwlMux domain model.

## Normative language

The key words **MUST**, **MUST NOT**, **REQUIRED**, **SHOULD**, **SHOULD NOT**, and **MAY** are normative. Unqualified present tense describes target architecture. Surrounding normative text controls if a diagram omits detail.

## Change discipline

A cross-cutting change requires review of every applicable specification and public document. New credentials, durable entities, network paths, process owners, or access boundaries require an explicit owner, failure contract, and trust boundary before entering architecture.
