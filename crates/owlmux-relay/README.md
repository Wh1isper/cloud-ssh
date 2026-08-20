# owlmux-relay

Target-side outbound reverse-connection client for OwlMux.

The Relay generates and atomically persists one permission-restricted Ed25519 candidate identity, reads enrollment tokens only from a no-echo prompt or bounded stdin, enrolls one fixed Machine against loopback `127.0.0.1:22`, authenticates signed Relay v1 tunnels, and multiplexes bounded SSH byte streams. It persists the accepted Deployment/Machine/route identity before setup so an ambiguous activation response can recover through an authenticated tunnel without token replay.

Relay never starts a shell, creates a PTY, invokes tmux, selects a target destination, modifies an account or SSH authorization store, receives the Deployment API key, or preserves a logical stream across reconnect.

## Install from crates.io

Install the Relay binary from the published source package with the locked dependency graph recorded in that package:

```bash
cargo install --locked owlmux-relay
```

The matching portable binary archive is also attached to each OwlMux GitHub release.

## Enroll and run

After the target administrator installs the Server-generated credential public key for the configured account, enroll once. The command prints the exact credential metadata returned by Server and requires an explicit authorization-readiness confirmation. On a Machine's first enrollment, Server then discovers the Ed25519 host key through constrained OpenSSH; Relay canonicalizes the Ed25519 key, locally recomputes and checks its SHA-256 fingerprint, prints that derived value with the standard SSH-style authenticity prompt, and continues only when the operator enters exact `yes` with no surrounding whitespace:

```bash
owlmux-relay enroll \
  --server wss://owlmux.example \
  --state /var/lib/owlmux/state.json \
  --account target-user
```

Then run the authenticated tunnel from the protected persisted state:

```bash
owlmux-relay run \
  --server wss://owlmux.example \
  --state /var/lib/owlmux/state.json
```

`start` is a convenience command that enrolls when state is not active and then runs the tunnel. `--confirm-ready` is intended only for automation that has independently completed target authorization; it skips only the local credential-installation acknowledgment and does not install an authorization key. Automated first enrollment must also pass `--expected-host-key-sha256 SHA256:...`; Relay requires an exact fingerprint match and offers no unconditional acceptance flag. After host confirmation, an independent strict Server-side SSH proof runs. First activation atomically pins the confirmed host key only if that proof succeeds.

## Re-enroll

Active re-enrollment is deliberately explicit. First request re-enrollment through the protected Server API, which fences the current owner and returns the Machine to tokenless `Pending` while retaining its pinned host key. Then reset the Relay candidate identity, issue a new one-use token, enroll, and run again. Re-enrollment skips host discovery and fails closed unless strict SSH verification sees the same key:

```bash
owlmux-relay reset --state /var/lib/owlmux/state.json
owlmux-relay enroll \
  --server wss://owlmux.example \
  --state /var/lib/owlmux/state.json \
  --account target-user
owlmux-relay run \
  --server wss://owlmux.example \
  --state /var/lib/owlmux/state.json
```

Reset does not issue a token and must not be used as a substitute for the Server-side re-enrollment transition.
