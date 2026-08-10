# OwlMux Repository Guide

## Language

- Communicate with the user in Chinese.
- Code, public APIs, file names, commit messages, and documentation are English.
- Do not use emoji in responses, code, comments, documentation, commits, or
  release notes.

## Product Boundary

OwlMux is a self-hosted terminal roaming gateway built on SSH and target-owned
tmux.

The non-negotiable invariant is:

```text
Server, Relay, database, cache, browser, or network failure
    => OwlMux attachment or reachability loss only
    != target tmux session loss
    != target process cleanup
```

- Target tmux alone owns sessions, windows, panes, PTYs, scrollback, layouts, and
  child-process lifetime.
- Target sshd owns SSH host identity and Unix-account authentication.
- `owlmux-server` is the public Web/API service, SSH/tmux client, organization
  authority, and Relay stream router.
- `owlmux-relay` is a target-side outbound reverse-connection client. It forwards
  bounded SSH streams only to enrolled loopback sshd and never starts a shell,
  creates a PTY, invokes tmux, or manages a process.
- Browser and Server projections are ephemeral and reconstructible from tmux.
- PostgreSQL is durable OwlMux authority. Redis is required disposable cache and
  rate-limit infrastructure. Neither stores or owns terminal state.

The current repository is Block 0 foundation only. Do not describe planned
authentication, organizations, Relay, SSH, or tmux behavior as implemented.

## Authentication And Authorization

- A deployment uses OwlAuth mode or API-key mode, never credential fallback.
- OwlAuth integrates through Project Auth, not downstream OIDC. Verify exact
  issuer, Project audience, OwlMux Application ID, JOSE type, EdDSA/JWKS,
  subject, session, and time claims.
- API-key mode uses one explicit high-entropy `OWLMUX_API_KEY` for a built-in
  owner/default organization; do not add local passwords or an OAuth server.
- OwlMux owns users, organizations, roles, memberships, and machine ownership.
  OwlAuth claims never create membership.
- First OwlAuth admission transactionally creates one local user, personal
  organization, and owner membership.
- Every active member can access every active organization machine. Do not add a
  per-machine ACL or private machine inside a shared organization.

## Secret Custody

- Recoverable SSH private keys cross one small statically composed interface.
- The official provider uses one fixed high-entropy `OWLMUX_SECRET_ROOT_KEY`
  environment value and context-bound authenticated encryption.
- Do not add a KMS SDK, remote custody protocol, dynamic plugin, key management
  UI, multi-root fallback, or online root-key rotation.
- Operators needing KMS/HSM custody implement the interface and compile their own
  Server.
- Non-recoverable enrollment/session values are stored as digests, not encrypted.

## Source Map

- `spec/` — normative target product and architecture;
- `docs/` — public VitePress guidance; clearly separate current from planned;
- `crates/owlmux-server/` — public Server runtime;
- `crates/owlmux-relay/` — target-side Relay runtime;
- `apps/web/` — React/Vite Web source;
- `dev/` — local PostgreSQL and Redis infrastructure;
- `.github/workflows/` — CI, docs deployment, image publication, and release.

Do not create extra crates until implemented sharing proves a concrete ownership
boundary. In particular, do not pre-create core, protocol, domain, storage, SDK,
CLI, or key-provider abstractions.

## Documentation Workflow

`spec/` is normative and may lead implementation. Public `docs/` may describe
accepted direction only with an explicit target-design/foundation label.

When changing product boundaries, repository structure, commands, CI, deployment,
or planned component responsibilities, review and update:

- `spec/*`;
- `docs/*`;
- `README.md`;
- `AGENTS.md`;
- `Cargo.toml` and crate manifests;
- `package.json`, workspace config, and lockfiles;
- `Makefile`, Dockerfile, Compose, and workflows.

Use Mermaid for diagrams. Do not add mdBook or duplicate public docs systems.

## Development Workflow

Install locked dependencies:

```bash
make install
```

After code or configuration changes, run:

```bash
make check
make test
make build
```

For the Server image:

```bash
make docker-build
```

For PostgreSQL and Redis development services:

```bash
make dev-up
make dev-status
make dev-down
```

## Coding Conventions

- Keep the foundation minimal and honest; do not add fake product surfaces.
- Work bottom-up from durable organization/machine constraints through Relay/SSH,
  tmux projection, public protocol, UI, and real end-to-end tests.
- Treat OpenSSH as the initial constrained SSH client. Use a dedicated
  Server-owned config, explicit host-key inputs, `IdentitiesOnly`, and no ambient
  agent for target authentication. Browser input never chooses SSH options,
  destinations, identities, remote commands, or tmux syntax.
- Keep tmux operations closed and typed; pane input is the only literal byte path.
- Do not persist terminal input, output, scrollback, or tmux projection.
- Do not retry ambiguous terminal input or mutating tmux operations.
- Database/cache/auth revocation closes OwlMux access only; it never cleans up
  target processes.
- CI is the validation authority. Release workflows publish CI-qualified commits
  and must not duplicate lint or tests.
- Use function-style tests and explicit, bounded failure semantics.
