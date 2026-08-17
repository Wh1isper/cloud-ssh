# Security policy

## Supported versions

OwlMux is pre-release software. No version is currently supported for production terminal access.

## Reporting a vulnerability

Please report vulnerabilities privately through GitHub Security Advisories for [`owlfoundry/owlmux`](https://github.com/owlfoundry/owlmux/security/advisories/new). If that is unavailable, contact `jizhongsheng957@gmail.com`.

Do not open a public issue for an undisclosed vulnerability. Include the affected commit/version, reproduction steps, impact, and any suggested mitigation. Avoid including real credentials, terminal contents, SSH keys, or personal data.

## Current scope

Blocks 0–3 implement the pre-release single-node profile: Deployment API-key authentication, generated encrypted SSH credentials, Machine and Relay enrollment controls, signed Relay transport, accepting-incarnation owner claims, constrained OpenSSH, and a target-authoritative read-only multi-pane tmux projection. Writable Browser interaction, writer takeover, clustered internal owner WSS, and multiple simultaneously Serving nodes are not implemented.

The central invariant is that Browser, Server, Relay, PostgreSQL, and network failure may close OwlMux access but must not terminate target tmux or its processes. The current and target threat boundaries are documented in [`docs/guide/security.md`](docs/guide/security.md) and specified normatively in [`spec/08-operations-security-and-resilience.md`](spec/08-operations-security-and-resilience.md), with Deployment access in [`spec/05-deployment-access-and-authentication.md`](spec/05-deployment-access-and-authentication.md) and private-key encryption in [`spec/06-storage-consistency-and-private-key-encryption.md`](spec/06-storage-consistency-and-private-key-encryption.md).
