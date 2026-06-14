# `ratatui-ozma` — ratatui Webview Widget + RPC handler

**Status:** Design (approved in brainstorming, pending spec review)
**Date:** 2026-06-14
**Crate:** `sdk/ratatui-ozma` (currently a stub: `Cargo.toml` + empty `src/lib.rs`)

## 1. Purpose & scope

`ratatui-ozma` is a **client-side Rust library** that a [ratatui](https://ratatui.rs)
TUI app — running *inside* an ozmux pane — depends on to embed a live ozmux
webview as a ratatui widget, and to talk to that webview over RPC.

It is the ergonomic Rust twin of the existing `@ozmux/sdk` (`sdk/typescript`)
and of `examples/dyn_webview_client.rs`, which already demonstrate the raw
register → mount → RPC → emit round-trip. This crate wraps that raw protocol so
apps never hand-roll escape codes or NDJSON.

### What it wraps (all pre-existing ozmux mechanisms)

1. **Control socket** — `$OZMUX_SOCK` (Unix domain socket) authenticated with
   `$OZMUX_TOKEN`. Used to `register` content (→ mint an opaque handle), and as
   the bidirectional RPC channel (`call` / `reply` / `emit`).
   - Code: `src/control_plane.rs`, `src/control_plane/protocol.rs`,
     `src/control_plane/listener.rs`.
2. **OSC 5379 `mount-inline` / `unmount-inline`** — positions the webview at a
   viewport cell rect using the **alt-screen fixed-anchor** model added in
   commit `57cc03f` (#115). This is exactly what a redraw-every-frame TUI needs.
   - Code: `src/osc_webview.rs`, `src/inline_webview.rs`,
     `crates/ozma_tty_engine/src/handle.rs` (anchor stamping),
     `crates/ozma_tty_engine/src/vt/listener.rs` (`AnchorMode`).
   - Spec: `docs/inline-webview.md` (note: its "alt-screen mounts are rejected"
     line is **obsolete** after #115).
3. **`window.ozmux`** JS bridge — `call` / `on` / `off`, injected into every
   Tier 1 page. The SDK is the program-side peer of this bridge.
   - Code: `src/extension_render/ozmux_bridge.js`, `src/extension_render.rs`.

### Constraints (inherited from the repo)

- **Tokio-free.** std threads + one background I/O thread + `crossbeam`/std
  channels, matching `examples/dyn_webview_client.rs` and the rest of ozmux.
- **ratatui 0.29** (latest) as the target.
- **macOS-only compositing.** The inline-webview GPU path is macOS-only; on
  other platforms the OSC is silently dropped, so the widget must degrade
  gracefully (see §4 fallback).
- Comment / doc / visibility rules per `.claude/rules/rust.md`.

### Out of scope for v1 (YAGNI)

- `instance_id` multi-mount. One `Webview` = one handle = one mount.
- The dormant `window.<ns>.<method>` Node host-API bridge (separate subsystem).
- Normal-screen (scrollback text-flow) mounting — ratatui runs in the alternate
  screen, so v1 targets the `FixedScreen` anchor only.
- Async runtimes (tokio/async-std).

## 2. The alt-screen fixed-anchor model (background)

Why this design is possible at all (from `#115`):

- On the alternate screen, `mount-inline` stamps an
  `AnchorMode::FixedScreen { row, col }` from the **cursor position at the OSC
  stop-point** (`crates/ozma_tty_engine/src/handle.rs`). `row`/`col` are
  **viewport-relative** cells (0 = top-left of the visible grid).
- The webview stays at that cell rect across full-buffer redraws (which ratatui
  does every frame). Re-emitting `mount-inline` with the same handle is an
  **idempotent in-place re-anchor** — ozmux's `set_if_neq` fast path means the
  CEF page is **not reloaded** and the surface is only resized when `rows`/`cols`
  actually change (`src/inline_webview.rs`). Safe at 60 fps.
- Moving the widget = re-emit with a new `(row, col)`; there is no separate
  "move" op. Terminal resize → ratatui recomputes layout → next re-emit
  re-anchors in one frame.
- **Auto-unmount:** leaving the alt screen despawns all `FixedScreen` webviews
  (`despawn_fixed_screen_on_alt_exit` in `src/inline_webview.rs`); the underlying
  registration survives and can be re-mounted on re-entering the alt screen.

## 3. Object model

```
Ozma                 // session: owns the socket + background I/O thread
 ├─ connect() -> Result<Ozma>             // read env, hello, spawn reader thread
 ├─ register(Webview) -> Result<WebviewHandle>
 ├─ frame() -> &mut FramePlacements        // StatefulWidget state for this frame
 └─ flush(&mut Terminal<B>) -> Result<()>  // emit cursor moves + OSC after draw

Webview              // builder: content + handlers, pre-registration
 ├─ inline(html: impl Into<String>) -> Webview
 ├─ dir(root: impl AsRef<Path>, entry: impl Into<String>) -> Webview
 ├─ interactive(bool) -> Webview           // control-plane focus/input flag (default true); fixed at register, not changeable per-frame
 ├─ fallback(impl Widget) -> Webview       // under-layer painted into cells
 └─ on(method, handler) -> Webview         // RPC handler, serde-typed

WebviewHandle        // cheap Clone (Arc to write-half + handle id), returned by register
 ├─ emit(event, &payload) -> Result<()>    // push to window.ozmux.on(event, …)
 └─ id() -> &str

WebviewWidget<'a>    // the StatefulWidget rendered each frame
 └─ new(&WebviewHandle) -> WebviewWidget   // optionally .fallback(...) per-frame
```

- `Ozma` owns the single `UnixStream`. Dropping it closes the socket; the control
  plane tears down every handle registered on that connection.
- `WebviewHandle` is `Clone` (shares an `Arc<Mutex<impl Write>>` to the socket
  write-half plus the handle id), so one copy lives in app state (for `emit`) and
  another is handed to the widget each frame.

## 4. Render flow — StatefulWidget + post-draw flush

A ratatui `Widget::render(area, buf)` only writes an in-memory cell `Buffer`; it
cannot write escape sequences to stdout. Mounting requires positioning the real
cursor and writing the OSC **after** `terminal.draw()` flushes. So the widget and
the OSC emission are split across the draw boundary:

```rust
loop {
    terminal.draw(|f| {
        f.render_stateful_widget(
            WebviewWidget::new(&chart).fallback(Paragraph::new("loading…")),
            layout[0],
            ozma.frame(),               // FramePlacements collector
        );
    })?;
    ozma.flush(&mut terminal)?;          // real cursor moves + OSC, after ratatui's flush
    // handle input / RPC-driven state …
}
```

- `FramePlacements` is a per-frame collector: `ozma.frame()` returns it cleared,
  and `flush()` consumes it — a skipped `draw()` cannot leave stale placements.
- **`render(area, buf, state)`** (in-memory, pure): paint the fallback widget into
  `buf` (or blank the cells if no fallback), and record
  `(handle_id, area, interactive)` into the `FramePlacements` collector.
- **`flush(&mut terminal)`**: emission is **diff-driven** against the previous
  frame. The SDK keeps a `HashMap<Handle, (row, col, rows, cols)>` of the
  last-emitted placement and, for each recorded placement, writes
  `ESC[{y+1};{x+1}H` (CUP — 1-based, viewport-relative, matching `FixedScreen`)
  then `ESC]5379;mount-inline;{handle};{height};{width}ESC\` **only when the
  tuple is new or changed** (e.g. a resize/move). It emits `unmount-inline;{handle}`
  only for handles that vanished. **No reserved newlines** (alt-screen; the buffer
  cells already hold the space). Re-emitting an unchanged placement would be
  idempotent and no-reload (`set_if_neq`), but diffing avoids the per-frame PTY
  write and the forced grid emit (`force_next_emit`) the parser path triggers
  (`crates/ozma_tty_engine/src/handle.rs`), so the SDK only emits on change.
- `flush` is generic over `B: Backend + Write` (crossterm and termion backends
  both implement `Write`) and writes through `terminal.backend_mut()` so the OSC
  lands immediately after ratatui's own flush, with the cursor under our control.
- **Area validation:** before emitting, `flush` skips any placement whose rect is
  degenerate (0 width/height) or out of the OSC's accepted range (rows `1..=200`,
  cols `1..=400`; handle charset `[A-Za-z0-9._-]{1,128}`). An out-of-range or
  malformed sequence is *silently dropped* by the VT layer with no error path
  (`crates/ozma_tty_engine/src/osc_webview.rs`), so the SDK must clamp/skip and
  surface the skip rather than emit a sequence that vanishes.
- **Cursor desync:** writing raw bytes through `backend_mut()` leaves ratatui's
  tracked cursor position stale (`Terminal::backend_mut` docs warn of this). This
  is safe *only* because the next `draw()` repositions the cursor — i.e. the SDK
  assumes a redraw-every-frame loop. We rely on `CrosstermBackend: Write` (stable)
  and deliberately avoid the unstable `writer()`/`writer_mut()` accessors.

**Fallback:** `Webview`/`WebviewWidget` take an optional inner `impl Widget`. When
set, it is rendered into the cells as the under-layer (visible on non-macOS or
before the page first composites — text always draws over the webview anyway);
when unset, the cells are blanked.

## 5. RPC handler model

Handlers are method-keyed closures; arguments and return values are
serde-(de)serialized JSON, mirroring the `window.ozmux` bridge.

```rust
let mut wv = Webview::inline(html);
wv = wv
    .on("ping", |arg: String| Ok::<_, RpcError>(format!("pong:{arg}")))
    .on("save", |doc: Doc| { /* … */ Ok(()) });
let chart = ozma.register(wv)?;
chart.emit("tick", &n)?;          // -> window.ozmux.on('tick', n)
```

- **Wire protocol** (the control plane already speaks these; note the inbound
  `call` frame is synthesized in `src/extension_render.rs`, not a `ServerMsg`
  variant in `protocol.rs`, and register replies are untagged `{ok, handle}` /
  `{ok:false, error}` — the SDK serializes `register()` through its single I/O
  thread so each reply matches its request):
  - inbound to the program: `{"op":"call","handle":H,"reqId":R,"method":M,"args":[…]}`
  - program reply: `{"op":"reply","reqId":R,"ok":true,"value":V}` /
    `{"op":"reply","reqId":R,"ok":false,"error":E}`
  - program → page push: `{"op":"emit","handle":H,"event":E,"payload":P}`.
    **`emit` is mount-scoped:** it fans out only to a *currently-mounted*
    page (`src/control_plane.rs` `Emit` arm), and is a silent no-op when the
    handle is not mounted (e.g. off the alt screen). `WebviewHandle::emit`
    returns `Ok(())` on a successful socket write regardless of delivery.
- **Threading:** handlers run on the background I/O thread, so they are
  `Send + 'static`. To touch app state, a handler shares it via
  `Arc<Mutex<…>>` or sends a message into a channel the main render loop drains.
  This tradeoff is documented prominently.
  **Lock-scope invariant (critical):** the socket write-half mutex is *never*
  held across a user handler invocation. A handler may call
  `WebviewHandle::emit` (which locks the write half) from inside dispatch; holding
  the write lock across the handler would deadlock. The reader thread locks only
  around each `writeln!`, matching `examples/dyn_webview_client.rs`. A
  channel-drained command pattern is offered as the deadlock-free default, with
  `Arc<Mutex<…>>` as the terser escape hatch.
- **Args mapping:** `window.ozmux.call(method, args)` always sends `args` as a
  JSON **array**. The handler parameter is deserialized from that array:
  single-arg closures via an extractor-style trait impl (`|arg: String|` ⇐
  `["hi"]`), multi-arg via a tuple (`|(a, b): (u32, String)|` ⇐ `[1, "x"]`). The
  precise extractor ergonomics (how many arities, the trait shape) are settled in
  the implementation plan; the fallback if extractors prove fiddly is a single
  `Params` argument with `.get::<T>(i)` accessors.
- **Errors:** a handler returning `Err(_)`, an unknown method, or a
  deserialization failure produces `{ok:false, error}`, surfacing as a rejected
  Promise in the page.

## 6. Lifecycle & error handling

- **Unmount on disappear:** when a widget is not rendered in a frame, the next
  `flush` emits `unmount-inline` for its handle (placement-set diff).
- **App exit / alt-screen leave:** ozmux auto-unmounts `FixedScreen` views;
  dropping `Ozma` unregisters everything on the connection.
- **`OzmaError`** (via `thiserror`):
  - `NotInPane` — `$OZMUX_SOCK` / `$OZMUX_TOKEN` unset.
  - `Io` — socket connect / read / write failures.
  - `Register` — control-plane rejection (`invalid_root`, `unsafe_entry`,
    `html_too_large`, `internal`).
  - `Serde` — (de)serialization failures.
- **`RpcError`** — per-handler failure type returned to the page.

## 7. Testing strategy

- **Fake control server:** a `UnixListener` test fixture speaking the NDJSON
  protocol, driving full register → call → reply → emit round-trips against the
  SDK with no real ozmux process. This is the primary integration test.
- **Sequence builders:** unit-test the exact CUP + `mount-inline` /
  `unmount-inline` bytes, and the mount/unmount placement-diff set.
- **Widget render:** render `WebviewWidget` into ratatui's `TestBackend` buffer;
  assert the fallback is painted (or cells blanked) and the placement is recorded
  in `FramePlacements`.
- **Example:** `examples/ratatui_webview.rs` — the ergonomic, end-to-end twin of
  `examples/dyn_webview_client.rs` (layout → widget → RPC handler → emit loop).

## 8. Resolved decisions & plan-time options

Resolved from spec review:
1. **Channels:** use `crossbeam-channel` for the internal plumbing, matching the
   rest of ozmux and `examples/dyn_webview_client.rs`.
2. **`flush` shape:** implement the byte emission as `flush_to(&mut impl Write)`
   and provide `flush(&mut Terminal<B>)` as the ergonomic wrapper over it (eases
   sequence tests and custom backend/output splits).
3. **RPC dispatch:** look up the method *before* deserializing args (return
   `unknown_method` first).

Plan-time options (ergonomics; do not affect correctness):
4. RPC-handler extractor arities and trait shape (axum-style, proc-macro-free)
   vs. shipping the `Params` wrapper first.
5. Whether to merge `WebviewHandle` + `WebviewWidget` via
   `impl StatefulWidget for &Webview`, and/or offer a single
   `ozma.draw(&mut term, |f| …)` wrapper so `flush` cannot be forgotten — weighed
   against losing per-frame `.fallback(...)` and the raw `Frame` boundary.
