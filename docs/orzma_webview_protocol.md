# Orzma Webview Protocol

> orzma is in early development; this wire format is documented as it is today
> and may change between releases. The [SDKs](#sdks) track these changes for
> you — prefer them unless you are implementing your own client.

## Overview

The Orzma Webview protocol lets a local program running inside an orzma pane
render webview content inline in the terminal and exchange messages with the
page. It spans three surfaces:

1. **The control socket** — a local Unix-socket connection over which a program
   registers content, manages it, and routes the page back-channel.
2. **OSC 5379** — terminal escape sequences that mount and unmount registered
   content at a cell rectangle.
3. **The `window.orzma` bridge** — an in-page JavaScript API the webview uses to
   call, subscribe to, and emit events to the registering program.

Three actors participate: the **registering program** (running in a pane), the
**orzma host**, and the **webview page**. A registration is a *Tier 1* (dynamic,
runtime-registered) webview — the only kind this protocol describes.

End to end: a program connects to the control socket, registers content and
receives an opaque **handle**, writes an `OSC 5379;mount;<handle>;…` sequence to
display it, and then talks to the page through the `window.orzma` bridge routed
over the same control socket. Unmounting (or disconnecting) tears it down.

## Architecture at a glance

```text
 registering program              orzma host                  webview page
 (inside an orzma pane)
        │  reads $ORZMA_SOCK / $ORZMA_TOKEN from its env
        │  hello{token} ───────────────►│
        │  register{kind,…} ───────────►│
        │◄─────────────── {ok,handle} ──│
        │  OSC 5379;mount;handle;r;c ──►│  mount orzma://handle/ ───►│ load page
        │                               │◄──── window.orzma.call ────│
        │◄──── {op:call,reqId,method} ──│                           │
        │  {op:reply,reqId,value} ─────►│──── resolve Promise ─────►│
        │  {op:emit,event,payload} ────►│──── window.orzma.on ──────►│
        │◄──── {op:event,…} ◄ window.orzma.emit ─────────────────────│
        │  OSC 5379;unmount;handle ────►│  remove webview ──────────►│
```

The control socket carries every horizontal arrow between the program and the
host; OSC 5379 carries the mount/unmount; the page bridge carries the
`window.orzma` arrows on the right.

## The control socket

### Transport

The control socket is a local Unix **stream** socket speaking **NDJSON**:
exactly one JSON object per line, terminated by `\n` (a trailing `\r` is
tolerated). Each line travels in one direction. The connection is long-lived —
it stays open for as long as the program wants its registrations to live.

### Discovery

orzma injects two environment variables into every pane's PTY. A program reads
them from its own environment:

- `$ORZMA_SOCK` — the absolute path to the control socket. Connect to this path
  verbatim; do not reconstruct it.
- `$ORZMA_TOKEN` — the per-pane handshake token. Treat it as opaque (it is
  currently of the form `orzma:<bits>`, but do not parse it).

If either variable is absent, the program is not running inside an orzma pane
and cannot use the protocol.

### Peer authentication

The host checks that the connecting peer's user id equals orzma's own user id
and silently drops the connection otherwise. Only processes running as the same
user can connect.

### Handshake

The **first** line a program sends MUST be a `hello` carrying `$ORZMA_TOKEN`:

```json
{"op":"hello","token":"orzma:4294967306"}
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
| `unregister` | `handle` | Release a handle owned by this connection; removes its mounted views. |
| `reply` | `reqId`, `ok`, `value?`, `error?` | Answer a host `call` (use the `call`'s `reqId`). |
| `emit` | `handle`, `event`, `payload` | Push an event to the handle's pages (delivered to `window.orzma.on`). |
| `focus` | `handle` (string or `null`), `instance` (string or `null`) | Set app-owned focus to a mounted view, or `null` to blur. |
| `navigate` | `handle`, `action` | Navigate a mounted view in place. |

`navigate.action` is one of the strings `"back"`, `"forward"`, `"reload"`, or
the object `{"to":"<http(s) url>"}` (`to` is valid only on a `url` view).

### Register kinds

`register` carries a `kind` discriminator and its fields:

| `kind` | Required | Optional (default) | Served at |
| --- | --- | --- | --- |
| `dir` | `root` (absolute dir path), `entry` (safe relative path, e.g. `index.html`) | `interactive` (`true`), `forward_keys` (`[]`), `preload` (`[]`) | `orzma://<handle>/` |
| `inline` | `html` (full document, ≤ 4 MiB) | `interactive` (`true`), `forward_keys` (`[]`), `preload` (`[]`) | `orzma://<handle>/index.html` |
| `url` | `url` (`http`/`https` only) | `interactive` (`true`), `bridge` (`false`), `forward_keys` (`[]`), `preload` (`[]`) | the remote URL directly (no `orzma://` origin) |

- `interactive` — whether the mounted view accepts pointer/keyboard input.
- `bridge` (`url` only) — opt into the `window.orzma` back-channel. `dir` and
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
| `call` | `handle`, `reqId`, `method`, `params` | A page `window.orzma.call(method, params)`. Respond with a `reply` carrying the same `reqId`. |
| `event` | `handle`, `event`, `payload` | A page `window.orzma.emit(event, payload)`. Fire-and-forget; no response. |
| `compositing` | `handle`, `active` (bool) | The view first composited (`true`) or was unmounted after compositing (`false`). |

Two directional details that are easy to get wrong:

- **`emit` vs. `event`.** A page's `window.orzma.emit(name, …)` arrives at the
  program as `op:"event"`. A program's own `emit` message (`op:"emit"`) is
  delivered to pages' `window.orzma.on(name, …)`. Same idea ("named event"), two
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
handle owns one isolated `orzma://<handle>/` origin.

### Example exchange

Program-to-host lines are marked `C→S`, host-to-program lines `S→C`:

```json
C→S {"op":"hello","token":"orzma:4294967306"}
C→S {"op":"register","kind":"inline","html":"<!doctype html><body>hi</body>"}
S→C {"ok":true,"handle":"nf2k7q5w3x3m5a6b2c4d6e7f"}
S→C {"op":"call","handle":"nf2k7q5w3x3m5a6b2c4d6e7f","reqId":"0","method":"save","params":{"text":"hi"}}
C→S {"op":"reply","reqId":"0","ok":true,"value":{"saved":true}}
C→S {"op":"emit","handle":"nf2k7q5w3x3m5a6b2c4d6e7f","event":"tick","payload":{"n":1}}
```

## OSC 5379 — mount / unmount

Once a handle is registered, the program mounts it by writing an OSC 5379 escape
sequence to its terminal. The sequence is framed `ESC ] 5379 ; <params> ST`,
where `ST` (string terminator) is `ESC \` or `BEL`. In raw bytes:

```text
mount:    \x1b]5379;mount;<view_id>;<rows>;<cols>\x1b\
unmount:  \x1b]5379;unmount;<view_id>\x1b\
```

### mount

```text
OSC 5379 ; mount ; <view_id> ; <rows> ; <cols> [ ; <instance_id> ] ST
```

- `view_id` — the handle from `register`; charset `^[A-Za-z0-9._-]{1,128}$`.
- `rows` — decimal `1`–`200`. `cols` — decimal `1`–`400`. Digits only, no sign.
- `instance_id` — optional, same charset as `view_id`. It lets one handle mount
  several independent placements. A trailing empty field (`mount;<id>;3;20;`) is
  malformed.

The view occupies a `rows`×`cols` rectangle of terminal cells, inline at the
cursor.

### unmount

```text
OSC 5379 ; unmount [ ; <view_id> [ ; <instance_id> ] ] ST
```

- No `view_id` → unmount all of this program's inline views on the terminal.
- `view_id` only → unmount that handle's default instance.
- `view_id` + `instance_id` → unmount that specific placement.

An `instance_id` is addressable only alongside a `view_id`.

### Ownership and malformed sequences

A `mount;<handle>` takes effect only in the pane whose `$ORZMA_TOKEN` registered
that handle — a program mounts its own handles in its own pane. Any malformed
sequence (bad charset, out-of-range dimensions, empty fields) is silently
dropped; the host reports no error.

### Example

Mount handle `nf2k7q5w3x3m5a6b2c4d6e7f` as a 24×80 view, then unmount it:

```text
\x1b]5379;mount;nf2k7q5w3x3m5a6b2c4d6e7f;24;80\x1b\
\x1b]5379;unmount;nf2k7q5w3x3m5a6b2c4d6e7f\x1b\
```

## The `orzma://` origin

