# Security

## Current foundation

The current Server exposes only placeholder assets plus `/health` and `/ready`. API/auth prefixes return not-implemented responses. Relay makes no network connection. Server clustering, product credentials, and terminal data do not exist yet.

## Target Deployment trust boundary

Once implemented, every Server node is a high-trust bastion in one Deployment trust domain. All Serving nodes run the exact same Server build and Deployment-critical configuration:

- every node holds the one Deployment API key and SSH private-key encryption key;
- clustered nodes hold the one cluster key and an internal TLS identity;
- an ingress/owner path can observe live terminal input/output;
- any owner can decrypt every Deployment SSH credential while attaching;
- an owner can act with every target account configured for its Machines.

Fixed private-key encryption protects database contents at rest, not a compromised running Server node. Cluster membership is not an isolation boundary between mutually hostile nodes.

## Deployment API key

One `OWLMUX_API_KEY`, formatted as `owlmux_sk_v1_` plus canonical unpadded base64url for exactly 32 operator-generated random bytes, grants complete Deployment authority. The same value is configured on every node. Deployment is the sole human/API trust boundary and has no finer-grained authorization or persistent Browser-authentication state.

Browser keeps the key only in current page memory, sends Bearer on every protected HTTP request, and sends it once as the first bounded attachment-WebSocket frame. It never enters URL/query/cookie/subprotocol/Web Storage/service worker/logs/analytics. Reload or logout clears it.

The accepting node verifies the key before Machine lookup, owner resolution, internal owner-WSS, or target allocation. Only Browser/Machine-affine API traffic may use at most one such hop; Relay/enrollment stays on its accepting node. A remote owner receives only short-lived cluster-authenticated verified context, never raw API-key bytes.

Same-origin XSS can steal the key and control the entire Deployment. Restrictive CSP, no third-party scripts, safe rendering, exact Origin, and memory-only handling are security boundaries, not optional UI hardening.

Key rotation drains/stops all nodes, waits for old leases to become invalid, replaces the sole value, increments the Deployment configuration epoch/proof, and starts coherent nodes. Old connections and config generations cannot rejoin. An ordinary unchanged-key node restart may reuse a still-open page-memory candidate for fresh authentication. Rotation has no grace key, online mutation, per-node transition, or durable authentication state.

## Cluster trust and fencing

Clustered mode uses one distinct canonical 32-byte `OWLMUX_CLUSTER_KEY` plus TLS on every internal connection. The destination first sends a fresh one-use challenge; ingress returns one domain-separated HMAC response binding that challenge, a source nonce, source/destination node incarnations, configuration epoch, Machine route revision/connection epoch, and connection class under a destination-local `CLOCK_BOOTTIME` deadline. No sender timestamp or reusable assertion exists. The key also creates a domain-separated configuration-consistency proof.

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

Target tmux and sshd are authoritative. A compromised expected target can emit malicious terminal data or control its shell. Host verification prevents silent routing to a different host but does not make the expected host trustworthy.

One owner-local pointer identifies the Browser attachment allowed to send OwlMux input, target resize, session creation, and the small typed mutation set for a Machine connection epoch/socket incarnation. Any API-key holder can explicitly take it over, so it is coordination UX rather than a privilege boundary. The owner atomically replaces the pointer and verifies the authenticated connection plus Machine/attachment epochs on every write; there is no writer TTL, renewal, generation, token, or distributed lock. Native tmux clients remain outside this coordination.

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

Credentials may be reused by multiple Machines. UI exposes reuse count because compromise scope follows all bindings. Initialization generates a default Ed25519 credential; the API-key holder may generate, rename, reset, rotate by replacement, and rebind. OwlMux accepts no private-key upload, imported key, passphrase, or alternate algorithm, and never reveals/downloads stored private keys. Target administrators exclusively install/remove public keys; OwlMux/Relay never mutate authorization stores. Rebind affects future SSH authentication and does not revoke an already authenticated child.

## Database and backup compromise

A disclosed database contains Machine/host/audit data, Relay public keys, node/owner coordination, metadata, and encrypted SSH credentials. It does not contain the API, cluster, or SSH encryption keys or terminal content. Database write compromise can corrupt product/owner authority and is a full Deployment integrity incident.

PostgreSQL HA, backup, and restore are operator responsibilities. OwlMux assumes the configured endpoint exposes one linearizable single-writer non-rollback history and preserves acknowledged commits; it does not validate topology or repair rollback. Before an operator restore, stop/isolate all Server nodes and restart fresh incarnations. If history goes backward, lease, revocation, enrollment, epoch, and credential guarantees are unsupported. Backups and the separate SSH encryption key remain sensitive.

## Separate Deployments

Scale can occur inside one Deployment through symmetric nodes. Stronger isolation or external sharding uses separate Deployments. Each has separate origin, Deployment ID, API/cluster/encryption keys, PostgreSQL, membership, credentials, Machines, Relays, and attachments.

A compromise remains Deployment-local only if operators do not reuse secrets or clone active databases across Deployments. OwlMux offers no shared database, cross-Deployment routing, automatic migration, failover, or global view.

## Process continuity

API-key replacement, node drain/fence, Machine owner loss, Machine disablement, Relay revocation, database failure, or infrastructure loss closes OwlMux access only. Credential rebind applies to future SSH children. None of these actions kills target tmux.

## Reporting vulnerabilities

Report privately through the repository [security policy](https://github.com/owlfoundry/owlmux/security/policy), not a public issue.

The normative threat model is in the [operations, security, and resilience specification](https://github.com/owlfoundry/owlmux/blob/main/spec/08-operations-security-and-resilience.md).
