# Security

## Current implementation

The current Server initializes PostgreSQL, registers a fenced incarnation, serves the protected API and Browser, manages generated encrypted SSH credentials and Machines, accepts Relay enrollment/tunnels, claims actual Machine owners, and opens constrained interactive SSH/tmux attachments with owner-local writer coordination. In the clustered profile, already authenticated Browser and Machine-affine invalidation ingress may use one fresh challenge-authenticated internal TLS/WSS hop to the owner. Terminal bytes, current projections, writer state, and internal connections remain owner-local and are not persisted.

## Deployment trust boundary

Every Server node is a high-trust bastion in one Deployment trust domain. All Serving nodes run the exact same Server build and Deployment-critical configuration:

- every node holds the one Deployment API key and SSH private-key encryption key;
- clustered nodes hold the one cluster key and an internal TLS identity;
- an ingress/owner path can observe live terminal input/output;
- any owner can decrypt every Deployment SSH credential while attaching;
- an owner can act with every target account configured for its Machines.

Fixed private-key encryption protects database contents at rest, not a compromised running Server node. Cluster membership is not an isolation boundary between mutually hostile nodes.

## Deployment API key

One `OWLMUX_API_KEY`, formatted as `owlmux_sk_v1_` plus canonical unpadded base64url for exactly 32 operator-generated random bytes, grants complete Deployment authority. The same value is configured on every node. Deployment is the sole human/API trust boundary and has no finer-grained authorization or persistent Browser-authentication state.

Browser keeps the key only in current page memory, sends Bearer on every protected HTTP request, and sends it once as the first bounded attachment-WebSocket frame. It never enters URL/query/cookie/subprotocol/Web Storage/service worker/logs/analytics. Reload or logout clears it.

The accepting node verifies the key before Machine lookup, owner resolution, internal owner-WSS, or target allocation. Attachment and Relay upgrades share a node-wide pre-authentication attempt bound plus an expiry-pruned per-observed-TCP-peer bound; those attempt permits are released immediately after first-frame authentication, while established connections retain their separate capacity permit. OwlMux does not trust forwarded-address headers, so a reverse proxy is intentionally one observed peer for this limiter and must enforce any desired client-IP policy itself. Only Browser/Machine-affine API traffic may use at most one owner hop; Relay/enrollment stays on its accepting node. A remote owner receives only short-lived cluster-authenticated verified context, never raw API-key bytes.

Same-origin XSS can steal the key and control the entire Deployment. Restrictive CSP, no third-party scripts, safe rendering, exact Origin, and memory-only handling are security boundaries, not optional UI hardening.

Key rotation drains/stops all nodes, waits for old leases to become invalid, replaces the sole value, increments the Deployment configuration epoch/proof, and starts coherent nodes. Old connections and config generations cannot rejoin. An ordinary unchanged-key node restart may reuse a still-open page-memory candidate for fresh authentication. Rotation has no grace key, online mutation, per-node transition, or durable authentication state.

## Cluster trust and fencing

Clustered mode uses one distinct canonical 32-byte `OWLMUX_CLUSTER_KEY` plus TLS on every internal connection. Startup validates each node's configured certificate/key against its configured CA roots and advertised hostname before membership registration. The destination first sends a fresh one-use challenge; ingress returns one domain-separated HMAC response binding that challenge, a source nonce, source/destination node incarnations, configuration epoch, Machine route revision/connection epoch, and connection class under a destination-local `CLOCK_BOOTTIME` deadline. No sender timestamp or reusable assertion exists. The key also creates a domain-separated configuration-consistency proof. Attachment and one-shot control connections have separate bounded inbound and outbound capacity, so legitimate long-lived attachments cannot consume the reserved invalidation path.

The cluster key cannot authenticate public API/Browser, enrollment, Relay, SSH, or private-key encryption. A stored config proof or Deployment ID cannot authenticate a node. Raw API keys, enrollment tokens, Relay proof candidates, SSH keys, and encryption keys never cross internal owner WSS. One-shot API control uses the same WSS challenge mode, not internal HTTPS.