`dir` and `inline` registrations are served from a per-handle origin
`orzma://<handle>/`. A request for an empty path resolves to `index.html`. The
scheme is standard, secure, CORS-enabled, fetch-enabled, and display-isolated,
so normal `fetch`, ES modules, and same-origin requests work within the
handle's origin. Each handle is its own isolated origin.

- **`dir`** — files are served from the registered `root`. Requests that escape
  the root — a `..` or `.` path component, an absolute path, or their
  percent-encoded forms — are rejected; each file is capped at 64 MiB; the
  content type is inferred from the file extension.
- **`inline`** — the single registered document is served only at `index.html`;
  any subresource request returns 404. Use `dir` for multi-file content.
- **`url`** — the remote `http(s)` page is loaded directly and has **no**
  `orzma://` origin.

## The `window.orzma` bridge

Bridged webviews expose a frozen `window.orzma` object to page scripts.
`dir` and `inline` views are always bridged; a `url` view is bridged only when
registered with `bridge:true`. A page should feature-detect before using it.

### API

| Method | Returns | Meaning |
| --- | --- | --- |
| `call(method, params?)` | `Promise` | Invoke a program method; resolves with the program's `reply` value, rejects with `Error(error)`. |
| `on(event, handler)` | `void` | Subscribe to a program `emit`. |
| `off(event, handler)` | `void` | Remove a handler by reference. |
| `emit(event, payload?)` | `void` | Send a one-way event to the program (arrives as `op:"event"`). |

