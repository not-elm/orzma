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
2. **APC verbs** — terminal escape sequences that mount and unmount registered
   content at a cell rectangle.
3. **The `window.orzma` bridge** — an in-page JavaScript API the webview uses to
   call, subscribe to, and emit events to the registering program.

Three actors participate: the **registering program** (running in a pane), the
**orzma host**, and the **webview page**. A registration is a *Tier 1* (dynamic,
runtime-registered) webview — the only kind this protocol describes.

End to end: a program connects to the control socket, registers content and
receives an opaque **handle** together with its first placement **instance**,
writes an `ESC _ Omount;n=<instance>,…` sequence to display it, and then talks to
the page through the `window.orzma` bridge routed over the same control socket.
Unmounting (or disconnecting) tears it down.

## Architecture at a glance

```text
 registering program              orzma host                  webview page
 (inside an orzma pane)
        │  reads $ORZMA_SOCK / $ORZMA_TOKEN from its env
        │  hello{token} ───────────────►│
        │  register{kind,…} ───────────►│
        │◄────── {ok,handle,instance} ──│
        │  APC Omount;n=i,r=n,c=n ─────►│  mount orzma://handle/ ───►│ load page
        │                               │◄──── window.orzma.call ────│
        │◄──── {op:call,reqId,method} ──│                           │
        │  {op:reply,reqId,value} ─────►│──── resolve Promise ─────►│
        │  {op:emit,event,payload} ────►│──── window.orzma.on ──────►│
        │◄──── {op:event,…} ◄ window.orzma.emit ─────────────────────│
        │  APC Ounmount;n=instance ────►│  remove webview ──────────►│
```

The control socket carries every horizontal arrow between the program and the
host; the APC verbs carry the mount/unmount; the page bridge carries the
`window.orzma` arrows on the right.

## The control socket

### Transport

The control socket is a local Unix-domain **stream** socket speaking **NDJSON**
(on Windows, an AF_UNIX socket, available since Windows 10 1809; the endpoint
is a filesystem path on every platform):
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

The host restricts the control socket to orzma's own user. On Unix it checks
that the connecting peer's user id equals its own and silently drops the
connection otherwise. On Windows the socket lives in a directory whose DACL
grants access to the current user only, so other users cannot connect at all.
Either way, only processes running as the same user can reach the handshake.

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

- A **request reply** is the only host line with **no `op` field**. Both
  `register` and `new_instance` are answered this way, and either can also
  reply `{"ok":false,"error":"…"}`.
- Every **host-initiated push** (`call`, `event`, `compositing`) carries an
  `op` field.

So: a line with an `op` is a push; a line without one is the reply to your
oldest outstanding request. This is the one framing rule a from-scratch client
must get right.

### Request ordering

`register` and `new_instance` are processed one at a time per connection, and
each reply arrives in request order. Neither carries a request id, so
correlation is positional: a client matches replies to its pending requests by
their order. (The back-channel `call`/`reply` pair below uses an explicit
`reqId` instead.)

### Program → host messages

Every program line carries an `op`:

