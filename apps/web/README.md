# OwlMux Web

React control plane and read-only terminal workspace for OwlMux Blocks 0–3.

The Browser:

- keeps the single Deployment API key only in current page memory and sends it on every protected request;
- manages generated SSH credential metadata, Machines, and enrollment controls through the versioned HTTP API;
- authenticates each attachment WebSocket with the exact Origin and one bounded `auth.api_key` first frame;
- always starts an attachment at an explicit tmux session chooser;
- validates the closed attachment v1 protocol and atomically installs chunked snapshots for every visible pane in the selected session's target-current window;
- renders each pane with a read-only xterm.js instance using target-authoritative coordinates and dimensions;
- accepts bounded binary-safe live output while dropping stale workspace epochs.

The API key is never stored in cookies, Web Storage, IndexedDB, Cache Storage, a service worker, a URL, or serialized application state. Reload and logout clear the candidate and require re-entry.

Writable terminal input, tmux mutations, writer takeover, and clustered remote-owner routing are not implemented yet.

From the repository root:

```bash
pnpm --filter @owlmux/web check
pnpm --filter @owlmux/web test
pnpm --filter @owlmux/web build
```

`make test-e2e` also launches real Chromium through Playwright and verifies the served product CSP, two-pane xterm snapshot and geometry, absence of Browser runtime errors, and reload-time API-key clearing against the live Relay/OpenSSH/tmux stack.
