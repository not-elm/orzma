# Ozma Webview Protocol

> ozmux is in early development; this wire format is documented as it is today
> and may change between releases. The [SDKs](#sdks) track these changes for
> you — prefer them unless you are implementing your own client.

## Overview

The Ozma Webview protocol lets a local program running inside an ozmux pane
render webview content inline in the terminal and exchange messages with the
page. It spans three surfaces:

1. **The control socket** — a local Unix-socket connection over which a program
   registers content, manages it, and routes the page back-channel.
2. **OSC 5379** — terminal escape sequences that mount and unmount registered
   content at a cell rectangle.
3. **The `window.ozma` bridge** — an in-page JavaScript API the webview uses to
   call, subscribe to, and emit events to the registering program.

Three actors participate: the **registering program** (running in a pane), the
**ozmux host**, and the **webview page**. A registration is a *Tier 1* (dynamic,
runtime-registered) webview — the only kind this protocol describes.

End to end: a program connects to the control socket, registers content and
receives an opaque **handle**, writes an `OSC 5379;mount;<handle>;…` sequence to
display it, and then talks to the page through the `window.ozma` bridge routed
over the same control socket. Unmounting (or disconnecting) tears it down.

## Architecture at a glance

```text
 registering program              ozmux host                  webview page
 (inside an ozmux pane)
        │  reads $OZMA_SOCK / $OZMA_TOKEN from its env
        │  hello{token} ───────────────►│
        │  register{kind,…} ───────────►│
        │◄─────────────── {ok,handle} ──│
        │  OSC 5379;mount;handle;r;c ──►│  mount ozma://handle/ ───►│ load page
        │                               │◄──── window.ozma.call ────│
        │◄──── {op:call,reqId,method} ──│                           │
        │  {op:reply,reqId,value} ─────►│──── resolve Promise ─────►│
        │  {op:emit,event,payload} ────►│──── window.ozma.on ──────►│
        │◄──── {op:event,…} ◄ window.ozma.emit ─────────────────────│
        │  OSC 5379;unmount;handle ────►│  despawn webview ─────────►│
```

The control socket carries every horizontal arrow between the program and the
host; OSC 5379 carries the mount/unmount; the page bridge carries the
`window.ozma` arrows on the right.

## The control socket

### Transport

The control socket is a local Unix **stream** socket speaking **NDJSON**:
exactly one JSON object per line, terminated by `\n` (a trailing `\r` is
tolerated). Each line travels in one direction. The connection is long-lived —
it stays open for as long as the program wants its registrations to live.

### Discovery

ozmux injects two environment variables into every pane's PTY. A program reads
them from its own environment:

- `$OZMA_SOCK` — the absolute path to the control socket. Connect to this path
  verbatim; do not reconstruct it.
- `$OZMA_TOKEN` — the per-pane handshake token. Treat it as opaque (it is
  currently of the form `ozma:<bits>`, but do not parse it).

If either variable is absent, the program is not running inside an ozmux pane
and cannot use the protocol.

### Peer authentication

The host checks that the connecting peer's user id equals ozmux's own user id
and silently drops the connection otherwise. Only processes running as the same
user can connect.

### Handshake

The **first** line a program sends MUST be a `hello` carrying `$OZMA_TOKEN`:

```json
{"op":"hello","token":"ozma:4294967306"}
```

The token binds the connection to the pane it was issued for. If the first line
is not a valid `hello`, or the token does not resolve, the host closes the
connection without a reply. A second `hello` on an already-handshaked
connection is ignored.

### Reply vs. push

After the handshake, two kinds of line arrive **from** the host on the same
connection, and a client must tell them apart:

- A **register reply** is the only host line with **no `op` field** — it is
  either `{"ok":true,"handle":"…"}` or `{"ok":false,"error":"…"}`.
- Every **host-initiated push** (`call`, `event`, `compositing`) carries an
  `op` field.

So: a line with an `op` is a push; a line without one is the reply to your most
recent `register`. This is the one framing rule a from-scratch client must get
right.

### Register ordering

Registrations are processed one at a time per connection, and each
`register`'s reply arrives in request order. `register` has no request id —
correlation is positional. (The back-channel `call`/`reply` pair below uses an
explicit `reqId` instead.)