| `op` | Fields | Meaning |
| --- | --- | --- |
| `hello` | `token` | Handshake; first line only. |
| `register` | `kind` + per-kind fields | Register content; mints a handle and its first instance. |
| `new_instance` | `handle` | Mint an additional instance on a handle this connection owns. |
| `unregister` | `handle` | Release a handle owned by this connection; removes its mounted views. |
| `reply` | `reqId`, `ok`, `value?`, `error?` | Answer a host `call` (use the `call`'s `reqId`). |
| `emit` | `handle`, `event`, `payload` | Push an event to every page mounted from the handle (delivered to `window.orzma.on`). |
| `focus` | `instance` (string or `null`) | Set app-owned focus to a mounted placement, or `null` to blur. |
| `navigate` | `instance`, `action` | Navigate one mounted placement in place. |
| `mount` | `instance`, `row`, `col`, `rows`, `cols` | Mount one placement at a 0-based cell of the pane's active screen, the socket form of the APC `mount` (see below). |
| `unmount` | `instance` | Remove one placement mounted with the socket `mount`. |

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
| `call` | `handle`, `instance`, `reqId`, `method`, `params` | A page `window.orzma.call(method, params)`. Respond with a `reply` carrying the same `reqId`. |
| `event` | `handle`, `event`, `payload` | A page `window.orzma.emit(event, payload)`. Fire-and-forget; no response. |
| `compositing` | `handle`, `instance`, `active` (bool) | The placement first composited (`true`) or was unmounted after compositing (`false`). |

`call` names the instance whose page called; `event` does not, because a
program reads events per handle rather than per placement. A program running
two placements of one handle can therefore tell which page called it, but not
which page emitted an event.

Two directional details that are easy to get wrong:

- **`emit` vs. `event`.** A page's `window.orzma.emit(name, …)` arrives at the
  program as `op:"event"`. A program's own `emit` message (`op:"emit"`) is
  delivered to pages' `window.orzma.on(name, …)`. Same idea ("named event"), two
  `op` values depending on direction.
- **`urlChanged`.** For a `url` view, the host reports top-level address changes
  as an `op:"call"` with `method:"urlChanged"` and `params:{"url":"<new>"}`.
  Despite the `call` shape it is fire-and-forget — any `reply` is discarded. Use
  it to track page-driven navigation.

### Request replies & error codes

A successful `register` replies
`{"ok":true,"handle":"<handle>","instance":"<instance>"}` — the handle owns the
registration, and the instance is its first placement. A successful
`new_instance` replies `{"ok":true,"instance":"<instance>"}`. A rejected request
of either kind replies `{"ok":false,"error":"<code>"}`:

| `error` | Cause |
| --- | --- |
| `invalid_root` | `dir.root` is not an absolute path to an existing directory. |
| `unsafe_entry` | `dir.entry` is empty, absolute, or contains `..`/`.`. |
| `html_too_large` | `inline.html` exceeds 4 MiB. |
| `invalid_url` | `url.url` does not parse or has no host. |
| `unsupported_scheme` | `url.url` is not `http`/`https`. |
| `unknown_handle` | `new_instance.handle` names no live registration. |
| `not_owner` | `new_instance.handle` is registered, but by another connection. |
| `owner_gone` | The `register`/`new_instance` request's owner surface has already despawned. |
| `internal` | The host failed to process the request. |

### Handle semantics

A handle is opaque, unique per registration, and lowercase: 128 CSPRNG bits
base32-encoded over the alphabet `a-z2-7`, which keeps it spellable as a URL
host. That encoding is unpadded, so a handle is always 26 characters. Treat
it as a token: do not parse it. Each handle owns one isolated
`orzma://<handle>/` origin, and it is what `unregister`, `emit`, and
`new_instance` address. A handle is never mounted — a mount addresses an
instance.

### Instance semantics

An instance is one placement of a registration, and it is the unit `mount`,
`unmount`, `focus`, and `navigate` address. Its wire spelling is exactly 32
lowercase hex digits (128 CSPRNG bits); any other spelling is malformed. Only
the host mints instances — `register` mints the first, `new_instance` each
additional one — so uniqueness is structural and nothing on the wire negotiates
it.

An instance stays valid for as long as its handle is registered, whether or not
it is currently mounted. Mounting an instance that is already live updates its
rectangle in place and leaves the page untouched; mounting one after it was
unmounted builds the page again from scratch. Every instance of a handle serves
that handle's registered content, and each one mounts independently.

### Example exchange

Program-to-host lines are marked `C→S`, host-to-program lines `S→C`:

```json
C→S {"op":"hello","token":"orzma:4294967306"}
C→S {"op":"register","kind":"inline","html":"<!doctype html><body>hi</body>"}
S→C {"ok":true,"handle":"nf2k7q5w3x3m5a6b2c4d6e7fgh","instance":"3f5a9c02d1e84b7690ab3cde12f45678"}
S→C {"op":"call","handle":"nf2k7q5w3x3m5a6b2c4d6e7fgh","instance":"3f5a9c02d1e84b7690ab3cde12f45678","reqId":"0","method":"save","params":{"text":"hi"}}
C→S {"op":"reply","reqId":"0","ok":true,"value":{"saved":true}}
C→S {"op":"emit","handle":"nf2k7q5w3x3m5a6b2c4d6e7fgh","event":"tick","payload":{"n":1}}
C→S {"op":"new_instance","handle":"nf2k7q5w3x3m5a6b2c4d6e7fgh"}
S→C {"ok":true,"instance":"a1b2c3d4e5f60718293a4b5c6d7e8f90"}
```

## APC webview verbs — mount / unmount

Once an instance is minted, the program mounts it by writing an APC escape
sequence to its terminal. The sequence is framed `ESC _ <payload> ST`, where
`ST` (string terminator) is `ESC \`. Unlike an OSC, a `BEL` does not terminate
an APC — it is taken as payload data. The payload opens with `O` (orzma), so a
sequence another program owns — kitty's `G`, for example — is left alone. In
raw bytes:

```text
mount:    \x1b_Omount;n=<instance>,r=<rows>,c=<cols>\x1b\
unmount:  \x1b_Ounmount;n=<instance>\x1b\
```

The payload is at most 1024 bytes. A multi-byte character inside a key or a
value makes that field malformed, but one outside a key or a value is
silently dropped by the terminal's APC collector rather than rejected.

### Socket form

A PTY that re-renders its child's output rather than passing it through —
ConPTY on Windows — drops APC strings, so every host also accepts the same
two verbs as control-socket ops:

```json
{"op":"mount","instance":"<instance>","row":<row>,"col":<col>,"rows":<rows>,"cols":<cols>}
{"op":"unmount","instance":"<instance>"}
```

`row` and `col` are the 0-based cell of the pane's active screen that the
rect's top-left corner occupies: the cell an APC mount reaches by first moving
the cursor with `CUP row+1;col+1`. Unlike `CUP`, they are absolute even when
DECOM origin mode is on. `rows` and `cols` obey the APC bounds (`1`–`200`,
`1`–`400`). A `mount` naming an instance the connection does not own, a
size out of range, or a cell outside the grid is dropped; a mount past the
per-terminal placement cap is refused by the terminal exactly as an APC mount
is. The socket `unmount` names one instance; there is no unmount-all form.
The `ratatui_orzma` SDK sends the socket form on Windows and the APC form
elsewhere.

### mount

```text
ESC _ O mount ; n=<instance>,r=<rows>,c=<cols> ST
```

- `instance` — an instance from `register` or `new_instance`; exactly 32
  lowercase hex digits.
- `rows` — decimal `1`–`200`. `cols` — decimal `1`–`400`. Digits only, no sign.

All three keys are required and order-independent (`c=80,n=<instance>,r=24` is
the same mount as `n=<instance>,r=24,c=80`). A repeated key, an unknown key —
the retired `v=` included — a missing `n` / `r` / `c`, or an empty params
section (`Omount;`) is malformed.

The placement occupies a `rows`×`cols` rectangle of terminal cells, inline at
the cursor.

### unmount

```text
ESC _ O unmount [ ; n=<instance> ] ST
```

- No params section → unmount every inline placement this program has on the
  terminal.
- `n=` → unmount that one placement.

`n` is the only key an unmount accepts, so no key-ordering rule applies. An
empty params section (`Ounmount;`) is malformed, as are an empty value
(`Ounmount;n=`) and the retired `v=` key.

### Ownership and malformed sequences

A `mount` takes effect only in the pane whose `$ORZMA_TOKEN` registered the
instance's handle — a program mounts its own instances in its own pane. An
instance the host never minted, or one whose handle belongs to another pane, is
silently dropped, as is any malformed sequence (a bad instance spelling,
out-of-range dimensions, an unknown or repeated key); the host reports no
error.

A mount the terminal accepts but cannot place — the per-terminal placement cap
of 12 is full — is also dropped, and the host logs it at debug level. The cap
counts both screens, so it is reached before the renderer runs out of overlay
slots.

### Example

Mount instance `3f5a9c02d1e84b7690ab3cde12f45678` as a 24×80 placement, then
unmount it:

```text
\x1b_Omount;n=3f5a9c02d1e84b7690ab3cde12f45678,r=24,c=80\x1b\
\x1b_Ounmount;n=3f5a9c02d1e84b7690ab3cde12f45678\x1b\
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

- `unregister{handle}` releases a handle, invalidates every instance minted
  from it, and removes its mounted views.
- Closing the control connection purges all of that program's handles, removes
  their views, and rejects every in-flight `call` with `owner_disconnected`.
- The `compositing` push reports one placement's first paint (`active:true`) and
  its teardown after compositing (`active:false`), naming both the `handle` and
  the `instance` it belongs to.

## Security model

- **Same user only.** The host restricts the control socket to orzma's own
  user: by peer user id on Unix, by the socket directory's DACL on Windows.
- **Scoped to one pane.** A connection's token binds it to the pane that issued
  `$ORZMA_TOKEN`; a program may only mount, focus, navigate, and emit to
  registrations it made itself.
- **Unguessable, isolated identifiers.** Handles and instances are both 128-bit
  CSPRNG values, and each handle is its own `orzma://` origin.
- **Authorized replies.** Back-channel `reqId`s are a shared, monotonic counter
  and therefore guessable, so the host authorizes a `reply` by its originating
  connection: a program replaying another connection's `reqId` can neither
  settle nor drop that call.

## SDKs

Prefer a ready-made client over implementing the wire protocol directly:

- [`ratatui_orzma`](../sdk/ratatui_orzma) — Rust SDK for the program side (a
  ratatui widget plus a back-channel RPC handler).
- [`@orzma/web`](../sdk/orzma-web) — TypeScript client for the page-side
  `window.orzma` bridge.
