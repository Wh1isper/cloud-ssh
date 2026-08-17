# owlmux-server

Symmetric modular-monolith Server-node runtime for OwlMux.

The current single-node profile implements:

- immutable Deployment configuration, PostgreSQL migrations, fenced node leases, readiness, and graceful drain;
- one full-authority Deployment API key verified on every protected HTTP request and as the first attachment WebSocket frame;
- generated Ed25519 SSH credentials encrypted with the fixed XChaCha20-Poly1305 envelope;
- pending Machine and one-use enrollment-token management;
- Relay v1 enrollment, independent loopback-sshd proof, signed tunnel authentication, bounded stream multiplexing, and accepting-incarnation owner claims;
- constrained owner-local OpenSSH children with exclusive private-key runtime custody;
- separate tmux client/running-server probing, explicit session selection, and a persistent bounded read-only control-mode adapter;
- pause/continue/capture-plus-final-metadata cutover for every visible pane in the selected target-current window, post-capture topology revalidation, target-authoritative layout, bounded post-barrier buffering, chunked binary-safe snapshots/live output, and notification-driven projection refresh;
- static delivery of the React control plane and one read-only xterm.js instance per visible pane.

Clustered internal owner-WSS routing and writable terminal interaction are not implemented yet. The current published behavior is single-node only.
