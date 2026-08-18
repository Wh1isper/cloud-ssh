# Deployment

## Choose a profile

Start with the default `single-node` profile unless one Server process and host are demonstrably insufficient. It runs the complete product, uses the local-owner fast path, and needs only PostgreSQL plus four Deployment values. The `clustered` profile adds symmetric capacity, but also requires internal TLS, a shared cluster key, per-node certificates and URLs, and coordinated cold configuration changes. It is not required for durability: target tmux owns terminal sessions in both profiles.

The production image runs one unprivileged `owlmux-server` process, includes the exact Web build, and listens on `0.0.0.0:8080`. Build and smoke-test the current source image with `make docker-build`. `ghcr.io/owlfoundry/owlmux:0.0.1` is the initial immutable evaluation image; the moving `:dev` tag follows the latest CI-qualified `main` and should not be used as a deployment pin.

## Single-node container deployment

A real single-node container deployment should explicitly provide only these OwlMux values:

1. `OWLMUX_PUBLIC_ORIGIN`, the exact external HTTPS origin with no trailing slash;
2. `OWLMUX_DATABASE_URL`, pointing to one private PostgreSQL writer;
3. `OWLMUX_API_KEY`, the full-authority Browser/API key;
4. `OWLMUX_SSH_KEY_ENCRYPTION_KEY`, the key protecting generated SSH private-key envelopes.

The image already supplies the listener and Web paths. The profile, configuration epoch, lease timings, shutdown timeout, and SSH runtime path have safe initial defaults and should not be copied into every deployment merely to restate them.

### Generate a private environment file

Run this in a dedicated deployment directory, then replace the example origin. The generated PostgreSQL password is URL-safe, so it can be embedded directly in the example connection URL.

```bash
umask 077
python3 - <<'PY' > .env
import base64
import secrets


def random_value() -> str:
    return base64.urlsafe_b64encode(secrets.token_bytes(32)).rstrip(b"=").decode()


print("OWLMUX_IMAGE=ghcr.io/owlfoundry/owlmux:0.0.1")
print("OWLMUX_PUBLIC_ORIGIN=https://terminal.example.com")
print(f"OWLMUX_POSTGRES_PASSWORD={random_value()}")
print(f"OWLMUX_API_KEY=owlmux_sk_v1_{random_value()}")
print(f"OWLMUX_SSH_KEY_ENCRYPTION_KEY={random_value()}")
PY
```

Keep `.env` outside version control with mode `0600`. The API key grants complete Deployment access. Back up the SSH encryption key together with PostgreSQL; losing it makes stored credential envelopes unusable. OwlMux currently reads these authority values from the process environment, so an orchestrator secret facility must inject environment values rather than mount unsupported `_FILE` variables.

### Create `compose.yml`

This baseline keeps PostgreSQL private, exposes Server only on host loopback for a host reverse proxy, persists only PostgreSQL, and places the node-local SSH runtime root on private tmpfs owned by the image UID.

```yaml
name: owlmux

services:
    postgres:
        image: postgres:17.10-alpine
        restart: unless-stopped
        environment:
            POSTGRES_DB: owlmux
            POSTGRES_USER: owlmux
            POSTGRES_PASSWORD: ${OWLMUX_POSTGRES_PASSWORD:?set OWLMUX_POSTGRES_PASSWORD}
        healthcheck:
            test: ["CMD-SHELL", "pg_isready --username owlmux --dbname owlmux"]
            interval: 5s
            timeout: 3s
            retries: 20
        volumes:
            - postgres-data:/var/lib/postgresql/data

    server:
        image: ${OWLMUX_IMAGE:?set OWLMUX_IMAGE}
        restart: unless-stopped
        depends_on:
            postgres:
                condition: service_healthy
        environment:
            OWLMUX_PUBLIC_ORIGIN: ${OWLMUX_PUBLIC_ORIGIN:?set OWLMUX_PUBLIC_ORIGIN}
            OWLMUX_DATABASE_URL: postgresql://owlmux:${OWLMUX_POSTGRES_PASSWORD}@postgres:5432/owlmux
            OWLMUX_API_KEY: ${OWLMUX_API_KEY:?set OWLMUX_API_KEY}
            OWLMUX_SSH_KEY_ENCRYPTION_KEY: ${OWLMUX_SSH_KEY_ENCRYPTION_KEY:?set OWLMUX_SSH_KEY_ENCRYPTION_KEY}
        ports:
            - "127.0.0.1:8080:8080"
        tmpfs:
            - /tmp/owlmux-ssh:uid=10001,gid=10001,mode=0700

volumes:
    postgres-data:
```

