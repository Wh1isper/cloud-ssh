# tmux Control And Roaming

## Decision

OwlMux integrates with the target's existing tmux server through tmux control
mode. It does not scrape a normal terminal, emulate a tmux server, or maintain a
second session model.

The Server projects target-owned sessions, windows, panes, layouts, and pane
output into typed browser events. Every projection is scoped to one ephemeral
attachment and can be discarded and rebuilt from the target.

## Compatibility Boundary

A target must provide:

- a supported OpenSSH server;
- a POSIX user account authorized for the Server's SSH credential;
- tmux with the control-mode behavior covered by OwlMux fixtures and end-to-end
  tests;
- a writable tmux socket available to that SSH account.

The first release must publish an exact minimum tmux version after fixtures pass
against the chosen Linux and macOS test matrix. OwlMux rejects unsupported or
unparseable versions before opening a writable workspace. It does not guess
protocol behavior from a version string alone.

A custom tmux socket name or path may be selected only by trusted target
configuration. Browser input cannot choose a socket path, server command, shell,
or environment.

## Target Identity And tmux Identity

OwlMux scopes observed tmux identifiers to:

```text
MachineInstance {
    machine_id,
    verified_ssh_host_identity,
    tmux_socket_identity,
    attachment_epoch,
}
```

Session names and tmux numeric IDs are not globally unique and are not durable
OwlMux resource IDs. They may be reused after tmux restart. A browser operation
must carry the current attachment epoch and the exact observed tmux identifier;
a value from an older attachment is rejected locally and never rendered into a
command.

OwlMux does not assign a `room_id`, `runtime_generation_id`, canonical output
sequence, or durable pane ID.

## Attachment Startup

A new attachment performs these stages:

1. establish an authorized direct or Relay-backed SSH route;
2. verify the configured target host identity;
3. authenticate the configured target Unix account;
4. probe tmux availability and compatibility with a fixed bounded command;
5. start a new tmux control-mode client without a local interactive PTY;
6. query the target session, window, pane, client, and layout graph;
7. capture bounded current content for panes admitted to the initial workspace;
8. install one complete browser projection;
9. forward later control notifications and pane output.

The Server does not create a tmux session merely because a browser connects. If
no session exists, it returns an empty target state. Session creation requires an
explicit user operation.

## Projection Model

The normalized live projection contains only observed target state:

```text
TmuxProjection {
    attachment_epoch,
    target,
    tmux_version,
    sessions[],
    windows[],
    panes[],
    selected_session?,
    selected_window?,
    active_panes[],
    layout_revision,
}
```

Each session includes its observed tmux ID, name, attached-client count, window
references, and safe status fields. Each window and pane includes its observed
ID, parent references, active flags, dimensions, layout position, title, current
command presentation, and safe process metadata exposed by the allowlisted tmux
formats.

OwlMux never treats command presentation, pane title, current path, or process
name as trusted markup or executable input.

## Control-Mode Parsing

The parser is an incremental byte parser with explicit limits for:

- one control line;
- decoded pane-output bytes per notification;
- command-response block bytes and lines;
- pending command count;
- identifiers and names;
- projection resource count;
- partial-line buffering;
- total queued browser output.

It recognizes only the control notifications required by the supported profile.
An unknown notification is either explicitly ignorable under a tested compatible
rule or fails the attachment. It never silently reinterprets unknown syntax as
pane output or command completion.

Pane output escaping is decoded exactly once into bytes. UTF-8 is not assumed.
The browser receives bytes and xterm.js owns terminal decoding and rendering.
Malformed escapes, response nesting faults, impossible identifiers, or resource
limits close the attachment without sending cleanup to tmux.

## Command Correlation

The Server serializes mutating tmux commands per control client unless a tested
control-mode guarantee permits bounded concurrency. Each issued operation has a
browser request ID and an internal pending-command entry. tmux response markers
complete that exact operation with success or a sanitized failure.

An operation timeout or SSH loss has an ambiguous result if tmux may already have
executed it. OwlMux does not blindly retry. It reconnects or refreshes the
projection and reports the observed target state.

