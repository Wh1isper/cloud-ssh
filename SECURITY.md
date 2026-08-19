# Security policy

## Supported versions

OwlMux is pre-release software. Versions `0.0.1` through `0.0.3` are published for evaluation only; no version is currently supported for production terminal access.

## Reporting a vulnerability

Please report vulnerabilities privately through GitHub Security Advisories for [`owlfoundry/owlmux`](https://github.com/owlfoundry/owlmux/security/advisories/new). If that is unavailable, contact `jizhongsheng957@gmail.com`.

Do not open a public issue for an undisclosed vulnerability. Include the affected commit/version, reproduction steps, impact, and any suggested mitigation. Avoid including real credentials, terminal contents, SSH keys, or personal data.

## Current scope

The pre-release single-node and clustered profiles implement Deployment API-key authentication, generated encrypted SSH credentials, complete Machine/Relay lifecycle controls, active-Machine credential rebind, signed Relay transport, accepting-incarnation owner claims, constrained OpenSSH, target-authoritative multi-pane tmux projection, owner-local Browser writer coordination, safe durable audit and low-cardinality metrics, fresh challenge-authenticated internal TLS/WSS routing between simultaneously Serving symmetric nodes, and repeatable failure/recovery evidence. Repository qualification covers the documented Linux x86_64/tmux/login-shell matrix, local and remote-owner failure paths, dependency review, and production Server image. Version `0.0.1` was the initial tag-driven evaluation release, `0.0.2` added Server and Relay crates.io source packages, and `0.0.3` adds the qualified terminal-first Web shell, bounded page-memory workspaces, and automatic visible-writer resize; no production-supported version has been released, and platforms outside that explicit matrix are not qualified.

The central invariant is that Browser, Server, Relay, PostgreSQL, and network failure may close OwlMux access but must not terminate target tmux or its processes. The current and target threat boundaries are documented in [`docs/guide/security.md`](docs/guide/security.md) and specified normatively in [`spec/08-operations-security-and-resilience.md`](spec/08-operations-security-and-resilience.md), with Deployment access in [`spec/05-deployment-access-and-authentication.md`](spec/05-deployment-access-and-authentication.md) and private-key encryption in [`spec/06-storage-consistency-and-private-key-encryption.md`](spec/06-storage-consistency-and-private-key-encryption.md).
