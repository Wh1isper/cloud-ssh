# OwlMux Web

React control plane and interactive terminal workspace for the OwlMux pre-release single-node and clustered profiles.

The Browser:

- keeps the single Deployment API key only in current page memory and sends it on every protected request;
- provides in-memory `/login`, `/machines`, `/machines/{machine_id}`, and `/ssh-credentials` journeys for generated credentials, complete Machine/Relay lifecycle controls, active-Machine credential rebind, safe audit events, and durable unknown-outcome reconciliation;
- authenticates each attachment WebSocket with the exact Origin and one bounded `auth.api_key` first frame;
- always starts an attachment at an explicit tmux session chooser;
- validates the closed attachment v1 protocol and atomically installs chunked snapshots for every visible pane in the selected session's target-current window;
- renders each pane with an xterm.js instance using target-authoritative coordinates and dimensions;
- accepts bounded binary-safe live output while dropping stale workspace epochs;
- rejects dimensions above 10,000, projections above 2,000,000 pane cells, more than 32 pending operations, more than 8,192 pending input bytes, or a WebSocket buffered amount above 65,536 bytes, and releases pending budgets whenever the attachment/selection epoch is replaced;
- serializes writer claim/takeover and exposes closed session create/refresh/select, observed window/pane selection, active-pane input, and writer resize operations;
- keeps observer stdin disabled, updates an existing xterm renderer in place across role changes, bounds and confirms paste, and never queues or retries an excess or ambiguous target mutation;
- treats a typed ambiguity, interrupted mutation response, malformed mutation response, or untyped mutation timeout as an unknown durable outcome, disables every mutation control, and requires a successful explicit summary refresh before another mutation.

The API key is never stored in cookies, Web Storage, IndexedDB, Cache Storage, a service worker, a URL, or serialized application state. Reload, logout, HTTP 401, or an attachment `unauthenticated` frame disposes the shared client, closes its sockets/requests, clears the candidate, and requires re-entry.

The Browser always uses the one Deployment origin. A Server ingress may route an already authenticated attachment to the current remote owner, but the Browser never receives an internal endpoint, node identity, cluster credential, or a second-hop protocol.

From the repository root:

```bash
pnpm --filter @owlmux/web check
pnpm --filter @owlmux/web test
pnpm --filter @owlmux/web build
```

`make test-e2e` also launches real Chromium through Playwright and verifies the served product CSP/security headers, safe XSS-sensitive text rendering, authenticated navigation, mutation-transport ambiguity lock/refresh, explicit logout, target-authoritative multi-pane rendering, writer input, observer geometry, takeover without renderer rollback, authoritative resize, target-observed read-only/writer client flags, stale-writer rejection, absence of unexpected Browser runtime errors, and reload-time API-key clearing against the live Relay/OpenSSH/tmux stack.
