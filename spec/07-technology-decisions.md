# Technology Decisions

## Decision

OwlMux starts again from a minimal public Rust Server, target-side Rust Relay,
and React application. Technology is added only when one accepted end-to-end
delivery block requires it.

The repository begins with only the selected OwlMux Server, Relay, Web,
documentation, PostgreSQL/Redis development, container, and CI boundaries. It
retains no superseded runtime or compatibility abstraction.

## Repository Shape

The foundation contains:

```text
apps/
  web/                  # React and Vite application source
crates/
  owlmux-server/        # public Web, control, SSH, tmux, and relay service
  owlmux-relay/         # target-side outbound reverse-connection client
dev/
  compose.yml           # PostgreSQL and Redis development infrastructure
docs/                   # VitePress documentation and Workers config
spec/                   # normative target product specifications
```

The first commit intentionally has exactly two runtime crates because Server and
Relay are independent deployed processes with opposite trust and network
boundaries. Both begin as honest placeholders.

`owlmux-core` may be extracted only when protocol values are genuinely shared by
implemented Server and Relay code. A native application is added only after the
Web product is qualified. Do not pre-create empty domain, storage, SDK, CLI, key
provider, or protocol crates.

## Rust Baseline

- stable Rust through `rust-toolchain.toml`;
- Rust edition 2024;
- Cargo resolver 3;
- Tokio for asynchronous process, socket, signal, and queue ownership;
- Axum for HTTP, WebSocket, health, and embedded application delivery;
- `tracing` for structured diagnostics;
- workspace-wide `unsafe_code = "forbid"`;
- Clippy `all` and `pedantic` as denied CI warnings.

The placeholder Server implements only process startup, `/health`, `/ready`, and
embedded static assets. The placeholder Relay reports its build identity and that
enrollment is not implemented. Neither pretends to implement product capability.
Product dependencies are not added until their delivery block begins.

## SSH Client

The initial product uses the system OpenSSH client as a constrained child
process. Reasons:

- mature SSH protocol, host-key, host-certificate, and `ProxyJump`
  implementation;
- dedicated Server-owned config and `known_hosts` inputs without ambient
  user-config or target-agent inheritance;
- no need to maintain a second SSH stack before validating the product;
- one process boundary is easy to kill without affecting target tmux.

The production image installs the OpenSSH client. OwlMux controls a dedicated SSH
configuration boundary, cleans the child environment, selects only the
per-machine target identity, and constructs arguments without a shell. A later
operator-owned direct-route profile may separately configure bastion credentials,
but it cannot widen target authentication.

An embedded Rust SSH client is not selected. It may replace OpenSSH only after a
measured deployment requirement and equivalent security/compatibility tests.

OwlMux never implements an SSH server in the target architecture.

## tmux Integration

OwlMux implements the required tmux control-mode parser and typed command renderer
inside `owlmux-server` first. It uses real tmux transcripts and process tests as
compatibility fixtures.

Do not add:

- a generic terminal multiplexer abstraction;
- a tmux server implementation;
- terminal screen scraping;
- a custom PTY runtime;
- a durable shadow session model;
- a second terminal parser on the server unless a proven protocol need requires
  one.

The browser uses xterm.js for pane rendering. Target tmux owns terminal state and
scrollback.

## Web Application

The application baseline follows the OwlFoundry workspace convention:

- Node.js 24;
- pnpm workspace with a pinned pnpm version;
- React 19;
- TypeScript;
- Vite;
- Vitest;
- ESLint and Prettier;
- xterm.js when the terminal vertical slice begins.

Source lives only under `apps/web`. The Rust server embeds or serves one
production `apps/web/dist` artifact. Generated production assets are not kept as
a second source tree.

The placeholder application is intentionally one page stating product direction
and implementation status. It contains no fake login, target, or terminal UI.

## Authentication

### OwlAuth

OwlMux integrates through OwlAuth Project Auth, not downstream OIDC. The Web flow
uses the supported OwlAuth client/Runtime contract. The server validates Project
access-token JWTs with exact configured issuer, Project audience, OwlMux
Application ID, JOSE `typ = at+jwt`, EdDSA key profile, time, subject, and session
claims.

The first implementation should consume an exact released OwlAuth SDK or its
published Runtime contract rather than copying OwlAuth internal models. OwlAuth
storage, migrations, provider adapters, and Control APIs never become OwlMux
dependencies.

### API-key mode

One configured high-entropy `OWLMUX_API_KEY` authenticates the implicit owner
and its default organization. OwlMux does not add a password database or local
OAuth server.

### Browser sessions

Opaque OwlMux sessions bridge either authentication mode to cookie-authenticated
HTTP and WebSocket access. PostgreSQL stores session digests and authority; Redis
may cache validity but cannot widen it.

## PostgreSQL And Redis

