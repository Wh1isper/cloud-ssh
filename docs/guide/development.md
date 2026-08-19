# Development

## Repository layout

```text
apps/web/             same-origin terminal-first shell, Host management, workspace tabs, and xterm.js rendering
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

The following is current behavior in the pre-release single-node and clustered profiles:

- reviewed/generated contracts, immutable typed configuration, Linux boot clock, cancellation, and bounded shutdown;
- Deployment initialization, fenced node lease, API-key control plane, generated encrypted SSH credentials, complete Machine/Relay lifecycle transactions, active-Machine credential rebind, one-use tokens, bounded safe audit presentation, and low-cardinality metrics;
- Relay candidate identity custody, token-only enrollment, signed activation/tunnel transcripts, bounded logical streams, actual owner claim, re-enrollment, revoke/disable fencing, and drain;
- same-origin Workspaces/Hosts/Credentials/Audit/Deployment navigation, bounded page-memory workspace tabs, explicit page-lifetime authentication, and durable unknown-outcome reconciliation;
- exact-Origin/first-frame attachment authentication, constrained OpenSSH, separate tmux client/server version and session probes, fresh explicit chooser, writer/read-only-observer control modes exposed as control/view-only UX, target-authoritative visible-pane layout, qualified final-capture/live cutover with continuous-token stress evidence, chunked binary-safe snapshots/live output, projection refresh, and xterm.js rendering;
- one route-scoped owner-local writer pointer across same-Machine tabs, serialized claim/takeover, session create/refresh/select, observed window/pane selection, bounded active-pane input, visible-writer automatic viewport resize with no rows/columns form, stale-writer fencing, and no replay of ambiguous mutations;
- exact-build/config symmetric membership, clustered configuration proof, private-CA internal TLS/WSS, fresh destination-challenge HMAC, one-hop remote attachment/invalidation routing, unreachable-owner behavior, protected ambiguous-commit observation, lease-fenced multi-node recovery, and cold API/configuration rotation evidence.

Release qualification is complete for the documented pre-release Linux x86_64 profiles: local and clustered owner paths, Ubuntu 22.04 tmux 3.2a, Debian 12 tmux 3.3a under `dash`, Debian 13 tmux 3.5a, checksum-pinned upstream tmux 3.7b, Chromium coverage of the terminal-first routes/page-memory tabs/automatic resize/same-page and mobile takeover, dependency audits, recovery exercises, and the production Server image. Version `0.0.1` was the initial tag-driven evaluation release. Version `0.0.2` added independently test-compiled Server and Relay crates.io source packages. Version `0.0.3` is the current evaluation release and adds the qualified terminal-first Web shell, bounded page-memory workspaces, same-Host tab coordination, and automatic visible-writer resize. No production-supported version or platform outside this explicit matrix is claimed.

## Design workflow

`spec/` is normative. Public docs distinguish current tested behavior from accepted target design. A capability becomes current behavior only after its reviewed versioned contract, durable invariants, implementation, and real acceptance evidence land together. CI runs production JavaScript audit, PostgreSQL integration, the single-node and two-node clustered Docker E2E, the production-image content smoke, and the complete Chromium/tmux matrix against Ubuntu 22.04 tmux 3.2a, Debian 12 tmux 3.3a with `dash`, Debian 13 tmux 3.5a, and checksum-pinned current upstream tmux 3.7b. CI also reports live RustSec advisory findings, but advisory-database changes alone do not block source/build or documentation delivery; supported findings are still triaged and fixed. The all-target RSA graph-absence constraint remains a hard gate, and the sole RustSec ignore is documented lock-only optional metadata absent from that graph.

Keep the central invariant visible in every change:

```text
OwlMux failure means attachment loss, never target tmux cleanup.
```

## Code boundaries

- Server and Relay are the only runtime crates.
- Do not create empty abstraction crates or split Gateway/Worker/scheduler services.
- PostgreSQL is the only durable product authority. Terminal bytes, current projection, Relay sockets, OpenSSH children, and Browser state stay bounded and owner-local.
- Relay state stays on the accepting owner node. Clustered Browser/API routing may use at most one internal owner WSS hop, while Relay enrollment/tunnel is never proxied.
- SSH private-key encryption is one fixed local XChaCha20-Poly1305 path; do not add a provider abstraction, KMS/HSM framework, KDF, or encryption-key-management surface.
- Documentation and development-image workflows run only after successful `main` CI and consume the exact qualified revision. CI builds and test-compiles the standalone Server and Relay source packages. Tag creation is the operator-controlled release boundary: the operator creates an immutable version tag only after the exact `main` commit has completed CI successfully. The tag-driven workflow intentionally does not query the Actions API; it trusts that pushed CI-qualified tag, checks repository/tag versions, publishes those exact packages to crates.io with checksum-safe rerun handling, embeds source revision metadata, and constructs archives, checksums, image tags, and release notes without repeating CI or source validation.

See [AGENTS.md](https://github.com/owlfoundry/owlmux/blob/main/AGENTS.md) for the complete repository guide.
