# Cloud SSH Specs

This directory contains design notes and implementation specs for Cloud SSH.

Cloud SSH is not a transparent SSH proxy. It is a collaborative terminal runtime:

> A collaborative terminal room that projects one real PTY to many clients.

## Specs

- [Collaborative Terminal Room](./001-collaborative-terminal-room.md)
- [Rust Runtime Architecture](./002-rust-runtime-architecture.md)
- [Room State And Protocol](./003-room-state-and-protocol.md)

## Design Principles

- The PTY is single-writer and stateful.
- The room is multi-client and collaborative.
- Each client owns an independent view.
- SSH is a client adapter, not the product boundary.
- Web is a client adapter, not a separate runtime.
- Room state may use collaborative conflict-resolution semantics.
- Terminal input is never CRDT-merged; it is serialized into one ordered byte stream.

## Current Implementation Direction

- Runtime language: Rust.
- Async runtime: Tokio.
- SSH adapter: Rust SSH server implementation.
- Web adapter: WebSocket-based browser terminal gateway.
- PTY backend: native PTY abstraction.
- Room CRDT state: Yjs-compatible Rust implementation through Yrs.
- Terminal screen state: VT parser plus canonical screen model.
- Terminal event persistence: append-only event log outside the CRDT document.