PostgreSQL is the only durable product database. SQLx uses explicit migrations,
foreign keys, checked transactions, bounded pools, and Rustls. It stores users,
organizations, memberships, machines, enrollment digests, Relay public keys,
encrypted SSH keys, browser-session digests, and audit.

Redis is required disposable infrastructure for bounded rate limits, session and
revocation cache, enrollment admission, and advisory machine reachability. It
does not own users, organizations, machines, Relay tunnels, SSH children, tmux
projection, terminal data, or distributed locks.

The initial Server remains one process. PostgreSQL and Redis do not imply
multi-Server coordination or SaaS. Do not add a generic multi-backend repository
abstraction.

## Secret Custody

Recoverable SSH private keys use one small object-safe interface inside
`owlmux-server`. The official statically composed provider reads one fixed
`OWLMUX_SECRET_ROOT_KEY` environment value and uses versioned authenticated
encryption with context-bound associated data.

Operators needing KMS/HSM custody implement the interface and compile their own
Server. Do not add a KMS SDK, remote provider protocol, dynamic plugin, key
management UI, multiple active root keys, or rotation workflow. Extracting a
public provider crate requires an actual external implementation.

Terminal input, output, scrollback, and tmux projection are never product
metadata.

## Relay Transport

The Relay implementation uses:

- outbound WebSocket over TLS on TCP 443;
- Rustls for Relay TLS clients;
- Ed25519 machine signatures;
- a small named, versioned binary or JSON control envelope selected from measured
  stream overhead and implementation simplicity;
- bounded stream multiplexing over one connection.

Do not add QUIC, STUN, TURN, ICE, libp2p, generic VPN, gRPC, or a general reverse
proxy framework in the first Relay implementation.

Wire values remain private to their runtime crates until both sides implement a
stable contract that justifies a shared module or crate. No placeholder public
protocol API is published early.

## Documentation

Public documentation follows the OwlAuth repository pattern:

- VitePress source under `docs/`;
- Mermaid rendered through a local VitePress theme component;
- local search;
- static output deployed with Cloudflare Workers assets;
- Worker name `owlmux-docs`;
- canonical initial docs origin `https://owlmux-docs.owlfoundry.org`;
- GitHub source links point to `https://github.com/owlfoundry/owlmux`.

`spec/` is normative target design. `docs/` is public guidance and must label the
repository as a foundation until capabilities are implemented.

## Container

One multi-stage Dockerfile:

1. installs locked pnpm dependencies and builds `apps/web`;
2. builds `owlmux-server --release --locked` with the exact Web artifact;
3. creates a minimal Debian runtime containing CA certificates, OpenSSH client,
   curl, and tini;
4. runs as a fixed unprivileged `owlmux` user;
5. exposes one HTTP port and uses `/health` for the image health check;
6. includes OCI source, version, revision, and license labels.

The Server image contains no Rust toolchain, Node.js, package manager, source
tree, SSH private key, OwlAuth credential, API key, secret root key, or Relay
private key. PostgreSQL and Redis are external services and are not bundled into
the image.

## CI And Release

CI on every pull request and `main` push is the sole validation authority. It:

- installs locked pnpm dependencies;
- formats, lints, tests, and builds the Web application;
- builds VitePress docs and dry-runs Workers deployment;
- checks Rust formatting, Clippy, tests, and release build;
- validates Markdown links and repository naming boundaries;
- builds and smoke-tests the production image.

Release workflows do not repeat CI. They assume a protected commit whose CI
already passed and perform only artifact construction, checksums, GHCR upload,
and GitHub Release publication. A release job may verify tag/version shape and
artifact presence; it must not rerun source tests or lint as disguised release
validation.

The initial server is not published to crates.io. The deliverables are:

- `owlmux-server` archives for supported release targets;
- `owlmux-relay` archives for supported target platforms;
- `ghcr.io/owlfoundry/owlmux` Server container tags;
- checksums and GitHub Release notes;
- the independently deployed documentation Worker.

## Explicitly Excluded Foundations

- legacy crate or database compatibility;
- server-side SSH endpoint;
- Agent installer or PTY supervisor;
- mdBook;
- npm workspaces outside pnpm;
- checked-in `node_modules` or build output;
- embedded SQLite or multi-backend storage abstraction;
- bundled PostgreSQL or Redis inside the Server image;
- KMS SDK and root-key rotation control plane;
- microservice decomposition;
- release-time duplicate CI validation.

## Acceptance Criteria

- A clean checkout can run the full CI contract with pinned lockfiles.
- Both placeholder binaries build and report honest foundation status.
- One Docker build produces an unprivileged Server image with the exact Web
  placeholder and healthy `/health` response.
- VitePress builds and Wrangler dry-run accepts the `owlmux-docs` Worker config.
- The repository contains only OwlMux package, binary, URL, documentation, and
  runtime boundaries and has no mdBook or superseded implementation tree.
- Placeholder code does not pretend to implement authentication, SSH, tmux, or
  Relay capabilities.