A `call` has **no client-side timeout** — if the program never replies, the
Promise stays pending. The host injects a rejection when it cannot route the
call: `no_owner` (the view has no registering connection), `owner_unavailable`
(the connection's writer is gone), or `owner_disconnected` (the program
disconnected with the call in flight).

### Binary round-trip

A **top-level** `Uint8Array` round-trips through the bridge — it is tagged
`{"__u8":"<base64>"}` on the wire and decoded back to a `Uint8Array`. This
applies to a `call`'s `params`, a resolved `value`, and an event `payload`.
Bytes **nested** inside an object or array are **not** tagged and are silently
lost. Pass binary as the top-level value, not as a field.

### Example

Using the [`@orzma/web`](../sdk/orzma-web) client:

```ts
import { orzma, isOrzmaAvailable } from "@orzma/web";

if (isOrzmaAvailable()) {
  // Request / response — annotate the reply type.
  const res = await orzma.call<{ saved: boolean }>("save", { text: "hi" });

  // Subscribe to a program event — annotate the payload.
  orzma.on<{ n: number }>("tick", (payload) => console.log(payload.n));

  // One-way event to the program.
  orzma.emit("ready", { ok: true });
}
```

## Lifecycle & teardown

- `unregister{handle}` releases a handle and removes its mounted views.
- Closing the control connection purges all of that program's handles, removes
  their views, and rejects every in-flight `call` with `owner_disconnected`.
- The `compositing` push reports a view's first paint (`active:true`) and its
  teardown after compositing (`active:false`).

## Security model

- **Same user only.** The host rejects any control connection whose peer user id
  differs from orzma's.
- **Scoped to one pane.** A connection's token binds it to the pane that issued
  `$ORZMA_TOKEN`; a program may only mount, focus, navigate, and emit to handles
  it registered.
- **Unguessable, isolated handles.** Handles are 128-bit random values, each its
  own `orzma://` origin.
- **Authorized replies.** Back-channel `reqId`s are a shared, monotonic counter
  and therefore guessable, so the host authorizes a `reply` by its originating
  connection: a program replaying another connection's `reqId` can neither
  settle nor drop that call.

## SDKs

Prefer a ready-made client over implementing the wire protocol directly:

- [`ratatui-orzma`](../sdk/ratatui-orzma) — Rust SDK for the program side (a
  ratatui widget plus a back-channel RPC handler).
- [`@orzma/web`](../sdk/orzma-web) — TypeScript client for the page-side
  `window.orzma` bridge.
