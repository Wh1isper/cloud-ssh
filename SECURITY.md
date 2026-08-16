# Security Policy

## Supported versions

OwlMux is pre-release foundation software. No version is currently supported for
production terminal access.

## Reporting a vulnerability

Please report vulnerabilities privately through GitHub Security Advisories for
[`owlfoundry/owlmux`](https://github.com/owlfoundry/owlmux/security/advisories/new).
If that is unavailable, contact `jizhongsheng957@gmail.com`.

Do not open a public issue for an undisclosed vulnerability. Include affected
commit/version, reproduction steps, impact, and any suggested mitigation. Avoid
including real credentials, terminal contents, SSH keys, or personal data.

## Current scope

The current foundation exposes only placeholder static assets plus `/health` and `/ready`; Server clustering, product authentication, Relay transport, SSH, and tmux integration are not implemented. The target threat model is normative in [`spec/08-operations-security-and-resilience.md`](spec/08-operations-security-and-resilience.md), with deployment access in [`spec/05-deployment-access-and-authentication.md`](spec/05-deployment-access-and-authentication.md) and private-key encryption in [`spec/06-storage-consistency-and-private-key-encryption.md`](spec/06-storage-consistency-and-private-key-encryption.md).
