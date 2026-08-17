# owlmux-relay

Target-side outbound reverse-connection client for OwlMux.

The Relay generates and atomically persists one permission-restricted Ed25519 candidate identity, reads enrollment tokens only from a no-echo prompt or bounded stdin, enrolls one fixed Machine against loopback `127.0.0.1:22`, authenticates signed Relay v1 tunnels, and multiplexes bounded SSH byte streams. It persists the accepted Deployment/Machine/route identity before setup so an ambiguous activation response can recover through an authenticated tunnel without token replay.

Relay never starts a shell, creates a PTY, invokes tmux, selects a target destination, modifies an account or SSH authorization store, receives the Deployment API key, or preserves a logical stream across reconnect.

## Enroll and run

After the target administrator installs the Server-generated public key for the configured account, enroll once. The command prints the exact public-key metadata returned by Server and requires an explicit readiness confirmation before proof begins:

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

`start` is a convenience command that enrolls when state is not active and then runs the tunnel. `--confirm-ready` is intended only for automation that has independently completed target authorization; it skips only the local human acknowledgment and does not install an authorization key. The independent Server-side SSH proof still runs and must succeed.

## Re-enroll

Active re-enrollment is deliberately explicit. First request re-enrollment through the protected Server API, which fences the current owner and returns the Machine to tokenless `Pending`. Then reset the Relay candidate identity, issue a new one-use token, enroll, and run again:

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
