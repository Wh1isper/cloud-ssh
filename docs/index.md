---
layout: home

title: OwlMux
titleTemplate: Terminal roaming with target-owned tmux

hero:
  name: OwlMux
  text: Return to the tmux session that never left your machine
  tagline: A self-hosted Web workspace and outbound Relay for reconnecting to target-owned terminal sessions.
  actions:
    - theme: brand
      text: Understand the architecture
      link: /guide/architecture
    - theme: alt
      text: Develop OwlMux
      link: /guide/getting-started
    - theme: alt
      text: View on GitHub
      link: https://github.com/owlfoundry/owlmux

features:
  - title: Target-owned continuity
    details: tmux on the target machine owns session, pane, PTY, scrollback, and child-process lifetime. OwlMux never replaces that boundary.
  - title: Web roaming
    details: OwlMux Server will reconstruct a graphical session, window, and pane workspace from live tmux state after every attachment.
  - title: Outbound Relay
    details: OwlMux Relay will connect outward to the public Server and carry an ordinary SSH connection back to target sshd.
  - title: Organization sharing
    details: Each user receives a personal organization, while shared organizations make every organization machine available to every active member.
  - title: OwlAuth or one API key
    details: A deployment can use OwlAuth Project Auth for multiple users or one deployment API key for a single built-in owner.
  - title: Self-hosted only
    details: One OwlMux Server, PostgreSQL, Redis, and user-installed Relays form one standalone trust domain with no SaaS control plane.
---

::: warning Foundation status
OwlMux currently provides a clean repository foundation: two placeholder Rust
binaries, a placeholder Web page, PostgreSQL and Redis development infrastructure,
Docker packaging, specifications, and CI. Authentication, organizations, machine
registration, Relay transport, SSH, and tmux integration are not implemented yet.
:::

## The durable boundary

```mermaid
flowchart LR
    browser["Browser"] --> server["Public OwlMux Server"]
    relay["Target-side OwlMux Relay"] -->|"outbound connection"| server
    server -->|"SSH through Relay"| sshd["Target sshd"]
    sshd --> tmux["Target-owned tmux"]
    tmux --> process["Shell or coding agent"]
```

Closing the browser or losing Server/Relay connectivity must affect only the
attachment. The target tmux session and its process continue independently.

## Read next

- [Getting started](/guide/getting-started) — build and inspect the current
  foundation.
- [Architecture](/guide/architecture) — understand target-owned tmux and the
  Server/Relay split.
- [Authentication and organizations](/guide/authentication) — review the accepted
  OwlAuth/API-key and sharing model.
- [Relay and roaming](/guide/relay) — understand the planned reverse connection
  and tmux hydration path.
- [Deployment](/guide/deployment) — current container and infrastructure shape.
- [Security](/guide/security) — trust boundaries that apply now and to planned
  capabilities.

The complete target design is normative under the repository
[`spec/`](https://github.com/owlfoundry/owlmux/tree/main/spec).