If the reverse proxy is another container, attach it to the Compose network and route directly to `server:8080` instead of expecting it to reach host loopback. Do not expose PostgreSQL publicly.

### Start and verify

```bash
docker compose --env-file .env config --quiet
docker compose --env-file .env up --detach
docker compose ps
curl --fail http://127.0.0.1:8080/health
curl --fail http://127.0.0.1:8080/ready
```

Terminate public TLS at a trusted reverse proxy, publish the one exact `OWLMUX_PUBLIC_ORIGIN`, preserve WebSocket upgrades, and use HTTP/1.1 upstream. Do not put the API key in proxy configuration, URLs, cookies, or health checks. `/health` is the liveness probe; `/ready` is the load-balancer admission probe.

## Configuration priorities

### Deployment values for a real single-node install

| Variable                        | Purpose                                                                                                                                           |
| ------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------- |
| `OWLMUX_PUBLIC_ORIGIN`          | Exact Browser/API origin such as `https://terminal.example.com`, without a path or trailing slash. Set it explicitly for every non-local install. |
| `OWLMUX_DATABASE_URL`           | Private `postgres://` or `postgresql://` endpoint exposing one linearizable writer.                                                               |
| `OWLMUX_API_KEY`                | `owlmux_sk_v1_` plus canonical unpadded base64url for exactly 32 random bytes. It grants complete Deployment access.                              |
| `OWLMUX_SSH_KEY_ENCRYPTION_KEY` | Canonical unpadded base64url for exactly 32 random bytes. Store it outside PostgreSQL and protect its backup.                                     |

### Image defaults

| Variable                                  | Image/default value         | Change only when                                                                                            |
| ----------------------------------------- | --------------------------- | ----------------------------------------------------------------------------------------------------------- |
| `OWLMUX_ADDR`                             | `0.0.0.0:8080` in the image | The container needs a different internal listener; also replace the image health check and port mapping.    |
| `OWLMUX_WEB_DIR`                          | `/usr/share/owlmux/web`     | Running an unpacked archive with a separately located exact Web build.                                      |
| `OWLMUX_SSH_RUNTIME_ROOT`                 | `/tmp/owlmux-ssh`           | Supplying another node-local private `0700` directory owned and writable by the Server UID.                 |
| `OWLMUX_PROFILE`                          | `single-node`               | Enabling the complete clustered configuration described below.                                              |
| `OWLMUX_CONFIG_EPOCH`                     | `1`                         | Performing a controlled all-node cold configuration transition; never increment it for an ordinary restart. |
| `OWLMUX_NODE_LEASE_TTL_SECONDS`           | `30`                        | The whole Deployment has an explicitly qualified alternative lease budget.                                  |
| `OWLMUX_NODE_LEASE_SAFETY_MARGIN_SECONDS` | `5`                         | The whole Deployment has a conservative qualified margin where `0 < margin < TTL`.                          |
| `OWLMUX_SHUTDOWN_TIMEOUT_SECONDS`         | `10`                        | The operator has chosen another bounded drain timeout from 1 through 60 seconds.                            |
| `OWLMUX_NODE_NAME`                        | unset                       | A stable diagnostic label is useful; it never supplies authority.                                           |
| `RUST_LOG`                                | `info`                      | A different structured log filter is required without enabling payload or secret capture.                   |

Treat lease settings, profile, epoch, public origin, authority-key digests, and exact build as Deployment-wide configuration. Do not vary them casually between nodes. Container CPU and memory limits remain ordinary runtime settings rather than OwlMux environment variables.

## Development infrastructure

`dev/compose.yml` provides PostgreSQL plus an opt-in loopback sshd/tmux target fixture; it does not start the OwlMux Server and is not a production Compose template:

