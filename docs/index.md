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
    details: OwlMux will reconstruct a graphical session, window, and pane workspace from live tmux state after every attachment.
  - title: Outbound Relay
    details: OwlMux Relay will connect outward through one Deployment origin and carry an ordinary SSH connection back to target sshd.
  - title: One Deployment API key
    details: One memory-only Browser/API key grants complete access within one independent self-hosted trust domain.
  - title: Symmetric horizontal scale
    details: Ordinary load balancing sends each new Relay to an accepting Server that becomes its fenced owner; Browser/API routing uses at most one owner WSS hop.
  - title: Simple fenced recovery
    details: Node loss closes OwlMux access; after the old owner lease expires, a new Relay ingress may claim a higher Machine epoch and reconstruct from target tmux without migration or rebalance.
---

::: warning Foundation status
OwlMux currently provides a clean repository foundation: two placeholder Rust binaries, a placeholder Web page, PostgreSQL development infrastructure, Docker packaging, specifications, and CI. Deployment API-key access, Server-node membership and ownership, Machine registration, Relay transport, SSH, and tmux integration are not implemented yet.
:::

## The durable boundary

```mermaid
flowchart LR
    browser["Browser"] --> origin["One Deployment origin"]
    relay["Target-side OwlMux Relay"] -->|"outbound connection"| origin
    origin --> ingress["TLS ingress"]
    ingress --> nodeA["Server node A"]
    ingress --> nodeB["Server node B"]
    nodeA --> postgres[("PostgreSQL")]
    nodeB --> postgres
    nodeA <-->|"Browser/API owner WSS only"| nodeB
    nodeB -->|"owner-local SSH through Relay"| relay
    relay --> sshd["Target sshd"]
    sshd --> tmux["Target-owned tmux"]
    tmux --> process["Shell or coding agent"]
```

Closing the browser or losing an ingress node, owner node, Relay, database, or network path must affect only OwlMux access. The target tmux session and its process continue independently. A replacement Relay ingress may claim a fresh Machine owner epoch only after no valid owner remains, then observes current tmux state rather than moving or replaying live state. If a valid owner is unreachable, the operator fences that node and waits for lease expiry.

## Read next

- [Getting started](/guide/getting-started) — build and inspect the current foundation.
- [Architecture](/guide/architecture) — understand target-owned tmux, symmetric Server nodes, and fenced Machine ownership.
- [Deployment access and credentials](/guide/authentication) — review the single API key, cluster authentication, integration metadata, and SSH credential model.
- [Relay and roaming](/guide/relay) — understand the planned reverse connection, Relay-ingress ownership, and tmux hydration path.
- [Deployment](/guide/deployment) — compare the current image with the target one-or-more-node topology.
- [Security](/guide/security) — review Deployment, cluster, target, and Browser trust boundaries.

The complete target design is normative under the repository [`spec/`](https://github.com/owlfoundry/owlmux/tree/main/spec).
