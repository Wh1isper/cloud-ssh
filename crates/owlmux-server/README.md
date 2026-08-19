# owlmux-server

Symmetric modular-monolith Server-node runtime for OwlMux.

The current single-node and clustered profiles implement:

- immutable Deployment configuration, PostgreSQL migrations, fenced node leases, readiness, and graceful drain;
- one full-authority Deployment API key verified on every protected HTTP request and as the first attachment WebSocket frame;
- generated Ed25519 SSH credentials encrypted with the fixed XChaCha20-Poly1305 envelope;
- complete bounded Machine alias, enrollment cancellation, disable/re-enroll/Relay-revoke, and active-credential-rebind lifecycle with owner-routed invalidation and protected ambiguous-commit observation;
- safe bounded audit presentation, low-cardinality operational counters, public request limits, and separate authenticated mutation admission;
- Relay v1 enrollment, independent loopback-sshd proof, signed tunnel authentication, bounded stream multiplexing, and accepting-incarnation owner claims;
- constrained owner-local OpenSSH children with exclusive private-key runtime custody;
- separate tmux client/running-server probing, explicit session selection/creation, atomic read-only `ignore-size` control-mode attach, and persistent bounded writer/observer adapters;
- cancellation-safe bounded control records, pause/continue/capture-plus-final-metadata cutover for every visible pane in the selected target-current window, post-capture topology revalidation, target-authoritative layout, bounded post-barrier buffering, chunked binary-safe snapshots/live output, and coalesced notification-driven projection refresh;
- one owner-local current-writer pointer with a shared target dispatch/hydration barrier, ordered tmux read-only/size-participation transitions, authoritative resize, closed window/pane selection, bounded literal pane input, direct pre/post-I/O route fencing, and conservative ambiguous-outcome recovery;
- static delivery of the same-origin terminal-first React shell with Host management, bounded page-memory workspace tabs, automatic visible-writer viewport resize, and one xterm.js instance per visible pane, with input enabled only for the current writer's active pane;
- exact-build/config symmetric node membership, private-CA Rustls WSS, fresh destination-challenge cluster authentication, one-hop remote-owner attachment/control routing, `owner_unreachable` handling, and lease-fenced owner-loss recovery.

The default `single-node` profile uses the same owner application boundary without an internal hop. The explicit `clustered` profile requires one complete internal TLS configuration and a distinct shared cluster key.

## Install from crates.io

Install the Server binary from the published source package with the locked dependency graph recorded in that package:

```bash
cargo install --locked owlmux-server
```

The crate contains the embedded PostgreSQL migrations and generated protocol bindings, but it does not embed the React application. Set `OWLMUX_WEB_DIR` to the matching Web assets from the same OwlMux GitHub release. The qualified Server image or release archive remains the recommended complete deployment artifact.

## Release archive

The official Server archive contains the binary, the exact qualified Web build in `web/`, reviewed contracts, embedded-migration source files, and `SOURCE_REVISION`. Set `OWLMUX_WEB_DIR` to the extracted `web/` directory when running the archive outside the production container. Other Deployment, PostgreSQL, secret, TLS, and runtime-root configuration remains explicit as documented in the deployment guide.
