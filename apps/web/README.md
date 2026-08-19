# OwlMux Web

Same-origin terminal-first React application shell and interactive workspace for the OwlMux pre-release single-node and clustered profiles.

The Browser:

- keeps the single Deployment API key only in current page memory and sends it on every protected request;
- enters `/workspaces` after `/login` and provides same-origin `/hosts`, `/hosts/new`, `/hosts/{id}`, `/ssh-credentials`, `/audit`, and `/deployment` journeys;
- calls durable Machine resources Hosts in the product UI while keeping Machine vocabulary in API/domain/contracts;
- provides generated credential management, complete Machine/Relay lifecycle controls, active-Machine credential rebind, immutable target-scope detail, safe audit events, and durable unknown-outcome reconciliation;
- holds at most 16 workspace tabs in current page memory, with one independent Attachment per tab; internal navigation preserves them, one tab close detaches only that Attachment, and page-lifetime end clears all of them;
- authenticates each attachment WebSocket with the exact Origin and one bounded `auth.api_key` first frame;
- always starts an attachment at an explicit tmux session chooser;
- validates the closed attachment v1 protocol and atomically installs chunked snapshots for every visible pane in the selected session's target-current window;
- renders each pane with an xterm.js instance using target-authoritative coordinates and dimensions;
- accepts bounded binary-safe live output while dropping stale workspace epochs;
- rejects dimensions above 10,000, projections above 2,000,000 pane cells, more than 32 pending operations, more than 8,192 pending input bytes, or a WebSocket buffered amount above 65,536 bytes, and releases pending budgets whenever the attachment/selection epoch is replaced;
- serializes the one per-Machine-route writer pointer across same-page and cross-page attachments and exposes it as `Take control`, `Take over`, `You have control`, and `View only`;
- exposes closed session create/refresh/select, observed window/pane selection, active-pane input, and writer resize operations;
- provides no rows/columns form: only the current visible ready writer derives dimensions from its pane viewport and xterm cell size, then sends bounded, debounced, deduplicated resize while target-authoritative replacement projection remains final;
- keeps observer stdin disabled, prevents hidden/observer tabs from changing target geometry, updates an existing xterm renderer in place across role changes, bounds and confirms paste, and never queues or retries an excess or ambiguous target mutation;
- treats a typed ambiguity, interrupted mutation response, malformed mutation response, or untyped mutation timeout as an unknown durable outcome, disables every mutation control, and requires a successful explicit summary refresh before another mutation.

The API key and workspace tabs are never stored in cookies, Web Storage, IndexedDB, Cache Storage, a service worker, a URL, or serialized application state. Internal SPA navigation preserves current page memory. Reload, page close, navigation away, logout, HTTP 401, or an attachment `unauthenticated` frame disposes the shared client, closes its sockets/requests, clears tabs and the key candidate, and requires re-entry.

The Browser always uses the one Deployment origin. A Server ingress may route an already authenticated attachment to the current remote owner, but the Browser never receives an internal endpoint, node identity, cluster credential, or a second-hop protocol.

From the repository root:

```bash
pnpm --filter @owlmux/web check
pnpm --filter @owlmux/web test
pnpm --filter @owlmux/web build
```

`make test-e2e` exercises the live attachment protocol against real PostgreSQL, Relay, OpenSSH, and target tmux without browser automation.