Read-only discovery commands may be retried only after a fresh command boundary.
Input bytes are never automatically replayed after an unknown outcome.

## Typed Operations

The initial browser can request only closed typed operations:

### Sessions

- create a session with a validated name and configured default startup command;
- rename an observed session;
- select or detach from an observed session;
- destroy an observed session after explicit confirmation.

### Windows

- create a window using the configured default command;
- rename, select, move, or close an observed window.

### Panes

- split an observed pane using a supported direction and bounded size;
- select, resize, or close an observed pane;
- send bounded input bytes to an observed pane.

### Client projection

- update the current control client's terminal dimensions;
- request a complete projection refresh;
- request bounded pane-history hydration.

The browser never submits raw tmux syntax, format expressions, shell commands,
socket selectors, environment assignments, target IDs, or command-line options.
Names reject control characters and obey explicit byte limits. Numeric IDs must
exactly match the current projection.

The first release uses one operator-configured default shell/start command. A
future command-profile feature requires a separate typed allowlist; arbitrary
browser command execution is not introduced accidentally through session or
window creation.

## Pane Input

Pane input is an opaque bounded byte string tied to the current attachment epoch
and observed pane ID. The Server sends it through the tmux operation designed
for literal input bytes and never interpolates it into a shell or tmux command
string.

The first product is personal roaming. It does not implement an OwlMux authority
holder, CRDT, shared cursor, or multi-writer merge. If the same account attaches
through multiple tmux clients, tmux defines the resulting behavior. OwlMux may
show other attached-client count but does not claim exclusive input ownership.

## Layout And Resize

Target tmux is authoritative for pane layout and dimensions. The browser renders
the latest observed layout and sends typed resize intent for the current control
client or pane operation.

Browser CSS dimensions are local presentation. They do not become a durable
canonical size. Debounced resize commands are bounded and superseded locally
before dispatch where safe. A dispatched resize with an ambiguous result is
resolved through a target refresh rather than replayed blindly.

## Hydration And Reconnection

Hydration reconstructs display state from tmux rather than an OwlMux journal.
For each admitted pane, the Server obtains bounded target scrollback and current
pane dimensions, initializes the browser terminal, then follows live output.

The implementation must prove a deterministic cutover between captured content
and later control-mode output for its supported tmux profile. The test suite must
exercise output written:

- before capture starts;
- while capture is in progress;
- at command-completion boundaries;
- immediately after live forwarding begins;
- during SSH disconnect and reattachment.

If exact byte-contiguous reconstruction cannot be proven for a tmux behavior,
OwlMux resets and redraws the pane from a fresh bounded capture and reports that
intermediate live output was rehydrated rather than inventing a durable sequence.
The primary promise is continued target process interaction, not terminal replay.

Browser reload, Server restart, SSH loss, and Relay loss all use the same
fresh hydration path. No resume cursor is persisted centrally.

## Backpressure

Each attachment has byte- and count-bounded queues between:

- OpenSSH stdout and the tmux parser;
- parsed pane events and the WebSocket writer;
- browser operations and the tmux command writer;
- Relay logical streams where applicable.

A slow browser may first receive a bounded warning. If it cannot keep up, OwlMux
closes only that attachment. It never blocks another attachment indefinitely and
never kills or pauses the target tmux session as a backpressure response.

## Terminal Exit And Detach

Closing a browser workspace closes the local control client and SSH process. It
must use tmux client detach semantics and must not kill the selected session,
window, pane, or child process.

Destructive tmux operations exist only as explicit typed user commands with a
confirmation boundary. Server shutdown never sends them.

## Acceptance Criteria

- Real tmux fixtures cover every accepted control notification, escape rule,
  response marker, and limit.
- A process continues while every OwlMux attachment is closed.
- Reattachment discovers the original session, windows, panes, and continued
  output from target tmux.
- No stale attachment epoch or unobserved tmux ID can reach command rendering.
- Browser text cannot become raw tmux syntax, a format expression, shell input,
  socket path, or SSH option except through the bounded literal pane-input path.
- An ambiguous mutating command or input outcome is never automatically replayed.
- Slow-browser handling closes only the attachment and never changes target tmux
  lifecycle.
