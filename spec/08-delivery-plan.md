# Delivery Plan

## Purpose

This plan starts OwlMux from one clean, parentless foundation commit with no
compatibility or migration contract.

Delivery proceeds in substantial end-to-end blocks. Each capability starts from
durable organization/machine state and target-owned tmux, crosses Relay and
Server, and ends in a real browser journey with failure tests.

## Block 0: Clean Foundation

### Work

- replace repository history with one parentless OwlMux initial commit;
- rename every package, binary, document, container, GitHub link, and Worker;
- keep only the selected OwlMux crates, Web source, packaging, VitePress,
  infrastructure, scripts, and workflows;
- create honest placeholder `owlmux-server` and `owlmux-relay` crates;
- create one `apps/web` placeholder application;
- create VitePress docs and the `owlmux-docs` Workers deployment;
- add PostgreSQL and Redis development Compose infrastructure without claiming
  the placeholders use it yet;
- build a minimal production Server image;
- establish locked Rust/pnpm, formatting, tests, docs, Docker, and CI;
- add release workflows that publish only and do not rerun CI.

### Exit Gate

- no tracked file contains the old product name, repository URL, crate name, or
  runtime architecture;
- `make check`, `make test`, `make build`, docs build/dry-run, and Docker smoke
  pass from the new tree;
- Server serves only `/health`, `/ready`, and the placeholder Web application;
- Relay reports only its build/foundation status;
- public docs clearly distinguish the foundation from planned capability;
- the new Cloudflare Worker exists and GitHub `main` has one initial commit.

## Block 1: API-Key Relay Roaming Vertical Slice

This block proves the actual product with one built-in owner and one default
organization before adding multi-user identity.

### Durable Domain And Custody

- add PostgreSQL migrations for the built-in owner/default organization,
  machines, enrollments, Relay identity, encrypted SSH credential, browser
  sessions, and audit;
- add Redis-backed bounded authentication, enrollment, attachment, and Relay
  admission limits plus advisory reachability cache;
- implement one fixed-environment-root secret-custody provider and the statically
  composable interface;
- generate one dedicated SSH key pair and one one-use Relay enrollment token per
  machine;
- keep terminal input/output and tmux projection out of both stores.

### Relay And SSH Boundary

- implement `owlmux-relay enroll` with TLS trust, local Ed25519 identity,
  observed host identity, selected Unix account, and explicit OwlMux public-key
  installation;
- implement signed outbound Relay authentication and bounded stream multiplexing;
- restrict Relay local dialing to the enrolled loopback sshd endpoint;
- bridge one constrained Server OpenSSH client through one logical stream;
- verify the enrolled SSH host key and authenticate with the decrypted
  per-machine SSH key.

### tmux And Web Boundary

- implement supported-version tmux control parsing and typed command rendering
  from real fixtures;
- implement deterministic session/window/pane discovery and bounded hydration;
- implement the API-key browser exchange, default organization, machine
  registration, one-time enrollment display, machine list, and attachment API;
- build the graphical React/xterm.js workspace;
- implement bounded output, input, layout, resize, reconnect, and safe errors.

### End-To-End Gate

- a new installation can create a machine, enroll Relay, attach through SSH,
  start or select tmux, and continue after browser, WebSocket, SSH, Relay, or
  Server interruption;
- target tmux survives every OwlMux failure and restart case;
- enrollment replay, wrong machine, host-key mismatch, malformed tunnel/control
  frames, stale attachment epochs, ambiguous operations, slow browser, Redis
  flush, and PostgreSQL outage fail conservatively;
- root and SSH private keys never enter browser, logs, Redis, or plaintext
  PostgreSQL;
- the official binary exposes no root-key rotation or KMS management surface.

## Block 2: OwlAuth, Organizations, And Sharing

### Identity And Organization Domain

- integrate OwlAuth Project Auth through its supported SDK/Runtime contract;
- verify exact issuer, Project audience, OwlMux Application ID, JOSE type,
  EdDSA/JWKS, subject, session, and time claims;
- optionally use Project Server API introspection when configured;
- transactionally provision one local user, personal organization, and owner
  membership on first admission;
