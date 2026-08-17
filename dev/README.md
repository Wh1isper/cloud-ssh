# Development infrastructure

`dev/compose.yml` provides disposable local infrastructure:

- PostgreSQL 17 on loopback port `55433` by default;
- an opt-in Debian target fixture with target-owned tmux 3.5a and sshd listening only on the fixture's `127.0.0.1:22`;
- named target host-key and Relay-state volumes for deliberate local iteration.

`dev/server.env` contains intentionally public, disposable local values. Never reuse them for a real Deployment. Changing PostgreSQL initialization values does not rewrite an existing volume; use `make dev-reset` when a clean database is required.

Start only PostgreSQL and run the Server:

```bash
make dev-up
make dev-postgres
make dev
```

Start PostgreSQL plus the target fixture:

```bash
make dev-target-up
make dev-target-status
```

The target fixture does not install an OwlMux public key automatically. The target administrator boundary remains explicit: copy a generated public key into `/home/owlmux/.ssh/authorized_keys`, create a Machine with the fixture's Ed25519 host public key, and run the Relay inside the target network namespace.

The repeatable acceptance target performs that setup in an isolated Compose project, runs both versioned Node protocol clients and real Chromium, and removes its volumes afterward. The matrix target repeats the same path across the minimum, maintained distribution packages, `bash`/`dash`, and a checksum-pinned current upstream release:

```bash
make test-e2e
make test-e2e-matrix
```

It verifies clean Deployment initialization, enrollment-token disconnect recovery and replacement, signed Relay authentication, real OpenSSH proof, owner claim, credential retirement locking, explicit tmux chooser, target-authoritative two-pane projection, a four-pane continuous-token snapshot/live cutover stress case, Chromium/xterm rendering under the product CSP, bounded binary live output, same-control projection refresh, graceful route replacement, active re-enrollment, zero-session behavior, reload key clearing, PostgreSQL-triggered lease hard fencing, and target tmux survival. The compatibility matrix covers Ubuntu 22.04 tmux 3.2a, Debian 12 tmux 3.3a with `dash`, Debian 13 tmux 3.5a, and SHA-256-pinned upstream tmux 3.7b built by `dev/target-upstream.Dockerfile`.

Stop local infrastructure with:

```bash
make dev-down
```
