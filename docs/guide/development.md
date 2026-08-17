# Development

## Repository layout

```text
apps/web/             React control plane and read-only xterm.js workspace
contracts/            reviewed public, Relay v1, and attachment v1 schemas and fixtures
crates/owlmux-server/ single-node Deployment, Relay ingress, SSH/tmux, API, and attachment runtime
crates/owlmux-relay/  target-side enrolled reverse-connection runtime
dev/                  PostgreSQL and opt-in loopback sshd/tmux Compose fixtures
docs/                 VitePress documentation and Workers configuration
spec/                 normative complete-product specifications
```

## Commands

```bash
make install            # install locked Cargo, pnpm, and Chromium test dependencies
make format             # format Rust and Web sources
make check              # generated contracts, lint, type-check, Web/docs, repository, and Compose checks
make test               # run Rust and Web tests
make test-containers    # require and run isolated PostgreSQL tests
make test-e2e           # real Relay/OpenSSH/tmux acceptance with Node clients and Chromium
make test-e2e-matrix    # add tmux 3.2a/3.3a/3.5a/current 3.7b and bash/dash coverage
make build              # build Web plus both release binaries
make dev                # build Web and run the single-node Server
make dev-up             # start development PostgreSQL
make dev-target-up      # start PostgreSQL plus the loopback sshd/tmux target fixture
make docs               # run VitePress locally
make docker-build       # build and smoke-test the Server image
```

## Current delivery boundary

Blocks 0–3 are current behavior in the single-node profile:

- reviewed/generated contracts, immutable typed configuration, Linux boot clock, cancellation, and bounded shutdown;
- Deployment initialization, fenced node lease, API-key control plane, generated encrypted SSH credentials, Machines, one-use tokens, and safe audit;
- Relay candidate identity custody, token-only enrollment, signed activation/tunnel transcripts, bounded logical streams, actual owner claim, re-enrollment, revoke/disable fencing, and drain;
- exact-Origin/first-frame attachment authentication, constrained OpenSSH, separate tmux client/server version and session probes, fresh explicit chooser, read-only control mode, target-authoritative visible-pane layout, qualified final-capture/live cutover with continuous-token stress evidence, chunked binary-safe snapshots/live output, projection refresh, and xterm.js rendering.

Writable pane input, session/window/pane mutations, writer takeover, clustered configuration proof, internal owner WSS, and multi-node acceptance remain later delivery blocks. Do not describe them as current.

## Design workflow

`spec/` is normative. Public docs distinguish current tested behavior from accepted target design. A capability becomes current behavior only after its reviewed versioned contract, durable invariants, implementation, and real acceptance evidence land together. CI runs PostgreSQL integration plus the complete Docker and Chromium E2E against Ubuntu 22.04 tmux 3.2a, Debian 12 tmux 3.3a with `dash`, Debian 13 tmux 3.5a, and checksum-pinned current upstream tmux 3.7b.

Keep the central invariant visible in every change:

```text
OwlMux failure means attachment loss, never target tmux cleanup.
```

## Code boundaries

- Server and Relay are the only runtime crates.
- Do not create empty abstraction crates or split Gateway/Worker/scheduler services.
- PostgreSQL is the only durable product authority. Terminal bytes, current projection, Relay sockets, OpenSSH children, and Browser state stay bounded and owner-local.
- Relay state stays on the accepting owner node. The current single-node profile has no remote-owner path; clustered Browser/API routing may later use at most one internal owner WSS hop, while Relay enrollment/tunnel is never proxied.
- SSH private-key encryption is one fixed local XChaCha20-Poly1305 path; do not add a provider abstraction, KMS/HSM framework, KDF, or encryption-key-management surface.
- Release workflows publish CI-qualified commits and do not rerun CI.

See [AGENTS.md](https://github.com/owlfoundry/owlmux/blob/main/AGENTS.md) for the complete repository guide.
