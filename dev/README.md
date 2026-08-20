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

The target fixture does not install an OwlMux public key automatically. The target administrator boundary remains explicit: copy a generated credential public key into `/home/owlmux/.ssh/authorized_keys`, create a Machine without supplying a host key, and run the Relay inside the target network namespace. First enrollment discovers the fixture's Ed25519 host key, requires exact interactive confirmation or an exact expected SHA-256 fingerprint in automation, proves account access on a separate strict stream, and pins the confirmed key only on activation.

The repeatable acceptance target performs that setup in an isolated Compose project, runs versioned attachment protocol clients, and removes its volumes afterward. The opt-in matrix repeats the target compatibility path across the minimum, maintained distribution packages, `bash`/`dash`, and a checksum-pinned current upstream release:

```bash
make test-e2e
make test-e2e-matrix
```

It verifies Deployment initialization, enrollment recovery, signed Relay authentication, real OpenSSH proof, owner claim, credential locking, tmux projection and writer operations, route replacement, hard fencing, and target tmux survival. The compatibility matrix covers Ubuntu 22.04 tmux 3.2a, Debian 12 tmux 3.3a with `dash`, Debian 13 tmux 3.5a, and SHA-256-pinned upstream tmux 3.7b built by `dev/target-upstream.Dockerfile`.

Stop local infrastructure with:

```bash
make dev-down
```