```bash
make dev-up
make dev-status
make dev-down
```

PostgreSQL binds only to a loopback development port. Default development credentials are not production settings. Use `make dev-target-up` for the target fixture and `make test-e2e` for an isolated full acceptance run.

## Clustered Deployment shape

The implemented clustered profile runs one or more symmetric Server nodes against one private PostgreSQL database:

```mermaid
flowchart TB
    clients["Browsers, API clients, and Relays"] --> origin["One Deployment origin"]
    origin --> ingress["TLS ingress or load balancer"]

    subgraph deployment["One Deployment trust domain"]
        ingress --> nodeA["owlmux-server node A"]
        ingress --> nodeB["owlmux-server node B"]
        ingress --> nodeN["owlmux-server node N"]
        nodeA --> postgres[("Private PostgreSQL")]
        nodeB --> postgres
        nodeN --> postgres
        nodeA <-->|"Browser/API owner WSS only"| nodeB
        nodeB <-->|"Browser/API owner WSS only"| nodeN
    end

    relayA["Relay on target A"] --> origin
    relayB["Relay on target B"] --> origin
```

Every Serving node runs the exact same Server image, build, Deployment-critical configuration, and capabilities. Each process uses Tokio's multi-threaded runtime across its assigned CPU budget. A one-node installation takes the local-owner fast path; adding equivalent nodes supplies horizontal scale without a Gateway/Worker split.

PostgreSQL is the only durable product authority and stores low-churn node leases plus actual Machine-owner epochs. It does not carry terminal payload. Each current owner process holds its Machine Relay tunnels, SSH/tmux clients, projections, Browser writer coordination, and bounded queues locally.

The public load balancer distributes new Relay connections using ordinary connection-level policy. The accepting/authenticating Server incarnation is the only node allowed to claim that Machine and keeps all Relay/Machine-affine live state local. OwlMux promises no even distribution and has no placement hash, candidate ranking, automatic/manual rebalance, or live migration.

A non-owner Browser or Machine-affine API ingress may open at most one bounded internal owner WSS hop. One-shot API control uses the same WSS challenge mode; Relay/enrollment and raw public credentials never cross it. Stickiness is optional and clients always use the same Deployment origin.

The design deliberately has no Redis, message queue, scheduler service, terminal broker, virtual-bucket table, distributed writer lock, or replicated terminal state.

## Clustered profile configuration

Choose the clustered profile only for additional Server capacity. Every node uses the same image, PostgreSQL endpoint, public origin, API key, SSH encryption key, cluster key, epoch, lease settings, and CA trust. Each node uses its own internal URL, certificate, private key, and optional diagnostic name.

| Additional variable        | Requirement                                                                                                               |
| -------------------------- | ------------------------------------------------------------------------------------------------------------------------- |
| `OWLMUX_PROFILE`           | Set to `clustered` on every node.                                                                                         |
| `OWLMUX_CLUSTER_KEY`       | One shared canonical unpadded base64url value for exactly 32 random bytes, distinct from the API and SSH encryption keys. |
| `OWLMUX_INTERNAL_ADDR`     | Dedicated internal TLS listener socket, for example `0.0.0.0:8443`.                                                       |
| `OWLMUX_INTERNAL_URL`      | This node's reachable exact `wss://` URL ending in `/internal/v1/owner`, with no query or fragment.                       |
| `OWLMUX_INTERNAL_TLS_CERT` | Absolute container path to this node's PEM certificate chain.                                                             |
| `OWLMUX_INTERNAL_TLS_KEY`  | Absolute container path to this node's PEM private key.                                                                   |
| `OWLMUX_INTERNAL_TLS_CA`   | Absolute container path to the private CA certificates trusted for peer destinations.                                     |

Mount certificate files read-only and keep the internal listener reachable only by Deployment nodes. The advertised host must match the certificate DNS/IP subject alternative name. Server validates its certificate, key, CA roots, and advertised hostname through a bounded in-memory client/server handshake before registering the incarnation. Internal TLS is mandatory and has no plaintext fallback.