One valid Machine owner is enforced by:

- database-time Server-node leases;
- a conservative hard deadline derived from Linux `CLOCK_BOOTTIME` and one Deployment-wide lease safety margin;
- serialized PostgreSQL owner claims;
- one authority-bearing random process incarnation plus optional non-authoritative display name;
- monotonically increasing Machine connection epoch;
- binding of every internal stream, Attachment, and operation to those epochs plus direct verification of the current writer attachment for writes.

For lease TTL `L`, the initial Linux Server profile samples `CLOCK_BOOTTIME` as `b0` before registration/renewal and sets `local_hard_deadline = b0 + L - S` after an exact success. The one conservative margin `S` covers supported PostgreSQL forward adjustment plus bounded local clock-read, scheduling, dispatch, and fence overhead. Startup validates only clock availability and `0 < S < L`, not future platform/database behavior. Every acceptance and target dispatch checks the deadline directly; a timer is only a wakeup. At the deadline the incarnation becomes unready and irreversibly fences owner-local access. Another node waits until PostgreSQL observes the old lease as invalid. Host suspend/container freeze requires this clock to advance through it. Operators must never resume, clone, or live-migrate the same process snapshot or frozen-clock incarnation; terminate it and start a fresh process before I/O. Availability may pause, but two nodes must not both accept new OwlMux dispatch as valid owner. Target sshd/tmux does not validate OwlMux epochs, so bytes already dispatched by the old valid owner may still resolve late; OwlMux treats them as ambiguous, refreshes target state, and never replays or automatically compensates them.

Node join does not move existing owners and OwlMux has no automatic/manual rebalance. Drain and failure close OwlMux connections; Relay and Browser reconnect through the Deployment origin. A valid unreachable owner cannot be stolen or remotely evicted: ingress returns `owner_unreachable`, and the operator fences/stops/isolates that node and waits for lease expiry. No live bytes or pending operations migrate or replay.

Every Serving Server uses one exact build/configuration. The initial Relay protocol accepts one exact version and has no negotiation or compatibility manifest; policy for older versions waits until a second version exists.

A compromised Server node or cluster key is a Deployment-wide incident. Isolate all nodes, replace API/cluster configuration through a new epoch, assess SSH credential exposure, and replace/remove target public keys as necessary. OwlMux never automatically cleans target processes during response.

## Target trust and Browser writer coordination

The current single-node and clustered workspaces implement the writer coordination in this section. A remote ingress transports the same bounded attachment protocol to the owner; writer authority and target dispatch remain owner-local.

Target tmux and sshd are authoritative. A compromised expected target can emit malicious terminal data or control its shell. Host verification prevents silent routing to a different host but does not make the expected host trustworthy.

One route-scoped owner-local pointer identifies the Browser attachment allowed to send OwlMux input, target resize, session creation, and the small typed mutation set for a Machine connection epoch/socket incarnation. Multiple page-memory tabs for one Host remain separate attachments competing for that pointer. The product UI calls a free claim **Take control**, an occupied claim **Take over**, and the holder **You have control**; any API-key holder can explicitly take it over, so this is coordination UX rather than a privilege boundary. Every control client atomically attaches read-only with `ignore-size`. Claims and takeovers serialize through one owner-local dispatch barrier; takeover first makes the old client `ignore-size` and read-only, then uses tmux's dedicated read-only toggle to promote the claimant, clears `ignore-size`, installs the pointer and size, and freshly hydrates it. Only the current visible ready writer automatically derives bounded resize from its viewport; observers and hidden tabs cannot change target geometry. Every write rechecks the current route, authenticated connection, Machine/attachment/workspace epochs, and writer pointer immediately before and after target I/O; there is no writer TTL, renewal, generation, token, or distributed lock. Native tmux clients remain outside this coordination.

