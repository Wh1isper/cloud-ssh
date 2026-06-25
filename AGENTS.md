# Repository Guidelines

## Language

- Communication with the user should be in Chinese.
- Code, public APIs, file names, commit messages, and documentation should be in English.
- Do not use emoji in responses, code, comments, documentation, commits, or release notes.

## Repository Overview

Cloud SSH is a Rust workspace for building a collaborative terminal room runtime.

The product boundary is the room, not SSH:

- `RealPty` owns one process tree, one input byte stream, one output byte stream, and one canonical size.
- `Room` owns collaboration state, input authority, resize policy, screen snapshots, and event persistence.
- `ClientView` owns each SSH or browser client's viewport, scroll state, and local projection.

Current workspace members:

- `crates/cloud-ssh-core` - shared IDs and terminal room model types.
- `crates/cloud-ssh-server` - initial server binary and future adapter host.

Planned areas live in `spec/` until their responsibilities and validation paths are clear.

## Documentation Workflow

Use `docs/` for user-facing guides and examples. Use `spec/` for product and architecture decisions.

When adding, removing, or renaming docs pages, update:

- `docs/SUMMARY.md`
- `docs/nav.json`
- `README.md` if the docs entry point changes

Prefer mermaid diagrams for architecture flows.

## Spec Workflow

Specs should stay concise and reviewable. Keep the key decision, runtime boundary, and acceptance criteria clear.

After changing repository structure, workspace boundaries, command behavior, CI, or planned module responsibilities, review and update:

- `docs/*`
- `spec/*`
- `README.md`
- `AGENTS.md`
- `Cargo.toml`
- crate manifests under `crates/*/Cargo.toml`
- `Makefile`
- `.pre-commit-config.yaml`
- `.github/workflows/*.yml`

## Development Workflow

After changing code, run:

```bash
make fmt-check
make check
make test
```

For full local validation, run:

```bash
make ci
```

For repository-wide hooks, run:

```bash
make lint
```

## Coding Conventions

- Keep early abstractions minimal and add crates only when ownership boundaries become concrete.
- Keep workspace metadata consistent across `Cargo.toml`, crate manifests, `Makefile`, `.pre-commit-config.yaml`, and `.github/workflows/ci.yml`.
- Treat SSH and WebSocket as adapters over the room runtime.
- Do not wire client channels directly to the PTY.
- Do not CRDT-merge terminal input. Serialize accepted PTY input through room authority.
- Do not store raw PTY output in the CRDT document. Store it in append-only event logs and snapshots.
