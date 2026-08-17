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
  - title: Read-only Web roaming
    details: The current single-node profile explicitly selects a live target session and renders every visible pane in that session's target-current window as one bounded read-only xterm.js projection.
  - title: Outbound Relay
    details: OwlMux Relay connects outward through one Deployment origin and carries bounded logical SSH streams back to enrolled loopback sshd.
  - title: One Deployment API key
    details: One memory-only Browser/API key grants complete access within one independent self-hosted trust domain.
  - title: 'Target: symmetric horizontal scale'
    details: The accepted clustered design lets ordinary load balancing send each new Relay to an accepting Server that becomes its fenced owner; Browser/API routing then uses at most one owner WSS hop.
  - title: Simple fenced recovery
    details: Node loss closes OwlMux access; after the old owner lease expires, a new Relay ingress may claim a higher Machine epoch and reconstruct from target tmux without migration or rebalance.
---

::: info Current single-node scope
Blocks 0–3 are implemented and Docker-qualified: Deployment API-key access, generated encrypted SSH credentials, Machine and enrollment-token control, signed Relay tunnels, actual owner claims, constrained OpenSSH, explicit tmux session choice, and a target-authoritative read-only xterm.js projection of every visible pane in that session's target-current window through real tmux control mode. Writable interaction and clustered internal owner-WSS routing remain target design.
:::

## Target topology and durable boundary

The diagram below shows the accepted clustered topology. The current Blocks 0–3 profile uses one Server node and no internal owner WSS.

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

- [Getting started](/guide/getting-started) — run and validate the current single-node implementation.
- [Architecture](/guide/architecture) — understand target-owned tmux, symmetric Server nodes, and fenced Machine ownership.
- [Deployment access and credentials](/guide/authentication) — review the single API key, cluster authentication, integration metadata, and SSH credential model.
- [Relay and roaming](/guide/relay) — understand current enrollment, Relay-ingress ownership, and read-only tmux hydration plus later target scope.
- [Deployment](/guide/deployment) — compare the current image with the target one-or-more-node topology.
- [Security](/guide/security) — review Deployment, cluster, target, and Browser trust boundaries.

The complete target design is normative under the repository [`spec/`](https://github.com/owlfoundry/owlmux/tree/main/spec).