Browser input is accepted only for the currently observed active pane, is bounded to 1024 bytes, uses canonical base64url on the attachment protocol, and becomes fixed-width hex arguments to `send-keys -H`; it never enters shell or tmux command grammar as raw text. Session creation uses the target's configured tmux default command rather than a Browser-provided startup command. Target mutations have exact-success, known-failure, or conservative-ambiguous outcomes. OwlMux never retries or automatically compensates an ambiguous mutation and instead clears writer authority and returns to fresh discovery.

## Credential separation

Distinct credential classes are:

- Deployment API key;
- cluster key and internal TLS identity;
- Relay enrollment token;
- Relay Machine key;
- Deployment SSH credential;
- SSH private-key encryption key.

No credential is accepted as another class. Raw values remain out of URLs, logs, telemetry, audit, internal challenge/HMAC transcripts, and persistent Browser storage.

OpenSSH uses a dedicated Server-owned configuration, strict host inputs, exact Machine-selected Deployment credential, `IdentitiesOnly`, and no ambient target-authentication agent. Every node has a separate non-shared private runtime root with one exclusive startup directory and one exclusive directory per owner-local child, preferably on local tmpfs. Each identity is an exclusive `0600` file. The owner keeps the path through spawn/TCP/banner/host verification and unlinks only after the first valid authenticated remote-protocol record proves OpenSSH loaded it.

Child cleanup cannot remove siblings or another node's files. Each node scavenges only fully validated OwlMux-owned orphans from its own root. A hard crash can leave bounded plaintext until private mount/container teardown or that node's next startup.

Credentials may be reused by multiple Machines. UI exposes reuse count because compromise scope follows all bindings. Initialization generates a default Ed25519 credential; the API-key holder may generate, rename, reset, select a default, rotate by replacement, explicitly rebind an active Machine, and retire an unreferenced non-default credential. OwlMux accepts no private-key upload, imported key, passphrase, or alternate algorithm, and never reveals/downloads stored private keys. Target administrators exclusively install/remove public keys; OwlMux/Relay never mutate authorization stores. Active-Machine rebind changes only the credential selected for future SSH children, increments the independent credential revision, and does not revoke an already authenticated child or change the owner-fencing route revision.

## Database and backup compromise

A disclosed database contains Machine/host/audit data, Relay public keys, node/owner coordination, metadata, and encrypted SSH credentials. It does not contain the API, cluster, or SSH encryption keys or terminal content. Database write compromise can corrupt product/owner authority and is a full Deployment integrity incident.

PostgreSQL HA, backup, and restore are operator responsibilities. OwlMux assumes the configured endpoint exposes one linearizable single-writer non-rollback history and preserves acknowledged commits; it does not validate topology or repair rollback. Before an operator restore, stop/isolate all Server nodes and restart fresh incarnations. If history goes backward, lease, revocation, enrollment, epoch, and credential guarantees are unsupported. Backups and the separate SSH encryption key remain sensitive. Follow the executable evidence and cold restore boundary in [Recovery and incident response](recovery.md).

## Separate Deployments

Scale can occur inside one Deployment through symmetric nodes. Stronger isolation or external sharding uses separate Deployments. Each has separate origin, Deployment ID, API/cluster/encryption keys, PostgreSQL, membership, credentials, Machines, Relays, and attachments.

A compromise remains Deployment-local only if operators do not reuse secrets or clone active databases across Deployments. OwlMux offers no shared database, cross-Deployment routing, automatic migration, failover, or global view.

## Process continuity

API-key replacement, node drain/fence, Machine owner loss, Machine disablement, Relay revocation, database failure, or infrastructure loss closes OwlMux access only. Credential rebind applies only to future SSH children. None of these actions kills target tmux.

## Reporting vulnerabilities

Report privately through the repository [security policy](https://github.com/owlfoundry/owlmux/security/policy), not a public issue.

The normative threat model is in the [operations, security, and resilience specification](https://github.com/owlfoundry/owlmux/blob/main/spec/08-operations-security-and-resilience.md).
