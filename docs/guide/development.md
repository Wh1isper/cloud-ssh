# Development

## Repository layout

```text
apps/web/             React and Vite placeholder application
crates/owlmux-server/ public Server placeholder
crates/owlmux-relay/  target-side Relay placeholder
dev/                  PostgreSQL and Redis Compose infrastructure
docs/                 VitePress documentation and Workers configuration
spec/                 normative target product specifications
```

## Commands

```bash
make install       # install locked Cargo and pnpm dependencies
make format        # format Rust and Web sources
make check         # lint, type-check, build Web/docs, and dry-run docs deploy
make test          # run Rust and Web tests
make build         # build Web plus both release binaries
make dev           # build Web and run the placeholder Server
make docs          # run VitePress locally
make docker-build  # build and smoke-test the Server image
```

## Design workflow

`spec/` is normative. Public docs can discuss accepted target design only when it
is clearly labeled. A capability becomes current behavior after its delivery
block and real end-to-end acceptance gate pass.

Keep the central invariant visible in every change:

```text
OwlMux failure means attachment loss, never target tmux cleanup.
```

## Code boundaries

- Server and Relay are the only runtime crates in the foundation.
- Do not create empty abstraction crates before real sharing exists.
- Web source belongs under `apps/web`.
- PostgreSQL is durable authority and Redis is disposable cache once implemented.
- Secret custody is statically composed; do not add a dynamic KMS framework.
- Release workflows publish CI-qualified commits and do not rerun CI.

See [AGENTS.md](https://github.com/owlfoundry/owlmux/blob/main/AGENTS.md) for the
complete repository guide.