The cluster key authenticates configuration proofs and fresh Browser/API owner-WSS hops only. It never substitutes for the Deployment API key, Relay identity, SSH credential, or SSH encryption key. One configuration proof commits the exact build, epoch/profile, authority-key digests, public origin, protocol/schema generations, and bounds; a same-epoch mismatch fails registration instead of creating mixed Serving membership. Single-node mode rejects any cluster variable, including an empty value, so remove cluster entries entirely rather than leaving blank placeholders.

### Node identity and discovery

Operators do not assign a durable Node ID or configure a static peer list. Every Server process generates a fresh authority-bearing incarnation UUID at startup. `OWLMUX_NODE_NAME` is only a diagnostic label and never supplies authority. The process registers its exact incarnation, advertised `OWLMUX_INTERNAL_URL`, lease, build, and configuration proof in PostgreSQL. A Machine owner row references that exact incarnation UUID.

When an ingress node resolves a remote Machine owner, it reads the owner incarnation from PostgreSQL, then reads that incarnation's registered internal WSS URL and lease-valid configuration. TLS validates the advertised hostname against the Deployment-private CA; a fresh cluster-HMAC exchange binds the source incarnation, destination incarnation, Machine, configuration, route, and local deadline. PostgreSQL therefore supplies membership discovery without a separate peers file, Consul, etcd, Redis, or internal load balancer.

A container restart may reuse its service DNS name, certificate, and internal URL, but it always receives a new incarnation UUID. If a request for an old incarnation reaches the replacement process, the destination-incarnation challenge does not match and the route fails closed. The new process cannot impersonate the old owner; Relay recovery must wait for explicit release or lease expiry and then claim a higher Machine connection epoch.

### Docker Compose node addressing

Compose deployments should define one explicit service per Server node so each live node has a unique service DNS name, advertised URL, and node-specific certificate. The following cluster-specific fragment replaces the single `server` service in the baseline Compose file while retaining its `postgres` service and durable volume:

```yaml
x-cluster-environment: &cluster-environment
    OWLMUX_PROFILE: clustered
    OWLMUX_PUBLIC_ORIGIN: ${OWLMUX_PUBLIC_ORIGIN:?set OWLMUX_PUBLIC_ORIGIN}
    OWLMUX_DATABASE_URL: postgresql://owlmux:${OWLMUX_POSTGRES_PASSWORD}@postgres:5432/owlmux
    OWLMUX_API_KEY: ${OWLMUX_API_KEY:?set OWLMUX_API_KEY}
    OWLMUX_SSH_KEY_ENCRYPTION_KEY: ${OWLMUX_SSH_KEY_ENCRYPTION_KEY:?set OWLMUX_SSH_KEY_ENCRYPTION_KEY}
    OWLMUX_CLUSTER_KEY: ${OWLMUX_CLUSTER_KEY:?set OWLMUX_CLUSTER_KEY}
    OWLMUX_CONFIG_EPOCH: "1"
    OWLMUX_INTERNAL_ADDR: 0.0.0.0:8443
    OWLMUX_INTERNAL_TLS_CA: /run/owlmux/tls/ca.crt

services:
    server-a:
        image: ${OWLMUX_IMAGE:?set OWLMUX_IMAGE}
        restart: unless-stopped
        depends_on:
            postgres:
                condition: service_healthy
        environment:
            <<: *cluster-environment
            OWLMUX_NODE_NAME: server-a
            OWLMUX_INTERNAL_URL: wss://server-a:8443/internal/v1/owner
            OWLMUX_INTERNAL_TLS_CERT: /run/owlmux/tls/server.crt
            OWLMUX_INTERNAL_TLS_KEY: /run/owlmux/tls/server.key
        volumes:
            - ./tls/ca.crt:/run/owlmux/tls/ca.crt:ro
            - ./tls/server-a.crt:/run/owlmux/tls/server.crt:ro
            - ./tls/server-a.key:/run/owlmux/tls/server.key:ro
        tmpfs:
            - /tmp/owlmux-ssh:uid=10001,gid=10001,mode=0700
        expose:
            - "8080"
            - "8443"

    server-b:
        image: ${OWLMUX_IMAGE:?set OWLMUX_IMAGE}
        restart: unless-stopped
        depends_on:
            postgres:
                condition: service_healthy
        environment:
            <<: *cluster-environment
            OWLMUX_NODE_NAME: server-b
            OWLMUX_INTERNAL_URL: wss://server-b:8443/internal/v1/owner
            OWLMUX_INTERNAL_TLS_CERT: /run/owlmux/tls/server.crt
            OWLMUX_INTERNAL_TLS_KEY: /run/owlmux/tls/server.key
        volumes:
            - ./tls/ca.crt:/run/owlmux/tls/ca.crt:ro
            - ./tls/server-b.crt:/run/owlmux/tls/server.crt:ro
            - ./tls/server-b.key:/run/owlmux/tls/server.key:ro
        tmpfs:
            - /tmp/owlmux-ssh:uid=10001,gid=10001,mode=0700
        expose:
            - "8080"
            - "8443"
```

