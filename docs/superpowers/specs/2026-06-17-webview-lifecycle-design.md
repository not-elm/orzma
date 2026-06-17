# Webview Compositing Lifecycle Events — Design Spec

## Problem

`ozmd` renders a `WebviewWidget` with `.fallback(Block::bordered().title("loading…"))` every frame.
Per `docs/inline-webview.md`, the terminal text layer always composites **above** the webview renderer.
Because `WebviewWidget::render()` always paints the fallback cells, the "loading…" border and title
remain permanently visible on top of the markdown content.

The widget's own doc comment acknowledges this: the fallback is "shown on non-macOS or **before** the
page composites." The missing piece is a signal that tells the application **when** compositing has begun.

## Solution Overview

Add a one-way lifecycle event from ozmux to the registered program when the inline webview first
successfully composites (and when it stops). The SDK surfaces this as a per-frame callback on
`WebviewWidget`. The app stores the compositing state in `App` using `Cell<bool>` and conditionally
suppresses the fallback.

## Data Flow

```
ozmux (Bevy)
  project_inline_overlays
    → {"op":"compositing","handle":"<id>","active":bool}
    → ConnectionWriters::send(connection_id, line)

SDK reader thread
    → Ozma shared state: pending_compositing: Arc<Mutex<Vec<(String, bool)>>>

ozma.frame()
    → drains pending_compositing into FramePlacements::pending_compositing: HashMap<String, bool>

WebviewWidget::render()
    → if pending event for this handle → call on_compositing_change callback
    → |active| app.set_compositing(active)   (Cell<bool>, takes &self)

Next frame's draw_body
    → app.compositing() → conditionally show/hide fallback
```

## Layer 1: ozmux Bevy (`src/inline_webview.rs`)

### New component

```rust
/// Marks an inline webview entity after the first frame it successfully
/// projected into `TerminalOverlays`.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CompositeNotified;
```

### `project_inline_overlays` changes

Add `mut writers: ResMut<ConnectionWriters>` and `(Has<CompositeNotified>, Option<&WebviewOwner>)` to
the inline entity query.

In the branch that writes to `overlays.rects[slot]`, after the write:

```rust
if !already_notified {
    if let Some(owner) = owner {
        // Serialize via ServerMsg::Compositing { handle, active: true } + serde_json::to_string
        // (requires adding the variant to control_plane/protocol.rs ServerMsg enum)
        writers.send(owner.connection_id, compositing_msg(&owner.handle, true));
    }
    commands.entity(child).insert(CompositeNotified);
}
```

### STOP event

Add a Bevy observer on `OnRemove<InlinePlacement>`. When the removed entity has `CompositeNotified`
and `WebviewOwner`:

```rust
fn on_placement_removed(
    trigger: On<OnRemove<InlinePlacement>>,
    mut writers: ResMut<ConnectionWriters>,
    mut commands: Commands,
    query: Query<(&WebviewOwner, Has<CompositeNotified>)>,
) {
    let entity = trigger.target();
    if let Ok((owner, true)) = query.get(entity) {
        // Serialize via ServerMsg::Compositing { handle, active: false } + serde_json::to_string
        // (requires adding the variant to control_plane/protocol.rs ServerMsg enum)
        writers.send(owner.connection_id, compositing_msg(&owner.handle, false));
    }
}
```

Register with `app.add_observer(on_placement_removed)`.

## Layer 2: SDK (`sdk/ratatui-ozma/`)

### `session.rs`

Add to `Ozma` (and its shared inner state):

```rust
pending_compositing: Arc<Mutex<HashMap<String, bool>>>,
```

In the reader thread, parse `{"op":"compositing",...}`:

```rust
} else if v["op"] == "compositing" {
    if let (Some(handle), Some(active)) =
        (v["handle"].as_str(), v["active"].as_bool())
    {
        if let Ok(mut q) = pending_compositing.lock() {
            q.insert(handle.to_owned(), active);
        }
    }
}
```

In `Ozma::frame()`, drain into a fresh `FramePlacements::pending_compositing: HashMap<String, bool>`.
A HashMap naturally deduplicates: if two events arrive for the same handle in one frame, the later
value wins. `FramePlacements::clear()` (called by `frame()` each draw) must also reset
`pending_compositing` — otherwise events from frame N would re-fire the callback on frame N+1.

