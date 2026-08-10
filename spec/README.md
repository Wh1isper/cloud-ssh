# OwlMux Specifications

`spec/` is the normative authority for the accepted OwlMux target product and
architecture. It may lead implementation. Public `docs/` must distinguish the
current foundation from planned capabilities.

## Product Invariant

Target tmux owns every session, pane PTY, and child process. OwlMux owns only
roaming attachments and connectivity. Browser, Server, SSH, and Relay
failure must not terminate target tmux.

## Specification Map

1. [Product Boundary](01-product-boundary.md) — product promise, durable object,
   authentication modes, trust boundary, and non-goals.
2. [System Architecture](02-system-architecture.md) — component and state
   ownership, attachment lifecycle, storage, and failure semantics.
3. [tmux Control And Roaming](03-tmux-control-and-roaming.md) — control-mode
   compatibility, projection, typed operations, hydration, and backpressure.
4. [Web Application And Browser Protocol](04-web-application-and-protocol.md) —
   HTTP/WebSocket surfaces, browser sessions, protocol messages, and UI.
5. [Connectivity And Relay](05-connectivity-and-relay.md) — direct SSH,
   outbound reverse relay, Relay identity, streams, and route safety.
6. [Identity, Authorization, And Security](06-identity-authorization-and-security.md)
   — OwlAuth, deployment API key, organizations, memberships, secret custody,
   credentials, threats, and audit.
7. [Technology Decisions](07-technology-decisions.md) — repository, Rust, Web,
   OpenSSH, tmux, docs, container, CI, and release choices.
8. [Delivery Plan](08-delivery-plan.md) — clean foundation and end-to-end product
   blocks.

## Current Repository State

The repository is being reset to Block 0: placeholder `owlmux-server` and
`owlmux-relay` binaries, one placeholder Web application, PostgreSQL/Redis
development infrastructure, VitePress documentation, Docker packaging, and CI.
Authentication, SSH, tmux control mode, roaming, and Relay behavior described
here are target design until their delivery block passes its acceptance gate.

## Review Rule

A specification change must keep terminology and ownership consistent across this
set. In particular, do not introduce:

- an OwlMux-owned terminal session, room, PTY, process generation, or lifecycle
  lease;
- a Relay command that starts or stops target work;
- a browser path that bypasses target SSH authentication;
- an OwlAuth claim that silently creates organization membership or machine
  access;
- a release capability claim before its real end-to-end gate passes.