Issue `server-a.crt` with `DNS:server-a` in its subject alternative names and `server-b.crt` with `DNS:server-b`; sign both with the mounted private CA. Keep each private key readable by only the Server container UID and keep port `8443` on the private Compose network. The public reverse proxy distributes HTTP/WSS connections across `server-a:8080` and `server-b:8080` and removes a node when `/ready` fails.

Do not use `docker compose up --scale server=3` with one shared internal URL and certificate. Compose service DNS may resolve that shared name to any replica, so a connection intended for one owner incarnation can reach another and fail the exact destination challenge. Use explicit services as above, or an orchestrator that injects a unique ordinal DNS name, advertised URL, and certificate into every replica, such as a Kubernetes StatefulSet.

Public load balancing uses ordinary connection-level distribution across ready nodes. Stickiness is optional. Relay and enrollment stay on their accepting node, while Browser and Machine-affine API traffic may use at most one internal owner WSS hop. There is no live migration, placement scheduler, or automatic rebalance.

## Health, readiness, and fencing

`/health` reports only process/event-loop liveness. `/ready` reports whether that node can safely accept new work.

A node becomes unready when draining, when a required dependency is unavailable, or when its conservative local node-lease deadline arrives. For lease TTL `L`, the initial Linux profile samples `CLOCK_BOOTTIME` as `b0` before registration/renewal and, after an exact success, sets `local_hard_deadline = b0 + L - S` using one Deployment-wide safety margin `S`. The pre-request sample includes request delay; `S` covers the supported PostgreSQL forward adjustment plus bounded local clock-read, scheduling, dispatch, and fence overhead. Every acceptance and target dispatch checks this deadline directly. Startup validates only clock availability and `0 < S < L`, not future platform or database behavior. Ordinary `Instant`, `CLOCK_MONOTONIC`, wall time, and Tokio timers alone are not lease authority. At the deadline the incarnation is irreversibly fenced: it rejects input/mutations, ignores late renewal responses, closes every owned Relay, Browser, SSH, and tmux connection, and must restart fresh rather than return to Serving. Another node may claim those Machines only after PostgreSQL observes the old database-time lease as invalid.

Host suspend/container freeze requires the elapsed clock to advance across it. Operators must never resume, clone, or live-migrate the same process snapshot or frozen-clock incarnation; terminate it and start a fresh incarnation before I/O. This fail-closed gap prevents split OwlMux dispatch authority. Node fencing never signals or destroys target tmux.

## Operational signals

Authenticated operators can read `/api/v1/metrics` for a bounded low-cardinality JSON snapshot of node readiness, API authentication/overload totals, and local/remote/absent/failed owner-resolution classes. Counters contain no Deployment, Machine, node-incarnation, request, stream, terminal, credential, endpoint, or key labels. `/api/v1/audit-events` returns at most the newest 200 safe durable events for configuration, credential, Machine, enrollment, Relay, owner, attachment path/lifecycle, SSH verification/control, writer, and closed tmux mutations. Security-critical durable mutations commit their required audit event atomically; live attachment/target events use a bounded best-effort write and never change or replay a target result when audit storage is unavailable. Audit excludes pane input, Browser resize, raw credentials, terminal input/output, unsafe target diagnostics, key paths, internal URLs, and challenge/HMAC payloads.

Server emits structured operational logs under `RUST_LOG`. Keep production collection redacted and bounded; do not enable payload capture or place raw secrets in log filters or surrounding process metadata. Missing/invalid API-key attempts create neither source-identifying logs nor durable audit rows.