### `widget.rs`

Add field and builder method to `WebviewWidget`:

```rust
pub struct WebviewWidget<'a, W = WebviewDefaultPlaceholder> {
    handle: &'a str,
    fallback: W,
    focused: bool,
    on_compositing_change: Option<Box<dyn Fn(bool) + 'a>>,
}

impl<'a, W> WebviewWidget<'a, W> {
    /// Registers a callback invoked when this webview's compositing state changes.
    ///
    /// Called synchronously during [`StatefulWidget::render`] when a pending
    /// compositing event is present for this handle.
    pub fn on_compositing_change(mut self, f: impl Fn(bool) + 'a) -> Self {
        self.on_compositing_change = Some(Box::new(f));
        self
    }
}
```

In `render()`, after `state.record()`:

```rust
if let Some(active) = state.take_compositing(self.handle) {
    if let Some(cb) = &self.on_compositing_change {
        cb(active);
    }
}
```

`FramePlacements::take_compositing(handle: &str) -> Option<bool>` removes and returns the entry.

## Layer 3: ozmd (`apps/ozmd/`)

### `app.rs`

Add `compositing: Cell<bool>` to `App`:

```rust
use std::cell::Cell;

pub(crate) struct App {
    // existing fields ...
    compositing: Cell<bool>,
}

impl App {
    /// Whether the inline webview is currently compositing.
    pub(crate) fn compositing(&self) -> bool {
        self.compositing.get()
    }

    /// Updates the compositing state. Takes `&self` (via `Cell`) so it can be
    /// called from a `WebviewWidget::on_compositing_change` callback while
    /// `&App` is borrowed in the draw closure.
    pub(crate) fn set_compositing(&self, active: bool) {
        self.compositing.set(active);
    }
}
```

`Cell<bool>` provides single-threaded interior mutability; `Default` derives as `Cell::new(false)`.

### `ui.rs` (`draw_body`)

The two branches have different generic types (`WebviewWidget<WebviewDefaultPlaceholder>` vs.
`WebviewWidget<Block>`), so each calls `render_stateful_widget` independently:

```rust
if app.compositing() {
    frame.render_stateful_widget(
        WebviewWidget::new(handle_id)
            .on_compositing_change(|active| app.set_compositing(active)),
        webview_area,
        placements,
    );
} else {
    frame.render_stateful_widget(
        WebviewWidget::new(handle_id)
            .fallback(Block::bordered().title("loading…"))
            .on_compositing_change(|active| app.set_compositing(active)),
        webview_area,
        placements,
    );
}
```

`ui::draw`'s signature is unchanged — `compositing` is read from `app`, no new parameter needed.

## Timing

The compositing callback fires synchronously inside `WebviewWidget::render()` during the draw closure.
The `Cell` update is immediate, but the decision to show the fallback is made at widget construction
time (before `render()`), so the visual change takes effect on the **next frame** — imperceptible in
practice.

## Files Changed

| File | Change |
|------|--------|
| `src/inline_webview.rs` | `CompositeNotified` component; extend `project_inline_overlays`; add stop observer |
| `sdk/ratatui-ozma/src/session.rs` | `pending_compositing` buffer; reader-thread parse; `frame()` drain |
| `sdk/ratatui-ozma/src/widget.rs` | `on_compositing_change` builder and field; `render()` dispatch |
| `apps/ozmd/src/app.rs` | `compositing: Cell<bool>` field; `compositing()`/`set_compositing()` |
| `apps/ozmd/src/ui.rs` | Conditional fallback in `draw_body` |

## Verification

1. `cargo build` — no compile errors.
2. `ozmd <file.md>` in an ozmux terminal: "loading…" appears briefly, then disappears once the webview
   composites.
3. File reload (`r`): "loading…" does NOT reappear — the webview stays mounted, `compositing` stays
   `true`.
4. Process exit and re-launch: "loading…" reappears on the fresh launch (compositing resets to
   `false`).
5. `cargo test` — no regressions.