- add shared organization creation and owner/admin/member lifecycle;
- keep machine organization immutable and remove per-machine ACL entirely.

### Product Surface

- add organization selection, member management by stable admitted-user ID,
  organization machine registration, and role-aware controls;
- let every active member discover and attach to every active organization
  machine;
- reauthorize membership before API commands, WebSocket upgrade, SSH start, and
  periodically during live attachments;
- close only affected attachments on user, membership, organization, or machine
  disablement.

### End-To-End Gate

- API-key and OwlAuth modes are mutually exclusive and both pass real browser
  journeys;
- concurrent first OwlAuth login creates exactly one user/personal
  organization/owner membership;
- owner, admin, and member capabilities match the normative matrix;
- nonmembers cannot discover machine existence;
- membership removal closes OwlMux access but leaves target tmux and Relay
  process state alone;
- email, provider, organization, and custom OwlAuth claims never create local
  membership.

## Block 3: Direct SSH Route

Relay is the primary route. Direct SSH is added only after the complete Relay
journey proves the shared byte-stream boundary.

### Work

- add user registration for a Server-reachable machine address and selected Unix
  account;
- reuse generated per-machine SSH credentials and secret custody;
- require explicit public-key installation and SSH host-key fingerprint
  confirmation;
- support fixed operator-owned VPN, bastion, or `ProxyJump` profiles without
  browser-selected SSH options;
- feed the same OpenSSH/tmux attachment path used above Relay.

### Exit Gate

- direct and Relay routes observe the same target host identity and tmux behavior;
- route change cannot switch machine, account, or host key implicitly;
- direct-route failure has the same attachment-only semantics;
- browser input cannot select arbitrary network destinations or SSH options.

## Block 4: Production Release

### Hardening

- qualify exact Linux/macOS Relay, target tmux/OpenSSH, and desktop-browser
  matrices;
- harden subprocess, environment, filesystem, signal, shutdown, network egress,
  PostgreSQL, Redis, and secret-custody limits;
- add audit retention, metrics, readiness, backup, restore, and production
  diagnostics;
- qualify reverse proxy, TLS, CSP, WebSocket, and browser behavior;
- rehearse Server and Relay upgrade/rollback without target-process impact.

### Packaging

- publish Server and Relay archives plus the GHCR Server image;
- provide self-hosted Server, PostgreSQL, Redis, TLS, OwlAuth, API-key, secret
  root, Relay, SSH, backup, restore, and incident guidance;
- publish exact supported versions and release checksums;
- verify docs and application artifacts originate from the release commit.

### Exit Gate

- every supported platform/browser tuple passes the complete Relay journey;
- PostgreSQL backup/restore preserves organization, machine, credential envelope,
  session, and audit invariants without claiming tmux recovery;
- Redis can be flushed and rebuilt safely;
- security review has no unresolved critical findings;
- release publication consumes a CI-qualified commit and does not duplicate CI.

## Block 5: Native Client, Only If Still Valuable

After the Web product is stable, measure browser keyboard fidelity, latency,
local SSH configuration demand, and offline UI demand.

If justified, extract only already-reused tmux protocol/projection code and build
a native client that preserves organization authorization and target-owned tmux.
Do not create a desktop Web wrapper without a measured problem.

## Features Requiring A New Specification

The following do not enter opportunistically:

- per-machine ACL or private machine inside a shared organization;
- simultaneous multi-user input authority;
- email invitations, domain auto-join, SCIM, or external group synchronization;
- central terminal transcript, replay, or search;
- target-local session restoration across reboot;
- file transfer or filesystem browsing;
- arbitrary remote commands or user-selected shell profiles;
- generic reverse proxying, VPN, or P2P direct-path upgrade;
- multi-Server Relay coordination or distributed live-session ownership;
- hosted SaaS tenancy, billing, or control plane;
- official KMS integrations or online root-key rotation.

## Continuous Acceptance Invariant

Every delivered block preserves:

```text
Server, Relay, database, cache, or route failure
    => OwlMux attachment or reachability loss only
    != tmux session loss
    != target process cleanup
```

A change that makes target process continuity depend on OwlMux liveness requires
an explicit replacement of the product boundary, not an implementation shortcut.
