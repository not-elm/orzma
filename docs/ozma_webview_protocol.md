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

### Program → host messages

Every program line carries an `op`:

| `op` | Fields | Meaning |
| --- | --- | --- |
| `hello` | `token` | Handshake; first line only. |
| `register` | `kind` + per-kind fields | Register content, mint a handle. |
| `unregister` | `handle` | Release a handle owned by this connection; despawns its mounted views. |
| `reply` | `reqId`, `ok`, `value?`, `error?` | Answer a host `call` (use the `call`'s `reqId`). |
| `emit` | `handle`, `event`, `payload` | Push an event to the handle's pages (delivered to `window.ozma.on`). |
| `focus` | `handle` (string or `null`), `instance` (string or `null`) | Set app-owned focus to a mounted view, or `null` to blur. |
| `navigate` | `handle`, `action` | Navigate a mounted view in place. |

`navigate.action` is one of the strings `"back"`, `"forward"`, `"reload"`, or
the object `{"to":"<http(s) url>"}` (`to` is valid only on a `url` view).

### Register kinds

`register` carries a `kind` discriminator and its fields:

| `kind` | Required | Optional (default) | Served at |
| --- | --- | --- | --- |
| `dir` | `root` (absolute dir path), `entry` (safe relative path, e.g. `index.html`) | `interactive` (`true`), `forward_keys` (`[]`), `preload` (`[]`) | `ozma://<handle>/` |
| `inline` | `html` (full document, ≤ 4 MiB) | `interactive` (`true`), `forward_keys` (`[]`), `preload` (`[]`) | `ozma://<handle>/index.html` |
| `url` | `url` (`http`/`https` only) | `interactive` (`true`), `bridge` (`false`), `forward_keys` (`[]`), `preload` (`[]`) | the remote URL directly (no `ozma://` origin) |

- `interactive` — whether the mounted view accepts pointer/keyboard input.
- `bridge` (`url` only) — opt into the `window.ozma` back-channel. `dir` and
  `inline` are always bridged; a `url` view is bridged only with `bridge:true`.
- `preload` — an array of JavaScript source strings injected before the page's
  own scripts (after the host bridge). Honored only for bridged views.

### Forward keys

`forward_keys` lists key chords the host passes through to the pane's PTY
instead of letting the focused webview consume them. Each chord is:

```json
{"mods":["alt"],"key":"h"}
```

`mods` is any subset of `"alt"`, `"ctrl"`, `"shift"`, `"meta"`. `key` is one of:
a lowercase letter `a`–`z`, a digit `0`–`9`, `tab`, `backtab`, `f1`–`f12`,
`esc`, `" "` (space), `up`, `down`, `pageup`, `pagedown`. Unrecognized chords
are silently ignored.

### Host → program messages

Every host push carries an `op`:

| `op` | Fields | Meaning / response |
| --- | --- | --- |
| `call` | `handle`, `reqId`, `method`, `params` | A page `window.ozma.call(method, params)`. Respond with a `reply` carrying the same `reqId`. |
| `event` | `handle`, `event`, `payload` | A page `window.ozma.emit(event, payload)`. Fire-and-forget; no response. |
| `compositing` | `handle`, `active` (bool) | The view first composited (`true`) or was unmounted after compositing (`false`). |

Two directional details that are easy to get wrong:

- **`emit` vs. `event`.** A page's `window.ozma.emit(name, …)` arrives at the
  program as `op:"event"`. A program's own `emit` message (`op:"emit"`) is
  delivered to pages' `window.ozma.on(name, …)`. Same idea ("named event"), two
  `op` values depending on direction.
- **`urlChanged`.** For a `url` view, the host reports top-level address changes
  as an `op:"call"` with `method:"urlChanged"` and `params:{"url":"<new>"}`.
  Despite the `call` shape it is fire-and-forget — any `reply` is discarded. Use
  it to track page-driven navigation.

### Register reply & error codes

A successful `register` replies `{"ok":true,"handle":"<handle>"}`. A rejected one
replies `{"ok":false,"error":"<code>"}`:

| `error` | Cause |
| --- | --- |
| `invalid_root` | `dir.root` is not an absolute path to an existing directory. |
| `unsafe_entry` | `dir.entry` is empty, absolute, or contains `..`/`.`. |
| `html_too_large` | `inline.html` exceeds 4 MiB. |
| `invalid_url` | `url.url` does not parse or has no host. |
| `unsupported_scheme` | `url.url` is not `http`/`https`. |
| `internal` | The host failed to process the request. |

### Handle semantics

A handle is opaque, unique per registration, lowercase, and matches
`^[a-z0-9._-]{1,128}$` (a subset of the OSC `view_id` charset, so a handle is
always a valid `mount` argument). Treat it as a token: do not parse it. Each
handle owns one isolated `ozma://<handle>/` origin.

### Example exchange

Program-to-host lines are marked `C→S`, host-to-program lines `S→C`:

```json
C→S {"op":"hello","token":"ozma:4294967306"}
C→S {"op":"register","kind":"inline","html":"<!doctype html><body>hi</body>"}
S→C {"ok":true,"handle":"nf2k7q9w3x1m5a8b0c4d6e7f"}
S→C {"op":"call","handle":"nf2k7q9w3x1m5a8b0c4d6e7f","reqId":"0","method":"save","params":{"text":"hi"}}
C→S {"op":"reply","reqId":"0","ok":true,"value":{"saved":true}}
C→S {"op":"emit","handle":"nf2k7q9w3x1m5a8b0c4d6e7f","event":"tick","payload":{"n":1}}
```
