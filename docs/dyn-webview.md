# Tier 1 Dynamic Local Webviews

Any local program running inside an ozmux pane can display an inline HTML
webview — either a static HTML string or a directory of local files — without
authoring an extension. This is the Tier 1 ("TUI dynamic") path. Phase A ships
display-only support; host-API escalation is deferred (see [Phase A scope](#phase-a-scope--not-yet)).

## Trust model

Content and capabilities never arrive from PTY bytes. The control socket is the
authenticated-local channel: only a process with access to the local Unix
socket and a valid per-surface `$OZMA_TOKEN` can register a view. The kernel's
peer-UID check on the socket connection is the outer boundary; `$OZMA_TOKEN`
provides attribution (which pane surface owns the registration). Remote input
piped into a shell prompt cannot reach the socket.

Registrations are scoped to the registering surface and the socket connection.
They are torn down automatically on socket disconnect or surface despawn. A
handle minted for one surface cannot be mounted from a different surface's
terminal output.

For the full threat model see
`docs/superpowers/specs/2026-06-13-tui-dynamic-webview-phase-a-design.md`.

## Environment variables

How these reach a pane depends on the multiplexer backend:

| Variable | Contents | Availability |
|---|---|---|
| `$OZMA_SOCK` | Absolute path to the control Unix socket | Present in every pane the direct-PTY backend spawns. Under the tmux backend, a pane that forked before ozma set it (a pre-existing session, or any pane opened before attach) does **not** inherit it — recover it with `tmux show-environment OZMA_SOCK` (see below). |
| `$OZMA_TOKEN` | Opaque per-surface token (attribution only) | Set only by the direct-PTY backend. Under the tmux backend it is unset; use the tmux pane id `$TMUX_PANE` (injected into every pane) as the identity instead. |

Both are absent when the control plane is not up (e.g. during `cargo test`
builds or when the feature flag is off).

The `ratatui-ozma` SDK does this resolution in `Ozma::connect()`: it reads
`$OZMA_SOCK`, falling back to `tmux -S <socket> show-environment OZMA_SOCK`
(deriving `<socket>` from `$TMUX`) so a pre-existing tmux pane still resolves the
path without a shell-rc hook; and it uses `$OZMA_TOKEN` when set, else
`$TMUX_PANE`. A program that speaks to the socket directly (without the SDK)
should do the same.

## Control protocol (NDJSON)

The socket speaks NDJSON: one JSON object per line in each direction. Send
requests, read one reply per `register`. Unknown `op` values are rejected.

### Handshake

Send once, before any `register`:

```json
{"op":"hello","token":"<$OZMA_TOKEN, else $TMUX_PANE>"}
```

No reply is sent for `hello`.

### Register

```json
{"op":"register","kind":"inline","html":"<full HTML document>","interactive":false}
```

```json
{"op":"register","kind":"dir","root":"/absolute/path","entry":"index.html","interactive":true}
```

```json
{"op":"register","kind":"url","url":"https://example.com","interactive":true,"bridge":false}
```

Fields:

| Field | Type | Default | Meaning |
|---|---|---|---|
| `kind` | `"inline"` \| `"dir"` \| `"url"` | required | Content source |
| `html` | string | — | Full HTML document (`inline` only). Max 4 MiB. |
| `root` | string | — | Absolute directory path (`dir` only). Must exist. |
| `entry` | string | — | HTML entry relative to `root` (`dir` only). No `..` or leading `/`. |
| `interactive` | bool | `true` | Whether the mounted webview accepts pointer/keyboard input. |
| `url` | string | — | Remote `http(s)` URL (`url` only). `http`/`https` schemes only. |
| `bridge` | bool | `false` | Inject the `window.ozma` back-channel (`url` only; `inline`/`dir` are always bridged). |

A `url` webview is display-only by default: without `bridge:true`, the page
receives no `window.ozma` bridge, and ozmux delivers it no `emit` events. The
URL itself still travels the authenticated socket (never PTY bytes), preserving
the Tier 1 trust model. `http`/`https` only — other schemes are rejected with
`unsupported_scheme`.

Reply on success:

```json
{"ok":true,"handle":"<opaque handle>"}
```

Reply on failure:

```json
{"ok":false,"error":"<error code>"}
```

Error codes:

| Code | Cause |
|---|---|
| `invalid_root` | `root` is not absolute, or does not name an existing directory |
| `unsafe_entry` | `entry` contains `..`, `.`, a leading `/`, or is empty |
| `html_too_large` | `html` exceeds the 4 MiB limit |
| `unsupported_scheme` | `url` scheme is not `http`/`https` |
| `invalid_url` | `url` is unparseable or has no host |
| `internal` | Server-side fallback if the ECS apply system drops the reply channel before responding — should not occur in normal operation (not a `build_view` validation error). |

The reply arrives synchronously (the listener blocks until the Bevy system
drains the event on the next frame, then sends the reply).

### Unregister

```json
{"op":"unregister","handle":"<handle from a prior register>"}
```

No reply. Only the connection that registered a handle may unregister it; a
mismatched handle is silently ignored. Closing the socket unregisters all
handles for that connection automatically.

## OSC mount

After receiving the handle, print this sequence to stdout at the cursor
position:

```
ESC ] 5379 ; mount ; <handle> ; <rows> ; <cols> ESC \
```

In Rust string syntax: `"\x1b]5379;mount;{handle};{rows};{cols}\x1b\\"`.

Then print `<rows>` newlines to reserve vertical space so subsequent output
lands below the webview.

- `<handle>` — the opaque string returned by the socket.
- `<rows>` / `<cols>` — view size in terminal cells.
- A handle may only be mounted from the surface that registered it (the one
  whose `$OZMA_TOKEN` was used in `hello`). Mounting from a different surface
  is silently dropped.

For the full OSC 5379 protocol (including `unmount`, geometry limits,
focus, and scrollback caveats) see [`docs/webview.md`](webview.md).

## Reference client

`examples/dyn_webview_client.rs` is a self-contained Rust program that
demonstrates the full flow: connect, `hello`, `register` inline HTML, print the
mount OSC, then sleep (keeping the registration alive) until killed.

Run it inside an ozmux pane:

```sh
cargo run --example dyn_webview_client
```

## Manual E2E recipe

The user runs this to verify the feature end-to-end:

```sh
# Terminal 1: launch ozmux (the debug feature enables the CEF DevTools endpoint)
cargo run --features debug

# Inside an ozmux pane:
cargo run --example dyn_webview_client
```

Expected behavior: a webview reading "hello from a TUI app" renders at
the cursor, scrolls with the scrollback buffer, and disappears when the client
is killed (`Ctrl-C` → socket disconnect → automatic registration teardown).

## Phase A scope / not-yet

The following are explicitly out of scope for Phase A and are deferred:

- **Host-API escalation** — dynamic webviews that call `window.<ns>.<method>`
  APIs; Phase A webviews are display-only (no `window.ozma` bridge).
- **Untrusted raw-OSC tier** — a lower-trust path that bypasses the socket
  entirely, using PTY escape codes alone; deferred pending a separate threat
  model review.

See §11 of
`docs/superpowers/specs/2026-06-13-tui-dynamic-webview-phase-a-design.md` for
the full deferred items list.
