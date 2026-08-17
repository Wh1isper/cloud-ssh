# Getting started

OwlMux currently implements Blocks 0–3 in the single-node profile. You can initialize one Deployment, authenticate the Browser control plane, manage generated SSH credentials and Machines, enroll one target Relay, claim the actual Machine owner, choose a target tmux session explicitly, and render every visible pane in that session's target-current window as one read-only live projection. Writable interaction and clustered remote-owner routing are not implemented yet.

## Prerequisites

- stable Rust from `rust-toolchain.toml`;
- Node.js 24;
- pnpm 11.20.0;
- Docker with Compose v2;
- OpenSSH client for a host-run Server.

## Install locked dependencies

```bash
make install
```

## Validate the implementation

```bash
make check
make test
make build
make test-e2e
```

`make check` verifies generated contracts, formats and lints Rust/Web sources, builds docs, and validates Compose. `make test` runs Rust and Web tests. `make build` builds the Web artifact plus both release binaries. `make test-e2e` creates isolated PostgreSQL and target containers and uses versioned Node WebSocket clients plus headless Chromium to prove enrollment recovery, credential locking, owner claim, OpenSSH, target-authoritative two-pane tmux/xterm rendering under the product CSP, continuous four-pane snapshot/live cutover, binary live output, projection refresh, route replacement, active re-enrollment, zero-session recovery, reload key clearing, lease hard fencing, and target tmux survival. `make test-e2e-matrix` repeats that complete path against Ubuntu 22.04 tmux 3.2a, Debian 12 tmux 3.3a under `dash`, Debian 13 tmux 3.5a, and checksum-pinned current upstream tmux 3.7b; CI runs the matrix.

## Run the Server

```bash
make dev
```

Open `http://127.0.0.1:8080` and enter the disposable API key from `dev/server.env`. That key remains only in the current page memory. The Server initializes PostgreSQL, creates the default generated Ed25519 credential, registers one fenced node incarnation, and exposes the protected control plane.

Start PostgreSQL separately when needed:

```bash
make dev-up
make dev-status
```

## Run the target fixture

The opt-in target contains sshd bound only to its own loopback address and target-owned tmux:

```bash
make dev-target-up
make dev-target-status
```

The fixture intentionally starts with no authorized OwlMux key. Use the control plane to copy the selected generated public key, install it into the target's `authorized_keys` as the target administrator, create a Machine with the target Ed25519 host public key, and copy the one-use enrollment token.

Relay reads the token from a no-echo prompt or bounded stdin and stores its identity in a mode-`0600` state file. After the target administrator installs the displayed Server-generated public key, enroll once and explicitly confirm that authorization is ready:

```bash
owlmux-relay enroll \
  --server ws://host.docker.internal:8080 \
  --state /var/lib/owlmux/state.json \
  --account owlmux
```

Then run the authenticated tunnel:

```bash
owlmux-relay run \
  --server ws://host.docker.internal:8080 \
  --state /var/lib/owlmux/state.json
```

`owlmux-relay start` combines those steps for first use and runs directly from already active state. Use `--confirm-ready` only in automation that has independently installed the displayed key; the flag skips only the local human acknowledgment and does not mutate target authorization. The independent Server-side SSH proof still runs and must succeed. Active re-enrollment requires the protected Server-side re-enrollment action first, followed by `owlmux-relay reset`, explicit new-token issuance, `enroll`, and `run`.

For production use `wss://`, a protected persistent state directory, and a target-local Relay process. The Relay endpoint is fixed to `127.0.0.1:22`; Browser input cannot choose SSH destinations, options, identities, commands, or tmux syntax.

## Current attachment behavior

An attachment upgrade requires the exact configured Origin and a bounded `auth.api_key` first frame. Only after authentication does the Server resolve the local owner, open a Relay stream, materialize the current credential in an exclusive child directory, run constrained OpenSSH, verify the target host/account, and probe tmux.

Every attachment stops at a fresh chooser, even for zero or one session. Selecting an observed session ID plus creation time starts a read-only `tmux -C` client. Server pauses that client while observing the target-current window and visible panes, then continues each pane and runs one synchronous capture-plus-final-metadata command list as its per-pane snapshot/live cutover. It accepts the two consecutive guarded responses under one deadline, retries if pane output separates capture from metadata, uses that final cursor/mode metadata to construct the bootstrap, and revalidates the complete topology afterward. Output already covered by that final capture is discarded; post-capture output is buffered within fixed bounds while projection metadata and binary-safe snapshot chunks reach the Browser. Browser validates and atomically installs the complete target-authoritative layout only at the final ready phase, after which buffered and new binary live output reaches one read-only xterm.js instance per pane. Layout, pane, window, or session notifications trigger a fresh projection epoch. A graceful Relay tunnel replacement returns an existing attachment to a fresh chooser rather than silently reselecting a session.

Detach, Relay failure, Server shutdown, database loss, or Browser close removes OwlMux access only; it does not stop target tmux.

## Stop development infrastructure

```bash
make dev-down
```

Read [Relay and roaming](relay.md) for the trust and recovery model and [Architecture](architecture.md) for the complete target design.