Public HTTP allows at most 256 accepted connections per node, closes incomplete HTTP/1 headers after five seconds, caps parsed headers to a 32 KiB connection buffer, caps bodies at 16 KiB, and limits parsed requests to 15 seconds. API authentication has bounded node/per-observed-peer attempts, authenticated mutations have a separate concurrency bound, attachments/Relays/internal WSS have independent capacities, and the critical PostgreSQL pool remains separate from ordinary public/enrollment work. Overload closes or rejects only the affected OwlMux scope and never target tmux.

The Server listener accepts HTTP/1.1, including WebSocket upgrade; terminate public HTTP/2 or HTTP/3 at the trusted reverse proxy and use HTTP/1.1 upstream. The observed source is deliberately the direct TCP peer. All traffic from one proxy instance therefore shares that peer quota; OwlMux never trusts `Forwarded` or `X-Forwarded-For`. Size the proxy pool for Relay reconnect bursts and enforce external-client source limits, TLS/header deadlines, and a connection cap at the proxy as the first public edge.

## Join, drain, and failure

Joining registers a fresh coherent node incarnation and begins accepting whatever new connections the load balancer sends to it. It does not move or rebalance existing owners and creates no distribution guarantee.

Controlled drain:

1. marks the exact node incarnation `Draining` and unready;
2. excludes it from new owner claims;
3. for each owner, closes the dispatch barrier, rejects writes, and fences routes/children/writers/queues/results in bounded batches;
4. only after local fencing, CAS-releases the exact owner epoch when PostgreSQL is available;
5. lets Relay and Browser reconnect through the unchanged Deployment origin;
6. removes only that node's local OpenSSH runtime files and exits.

A crash cannot transfer live sockets or clean its registry rows. Relays reconnect, owner claims wait for lease expiry, and only the node accepting a later Relay reconnect may obtain a higher Machine connection epoch. Browser reconnects and hydrates from target tmux. No terminal input, output, pending operation, Browser writer state, or projection is replayed.

## API-key and cluster configuration changes

API-key, Server build, and Deployment-critical configuration changes use a controlled all-node cold transition:

1. drain and stop every Server node;
2. wait until no old node lease remains valid;
3. atomically install the new release's embedded `server_build_id` with the Deployment-wide configuration and next explicit epoch under the shared Deployment-row lock;
4. apply compatible schema/protocol changes;
5. keep the public origin gated if distribution matters, start every intended exact-build/config node, then open ingress.

A same-epoch proof mismatch, local embedded `server_build_id` mismatch, stale node, or non-exact Relay protocol fails before Serving. Mixed-build rolling Server upgrade is unsupported. The first implementation has no Relay version negotiation or compatibility manifest; that policy is deferred until a second protocol version exists. There is no dual-key grace or per-node API-key rotation. Target tmux continues throughout.

## PostgreSQL operator contract

PostgreSQL HA, failover, backup, restore, promotion, and replica fencing are deployment-operator responsibilities. OwlMux uses one configured endpoint and assumes it exposes one linearizable single-writer non-rollback history that preserves every acknowledged commit. OwlMux does not discover topology, validate recovery, clear rows to repair a rolled-back history, or orchestrate database failover.

Before an operator restore or history replacement, stop/isolate every Server node and start only fresh process incarnations afterward. This prevents old processes from continuing but does not make rollback safe. If history loses acknowledged commits or revives earlier rows, OwlMux provides no one-use-token, revocation, lease/owner epoch, configuration, audit, or credential-lifecycle guarantee. Treat it as an unsupported Deployment integrity incident.

The operator separately protects the matching SSH encryption key and other startup secrets. Database state never restores target tmux or live OwlMux state. A database copy must not run concurrently as the same Deployment; initialize a separate Deployment independently.

Read [Getting started](getting-started.md) for current commands, [Recovery and incident response](recovery.md) for cold restore/rotation and compromise runbooks, and the normative [operations and resilience specification](https://github.com/owlfoundry/owlmux/blob/main/spec/08-operations-security-and-resilience.md) for the target Deployment boundary.
