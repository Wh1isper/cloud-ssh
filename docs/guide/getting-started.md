# Getting started

OwlMux is currently a foundation, not a usable terminal roaming product. This
guide builds the placeholder Web application and Rust binaries and lets you
inspect the repository boundary without implying that Relay, SSH, or tmux works.

## Prerequisites

- stable Rust from `rust-toolchain.toml`;
- Node.js 24;
- pnpm 11.20.0;
- Docker with Compose v2 for PostgreSQL/Redis and image checks.

## Install locked dependencies

```bash
make install
```

## Validate the foundation

```bash
make check
make test
make build
```

`make check` formats and lints Rust/Web sources and builds the docs. `make test`
runs Rust and Web tests. `make build` builds the placeholder Web artifact plus
`owlmux-server` and `owlmux-relay` release binaries.

## Run the placeholder Server

```bash
make dev
```

Open `http://127.0.0.1:8080`. The current page explains the product direction.
The only implemented service endpoints are:

```text
GET /health
GET /ready
```

Requests under `/api/` and `/auth/` return an explicit foundation error rather
than falling back to the SPA.

## Inspect the Relay placeholder

```bash
cargo run --locked --package owlmux-relay -- --help
```

It reports that enrollment and reverse transport are not implemented. It does
not create local state or network connections.

## Development infrastructure

```bash
make dev-up
make dev-status
make dev-down
```

The Compose file starts PostgreSQL and Redis for future delivery blocks. The
foundation Server does not connect to them yet.

## Read the target design

Start with the [architecture guide](architecture.md), then review the normative
[specifications](https://github.com/owlfoundry/owlmux/tree/main/spec). Capability
is considered implemented only after its delivery block and end-to-end gate pass.
