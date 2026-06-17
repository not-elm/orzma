# Webview Compositing Lifecycle — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the persistent "loading…" overlay in ozmd by adding a one-way compositing lifecycle event from ozmux to the registered program.

**Architecture:** ozmux Bevy tracks first-composite via a `CompositeNotified` marker component, sends `{"op":"compositing","handle":"...","active":bool}` over the control socket; the ratatui-ozma SDK buffers it per-frame and dispatches via `WebviewWidget::on_compositing_change` callback; ozmd stores the state in `App::compositing: Cell<bool>` and conditionally omits the fallback widget.

**Tech Stack:** Rust 1.95, Bevy 0.18, ratatui, serde_json 1.x (all already in workspace).

## Global Constraints

- Rust edition 2024, toolchain 1.95 (`rust-toolchain.toml`)
- No `mod.rs` files — module roots are `foo.rs` + `foo/bar.rs`
- Only `// TODO:`, `// NOTE:`, `// SAFETY:` line comments permitted (no narrative comments)
- `//!` module-level doc on every file that declares a module
- `///` doc comments required on every `pub` item
- Mutable parameters before immutable in function signatures (exception: fixed structural first params like `&self`, `On<E>` observer triggers)
- Private items last in `impl` blocks
- `#[expect(..., reason = "...")]` preferred over `#[allow(...)]`
- `run_if(resource_exists_and_changed::<T>)` instead of in-body `if !res.is_changed() { return; }`
- No `set_changed()` / `bypass_change_detection()` hacks — mutate conditionally so normal `DerefMut` drives change detection

---

### Task 1: Wire protocol — `PushMsg::Compositing`

**Files:**
- Modify: `src/control_plane/protocol.rs`

**Interfaces:**
- Produces: `pub(crate) enum PushMsg` with variant `Compositing { handle: String, active: bool }` serializing to `{"op":"compositing","handle":"<id>","active":<bool>}`

- [ ] **Step 1: Write failing test**

Add inside the existing `#[cfg(test)] mod tests` block at the bottom of `src/control_plane/protocol.rs`:

```rust
#[test]
fn serializes_compositing_start() {
    let msg = PushMsg::Compositing {
        handle: "abc123".into(),
        active: true,
    };
    assert_eq!(
        serde_json::to_string(&msg).unwrap(),
        r#"{"op":"compositing","handle":"abc123","active":true}"#
    );
}

#[test]
fn serializes_compositing_stop() {
    let msg = PushMsg::Compositing {
        handle: "abc123".into(),
        active: false,
    };
    assert_eq!(
        serde_json::to_string(&msg).unwrap(),
        r#"{"op":"compositing","handle":"abc123","active":false}"#
    );
}
```

- [ ] **Step 2: Run test to confirm it fails**

```
cargo test -p ozmux-gui --test '*' -- serializes_compositing 2>&1 | head -30
```

Expected: error — `PushMsg` not found.

- [ ] **Step 3: Implement `PushMsg`**

Add to `src/control_plane/protocol.rs` after the `ServerMsg` impl block (before `fn default_true()`):

```rust
/// An outbound push notification sent from the control plane to a registered
/// program over the control socket without being a reply to a request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub(crate) enum PushMsg {
    /// Fired when the inline webview first composites (`active: true`) or is
    /// unmounted after compositing (`active: false`).
    Compositing {
        /// The registered handle whose compositing state changed.
        handle: String,
        /// `true` when compositing starts; `false` when it stops.
        active: bool,
    },
}
```

- [ ] **Step 4: Run test to confirm it passes**

```
cargo test -p ozmux-gui -- serializes_compositing
```

Expected: both tests PASS.

- [ ] **Step 5: Commit**

```bash
git add src/control_plane/protocol.rs
git commit -m "$(cat <<'EOF'
feat(control_plane): add PushMsg::Compositing wire type

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Bevy notification — `CompositeNotified` + `project_inline_overlays` + stop observer

**Files:**
- Modify: `src/inline_webview.rs`

**Interfaces:**
- Consumes: `PushMsg` from `crate::control_plane::protocol`, `ConnectionWriters` from `crate::control_plane`, `WebviewOwner` from `crate::control_plane`
- Produces: `CompositeNotified` marker component; `project_inline_overlays` sends `{"op":"compositing","handle":"...","active":true}` on first projection per entity; `on_placement_removed` observer sends `{"op":"compositing","handle":"...","active":false}` on despawn if already notified

- [ ] **Step 1: Write failing tests**

Add to the `#[cfg(test)] mod tests` block at the bottom of `src/inline_webview.rs`.

First add two helpers in the test module (these are test-only, not test functions):

```rust
use crossbeam_channel::bounded;

fn compositing_writers(
    connection_id: u64,
) -> (ConnectionWriters, crossbeam_channel::Receiver<String>) {
    let (tx, rx) = bounded(16);
    let writers = ConnectionWriters::default();
    writers.insert(connection_id, tx);
    (writers, rx)
}

fn spawn_owned_projection_child(
    app: &mut App,
    terminal: Entity,
    slot: u8,
    placement: InlinePlacement,
    connection_id: u64,
    handle: &str,
) -> Entity {
    let texture = app
        .world_mut()
        .resource_mut::<Assets<Image>>()
        .add(Image::default());
    app.world_mut()
        .spawn((
            ChildOf(terminal),
            InlineWebview { view_id: handle.into(), instance_id: None, slot },
            placement,
            WebviewTextureTarget(texture),
            WebviewOwner { connection_id, handle: handle.into() },
        ))
        .id()
}
```

Then add the new tests:

```rust
#[test]
fn first_projection_sends_compositing_start() {
    let mut app = make_test_app();
    let terminal = app.world_mut().spawn(projection_grid(7)).id();
    let entity = spawn_owned_projection_child(
        &mut app,
        terminal,
        0,
        InlinePlacement {
            anchor: AnchorMode::Scrollback { line: 42, col: 3 },
            rows: 10,
            cols: 40,
            frame_seq: 7,
        },
        1,
        "h1",
    );

    let (writers, rx) = compositing_writers(1);
    app.world_mut().insert_resource(writers);

    project(&mut app);

    assert!(
        app.world().get::<CompositeNotified>(entity).is_some(),
        "first successful projection must stamp CompositeNotified"
    );
    let msg = rx.try_recv().expect("compositing start must be sent");
    let v: serde_json::Value = serde_json::from_str(&msg).unwrap();
    assert_eq!(v["op"], "compositing");
    assert_eq!(v["handle"], "h1");
    assert_eq!(v["active"], true);
}

#[test]
fn second_projection_does_not_resend() {
    let mut app = make_test_app();
    let terminal = app.world_mut().spawn(projection_grid(7)).id();
    spawn_owned_projection_child(
        &mut app,
        terminal,
        0,
        InlinePlacement {
            anchor: AnchorMode::Scrollback { line: 42, col: 3 },
            rows: 10,
            cols: 40,
            frame_seq: 7,
        },
        1,
        "h1",
    );

    let (writers, rx) = compositing_writers(1);
    app.world_mut().insert_resource(writers);

    project(&mut app);
    assert!(rx.try_recv().is_ok(), "first projection sends start");

    project(&mut app);
    assert!(rx.try_recv().is_err(), "second projection must not resend");
}

#[test]
fn stop_observer_sends_compositing_stop_when_notified() {
    let mut app = make_test_app();
    app.add_observer(on_placement_removed);
    let terminal = app.world_mut().spawn_empty().id();
    let entity = app.world_mut().spawn((
        ChildOf(terminal),
        InlinePlacement {
            anchor: AnchorMode::Scrollback { line: 1, col: 0 },
            rows: 4,
            cols: 10,
            frame_seq: 0,
        },
        WebviewOwner { connection_id: 1, handle: "h1".into() },
        CompositeNotified,
    )).id();

    let (writers, rx) = compositing_writers(1);
    app.world_mut().insert_resource(writers);

    app.world_mut().entity_mut(entity).despawn();
    app.world_mut().flush();

    let msg = rx.try_recv().expect("compositing stop must be sent");
    let v: serde_json::Value = serde_json::from_str(&msg).unwrap();
    assert_eq!(v["op"], "compositing");
    assert_eq!(v["handle"], "h1");
    assert_eq!(v["active"], false);
}

#[test]
fn stop_observer_does_not_send_when_not_notified() {
    let mut app = make_test_app();
    app.add_observer(on_placement_removed);
    let terminal = app.world_mut().spawn_empty().id();
    let entity = app.world_mut().spawn((
        ChildOf(terminal),
        InlinePlacement {
            anchor: AnchorMode::Scrollback { line: 1, col: 0 },
            rows: 4,
            cols: 10,
            frame_seq: 0,
        },
        WebviewOwner { connection_id: 1, handle: "h1".into() },
    )).id();

    let (writers, rx) = compositing_writers(1);
    app.world_mut().insert_resource(writers);

    app.world_mut().entity_mut(entity).despawn();
    app.world_mut().flush();

    assert!(rx.try_recv().is_err(), "stop must not be sent if start was never sent");
}
```

Also update `make_test_app()` to initialize `ConnectionWriters` so all existing tests keep passing (existing tests pass a no-op `ConnectionWriters` with no registered channels, so no actual sends occur):

```rust
fn make_test_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .init_resource::<DynamicRegistry>()
        .init_resource::<Assets<Image>>()
        .init_resource::<ConnectionWriters>()    // ← add this line
        .add_observer(on_osc_webview_request);
    app
}
```

Add these imports at the top of the `#[cfg(test)] mod tests` block (before the existing `use super::*;`-style imports):

```rust
use crate::control_plane::ConnectionWriters;
use crossbeam_channel::bounded;
```

- [ ] **Step 2: Run tests to confirm they fail**

```
cargo test -p ozmux-gui -- inline_webview 2>&1 | tail -30
```

Expected: `CompositeNotified` not found, `on_placement_removed` not found.

- [ ] **Step 3: Add `CompositeNotified` component and extend imports**

At the top of `src/inline_webview.rs`, make these two changes to the existing import block:

1. Extend the existing `use crate::control_plane::{...}` line to include `ConnectionWriters`:
   ```rust
   use crate::control_plane::{ConnectionWriters, DynSource, DynamicRegistry, NormalizedChord, WebviewOwner};
   ```
   (`WebviewOwner` is already imported — do not add it twice.)

2. Add a new import line for `PushMsg` (keep it in the same import block, adjacent to the other `crate::` imports):
   ```rust
   use crate::control_plane::protocol::PushMsg;
   ```

Add the new component before the `PassthroughKeys` definition:

```rust
/// Marks an inline webview entity after the first frame it successfully
/// projected into `TerminalOverlays`. Prevents re-sending the lifecycle
/// notification on every subsequent frame.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CompositeNotified;
```

- [ ] **Step 4: Extend `project_inline_overlays`**

Update the system signature to add `writers: Res<ConnectionWriters>` and extend the inline query:

```rust
fn project_inline_overlays(
    mut commands: Commands,
    writers: Res<ConnectionWriters>,
    terminals: Query<(
        Entity,
        &TerminalGrid,
        Option<&Children>,
        Has<TerminalOverlays>,
    )>,
    inline: Query<(
        &InlineWebview,
        &InlinePlacement,
        &WebviewTextureTarget,
        Has<CompositeNotified>,
        Option<&WebviewOwner>,
    )>,
) {
```

Then update the inner destructure from:
```rust
let Ok((view, placement, texture)) = inline.get(child) else {
```
to:
```rust
let Ok((view, placement, texture, already_notified, owner)) = inline.get(child) else {
```

And after the two lines that write `overlays.rects[slot]` and `overlays.textures[slot]`, add:

```rust
if !already_notified {
    if let Some(owner) = owner {
        let line = serde_json::to_string(&PushMsg::Compositing {
            handle: owner.handle.clone(),
            active: true,
        })
        .unwrap_or_default();
        writers.send(owner.connection_id, line);
    }
    commands.entity(child).insert(CompositeNotified);
}
```

- [ ] **Step 5: Add `on_placement_removed` observer and register it**

Add the observer function before the private helpers section (after `project_inline_overlays`):

```rust
/// Sends `{"op":"compositing","active":false}` when a composited inline webview
/// is removed (despawned or explicitly unmounted). Only fires if the entity was
/// previously stamped `CompositeNotified` — i.e., compositing had started.
fn on_placement_removed(
    trigger: On<OnRemove<InlinePlacement>>,
    writers: Res<ConnectionWriters>,
    query: Query<(&WebviewOwner, Has<CompositeNotified>)>,
) {
    let entity = trigger.target();
    if let Ok((owner, true)) = query.get(entity) {
        let line = serde_json::to_string(&PushMsg::Compositing {
            handle: owner.handle.clone(),
            active: false,
        })
        .unwrap_or_default();
        writers.send(owner.connection_id, line);
    }
}
```

In `OzmuxInlineWebviewPlugin::build()`, add the observer registration alongside the existing one:

```rust
app.add_observer(despawn_fixed_screen_on_alt_exit);
app.add_observer(on_placement_removed);
```

- [ ] **Step 6: Run tests to confirm they pass**

```
cargo test -p ozmux-gui -- inline_webview
```

Expected: all inline_webview tests PASS (including the 4 new ones).

- [ ] **Step 7: Run full build to confirm no compile errors**

```
cargo build
```

Expected: exits 0 with no errors.

- [ ] **Step 8: Commit**

```bash
git add src/inline_webview.rs
git commit -m "$(cat <<'EOF'
feat(inline_webview): send compositing lifecycle notifications

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: SDK session — compositing buffer in `FramePlacements`

**Files:**
- Modify: `sdk/ratatui-ozma/src/session.rs`

**Interfaces:**
- Consumes: NDJSON lines with `{"op":"compositing","handle":"...","active":bool}` from the reader thread
- Produces:
  - `FramePlacements::pending_compositing: HashMap<String, bool>` — drained by `Ozma::frame()` each call
  - `FramePlacements::take_compositing(&mut self, handle: &str) -> Option<bool>` — removes and returns the compositing state for a specific handle; consumed by `WebviewWidget::render()`

- [ ] **Step 1: Write failing tests**

Add to the `#[cfg(test)] mod tests` block in `sdk/ratatui-ozma/src/session.rs`:

```rust
#[test]
fn frame_drains_pending_compositing_into_frame_placements() {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    let pending: Arc<Mutex<HashMap<String, bool>>> = Arc::new(Mutex::new(HashMap::new()));
    pending.lock().unwrap().insert("h1".into(), true);

    // Simulate what Ozma::frame() must do: drain pending into FramePlacements.
    let mut frame = FramePlacements::default();
    let drained: HashMap<String, bool> = std::mem::take(&mut *pending.lock().unwrap());
    frame.pending_compositing = drained;

    assert_eq!(frame.take_compositing("h1"), Some(true));
    assert_eq!(frame.take_compositing("h1"), None, "take removes the entry");
}

#[test]
fn take_compositing_returns_none_for_unknown_handle() {
    let mut frame = FramePlacements::default();
    assert_eq!(frame.take_compositing("ghost"), None);
}

#[test]
fn frame_clear_resets_pending_compositing() {
    // Simulate: two events arrive in frame N but only one widget renders.
    // After ozma.frame() is called for frame N+1, the un-taken event must be gone.
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    let pending: Arc<Mutex<HashMap<String, bool>>> = Arc::new(Mutex::new(HashMap::new()));
    pending.lock().unwrap().insert("h1".into(), true);

    // Simulate first frame(): drain pending into FramePlacements.
    let mut frame = FramePlacements::default();
    frame.pending_compositing = std::mem::take(&mut *pending.lock().unwrap());
    // Widget does NOT call take_compositing("h1") — simulate a missed event.

    // Simulate second frame(): frame() clears and re-drains. pending is now empty.
    frame.placements.clear();
    frame.focused = None;
    frame.pending_compositing = std::mem::take(&mut *pending.lock().unwrap());

    assert_eq!(
        frame.take_compositing("h1"),
        None,
        "un-taken event from frame N must not survive into frame N+1"
    );
}
```

- [ ] **Step 2: Run tests to confirm they fail**

```
cargo test -p ratatui-ozma -- session 2>&1 | tail -20
```

Expected: `pending_compositing` field not found, `take_compositing` method not found.

- [ ] **Step 3: Add `pending_compositing` to `FramePlacements`**

`HashMap` is already imported in `sdk/ratatui-ozma/src/session.rs` — no import change needed.

Extend `FramePlacements`:

```rust
/// The per-frame collector handed to the [`crate::WebviewWidget`] as its state.
#[derive(Debug, Default)]
pub struct FramePlacements {
    placements: Vec<Placement>,
    focused: Option<String>,
    pub(crate) pending_compositing: HashMap<String, bool>,
}
```

Add `take_compositing` as the last method in `FramePlacements`'s `impl` block:

```rust
/// Removes and returns the pending compositing state for `handle`.
///
/// Called by [`crate::WebviewWidget::render`] to dispatch the
/// `on_compositing_change` callback. `None` when no event arrived this frame
/// for this handle.
pub(crate) fn take_compositing(&mut self, handle: &str) -> Option<bool> {
    self.pending_compositing.remove(handle)
}
```

- [ ] **Step 4: Add `pending_compositing` to `Ozma` and drain in `frame()`**

Add `pending_compositing: Arc<Mutex<HashMap<String, bool>>>` to the `Ozma` struct:

```rust
pub struct Ozma {
    writer: SharedWriter,
    pending: PendingRegisters,
    frame: Arc<Mutex<FramePlacements>>,
    pending_compositing: Arc<Mutex<HashMap<String, bool>>>,
}
```

In `Ozma::connect()`, initialize it and pass the clone into `spawn_reader`. Modify the construction block:

```rust
let pending_compositing: Arc<Mutex<HashMap<String, bool>>> =
    Arc::new(Mutex::new(HashMap::new()));

spawn_reader(
    stream,
    writer.clone(),
    handlers.clone(),
    pending.clone(),
    pending_compositing.clone(),
);

Ok(Self {
    writer,
    pending,
    frame: Arc::new(Mutex::new(FramePlacements::default())),
    pending_compositing,
})
```

Update `Ozma::frame()` to drain:

```rust
pub fn frame(&self) -> MutexGuard<'_, FramePlacements> {
    let mut frame = self.frame.lock().unwrap_or_else(|e| e.into_inner());
    frame.placements.clear();
    frame.focused = None;
    let mut pending = self
        .pending_compositing
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    frame.pending_compositing = std::mem::take(&mut *pending);
    frame
}
```

- [ ] **Step 5: Update `spawn_reader` to accept and parse compositing messages**

Add `pending_compositing: Arc<Mutex<HashMap<String, bool>>>` as the last parameter to `spawn_reader`:

```rust
fn spawn_reader(
    stream: UnixStream,
    writer: SharedWriter,
    handlers: HandlerRegistry,
    pending: PendingRegisters,
    pending_compositing: Arc<Mutex<HashMap<String, bool>>>,
) {
```

In the reader loop, after the existing `is_call` / `RegisterReply` branches, add a third branch to parse compositing events. Find the block:

```rust
let is_call = serde_json::from_str::<serde_json::Value>(trimmed)
    .ok()
    .map(|v| v["op"] == "call")
    .unwrap_or(false);
```

Replace the entire parse-and-dispatch block with the following (preserving the existing NOTE comment about handler-install ordering):

```rust
let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) else {
    continue;
};
if v["op"] == "call" {
    if let Ok(call) = serde_json::from_str::<IncomingCall>(trimmed) {
        dispatch_call(&writer, &handlers, call);
    }
} else if v["op"] == "compositing" {
    if let (Some(handle), Some(active)) = (v["handle"].as_str(), v["active"].as_bool()) {
        if let Ok(mut q) = pending_compositing.lock() {
            q.insert(handle.to_owned(), active);
        }
    }
} else if let Ok(reply) = serde_json::from_str::<RegisterReply>(trimmed)
    && let Some(reg) = pending.lock().ok().and_then(|mut q| q.pop_front())
{
    let outcome = if reply.ok {
        match reply.handle {
            // NOTE: install handlers under the minted handle on this thread,
            // before the next line is read, so a `call` pipelined right after
            // the reply finds its handlers rather than racing the registrant's
            // main thread.
            Some(h) => {
                if let Ok(mut map) = handlers.lock() {
                    map.insert(h.clone(), reg.handlers);
                }
                Ok(h)
            }
            None => Err(OzmaError::Register {
                reason: "missing handle".into(),
            }),
        }
    } else {
        Err(OzmaError::Register {
            reason: reply.error.unwrap_or_else(|| "unknown".into()),
        })
    };
    let _ = reg.reply.send(outcome);
}
```

- [ ] **Step 6: Run tests to confirm they pass**

```
cargo test -p ratatui-ozma -- session
```

Expected: all session tests PASS.

- [ ] **Step 7: Run full build**

```
cargo build
```

Expected: exits 0.

- [ ] **Step 8: Commit**

```bash
git add sdk/ratatui-ozma/src/session.rs
git commit -m "$(cat <<'EOF'
feat(ratatui-ozma/session): add compositing event buffer to FramePlacements

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: SDK widget — `on_compositing_change` callback

**Files:**
- Modify: `sdk/ratatui-ozma/src/widget.rs`

**Interfaces:**
- Consumes: `FramePlacements::take_compositing(handle: &str) -> Option<bool>` (from Task 3)
- Produces: `WebviewWidget::on_compositing_change(f: impl Fn(bool) + 'a) -> Self` — builder method; callback is called synchronously in `render()` when a pending compositing event exists for this handle

- [ ] **Step 1: Write failing tests**

Add to `#[cfg(test)] mod tests` in `sdk/ratatui-ozma/src/widget.rs`:

```rust
#[test]
fn on_compositing_change_fires_when_event_pending() {
    use std::cell::Cell;

    let area = Rect { x: 0, y: 0, width: 4, height: 1 };
    let mut buf = Buffer::empty(area);
    let mut state = FramePlacements::default();
    state.pending_compositing.insert("v".into(), true);

    let fired = Cell::new(false);
    WebviewWidget::new("v")
        .on_compositing_change(|active| {
            assert!(active, "active must be true");
            fired.set(true);
        })
        .render(area, &mut buf, &mut state);

    assert!(fired.get(), "callback must be called when event is pending");
    assert_eq!(state.take_compositing("v"), None, "event must be consumed");
}

#[test]
fn on_compositing_change_does_not_fire_when_no_event() {
    let area = Rect { x: 0, y: 0, width: 4, height: 1 };
    let mut buf = Buffer::empty(area);
    let mut state = FramePlacements::default();

    let fired = std::cell::Cell::new(false);
    WebviewWidget::new("v")
        .on_compositing_change(|_| fired.set(true))
        .render(area, &mut buf, &mut state);

    assert!(!fired.get(), "callback must not fire when no event is pending");
}

#[test]
fn on_compositing_change_is_carried_through_fallback_builder() {
    use std::cell::Cell;

    let area = Rect { x: 0, y: 0, width: 4, height: 1 };
    let mut buf = Buffer::empty(area);
    let mut state = FramePlacements::default();
    state.pending_compositing.insert("v".into(), false);

    let fired = Cell::new(Option::<bool>::None);
    WebviewWidget::new("v")
        .fallback(ratatui::widgets::Clear)
        .on_compositing_change(|active| fired.set(Some(active)))
        .render(area, &mut buf, &mut state);

    assert_eq!(fired.get(), Some(false), "callback must carry through fallback builder");
}
```

- [ ] **Step 2: Run tests to confirm they fail**

```
cargo test -p ratatui-ozma -- widget 2>&1 | tail -20
```

Expected: `on_compositing_change` method not found; `pending_compositing` field not accessible from test.

- [ ] **Step 3: Add `on_compositing_change` field and builder**

Update `WebviewWidget` struct to add the new field:

```rust
pub struct WebviewWidget<'a, W = WebviewDefaultPlaceholder> {
    handle: &'a str,
    fallback: W,
    focused: bool,
    on_compositing_change: Option<Box<dyn Fn(bool) + 'a>>,
}
```

Update the `new()` constructor to initialize the new field:

```rust
impl<'a> WebviewWidget<'a, WebviewDefaultPlaceholder> {
    /// Creates a widget for the given webview handle id.
    pub fn new(handle: &'a str) -> Self {
        Self {
            handle,
            fallback: WebviewDefaultPlaceholder,
            focused: false,
            on_compositing_change: None,
        }
    }
}
```

Update the `fallback()` builder to carry `on_compositing_change` through the type change:

```rust
pub fn fallback<W2: Widget>(self, widget: W2) -> WebviewWidget<'a, W2> {
    WebviewWidget {
        handle: self.handle,
        fallback: widget,
        focused: self.focused,
        on_compositing_change: self.on_compositing_change,
    }
}
```

Add the `on_compositing_change()` builder method after `focused()` in the `impl<'a, W>` block:

```rust
/// Registers a callback invoked when this webview's compositing state changes.
///
/// Called synchronously during [`StatefulWidget::render`] when a pending
/// compositing event is present for this handle.
pub fn on_compositing_change(mut self, f: impl Fn(bool) + 'a) -> Self {
    self.on_compositing_change = Some(Box::new(f));
    self
}
```

- [ ] **Step 4: Dispatch the callback in `render()`**

Update the `render()` implementation to dispatch after `state.record()`:

```rust
impl<W: Widget> StatefulWidget for WebviewWidget<'_, W> {
    type State = FramePlacements;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        Clear.render(area, buf);
        self.fallback.render(area, buf);
        state.record(self.handle.to_owned(), area);
        if self.focused {
            state.set_focused(self.handle.to_owned());
        }
        if let Some(active) = state.take_compositing(self.handle) {
            if let Some(cb) = &self.on_compositing_change {
                cb(active);
            }
        }
    }
}
```

- [ ] **Step 5: Run tests to confirm they pass**

```
cargo test -p ratatui-ozma -- widget
```

Expected: all widget tests PASS including the 3 new ones.

- [ ] **Step 6: Run full test suite for the crate**

```
cargo test -p ratatui-ozma
```

Expected: all tests PASS.

- [ ] **Step 7: Commit**

```bash
git add sdk/ratatui-ozma/src/widget.rs
git commit -m "$(cat <<'EOF'
feat(ratatui-ozma/widget): add on_compositing_change callback to WebviewWidget

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>
EOF
)"
```

---

### Task 5: ozmd app state and conditional fallback

**Files:**
- Modify: `apps/ozmd/src/app.rs`
- Modify: `apps/ozmd/src/ui.rs`

**Interfaces:**
- Consumes: `WebviewWidget::on_compositing_change` (from Task 4), `FramePlacements` (from Task 3)
- Produces: `App::compositing() -> bool`, `App::set_compositing(&self, bool)` — used in `draw_body` to conditionally suppress the fallback

- [ ] **Step 1: Write failing tests for `App`**

Add to the `#[cfg(test)] mod tests` block in `apps/ozmd/src/app.rs`:

```rust
#[test]
fn compositing_defaults_to_false() {
    let app = App::default();
    assert!(!app.compositing());
}

#[test]
fn set_compositing_updates_state() {
    let app = App::default();
    app.set_compositing(true);
    assert!(app.compositing());
    app.set_compositing(false);
    assert!(!app.compositing());
}

#[test]
fn set_compositing_takes_shared_ref() {
    // Verifies the Cell<bool> pattern: set_compositing takes &self so it can
    // be called while &App is borrowed elsewhere (e.g. in a draw closure).
    let app = App::default();
    let _borrow: &App = &app;
    app.set_compositing(true);  // must compile even with &app alive above
    assert!(app.compositing());
}
```

- [ ] **Step 2: Run tests to confirm they fail**

```
cargo test -p ozmd -- app 2>&1 | tail -20
```

Expected: `compositing` method not found.

- [ ] **Step 3: Add `compositing: Cell<bool>` to `App`**

Add `use std::cell::Cell;` to the import block at the top of `apps/ozmd/src/app.rs`.

Extend the `App` struct with the new field:

```rust
#[derive(Debug, Default)]
pub(crate) struct App {
    mode: Mode,
    pending_prefix: Option<char>,
    outline: Vec<Heading>,
    outline_open: bool,
    outline_selected: usize,
    current_heading_index: Option<usize>,
    search_query: String,
    search_active: bool,
    compositing: Cell<bool>,
}
```

Add the two accessor methods to the `impl App` block, after the existing public methods and before private helpers (keeping visibility ordering — `pub(crate)` before private):

```rust
/// Whether the inline webview is currently compositing.
pub(crate) fn compositing(&self) -> bool {
    self.compositing.get()
}

/// Updates the compositing state.
///
/// Takes `&self` via [`Cell`] so it can be called from the
/// `WebviewWidget::on_compositing_change` callback while `&App` is borrowed
/// in the draw closure.
pub(crate) fn set_compositing(&self, active: bool) {
    self.compositing.set(active);
}
```

- [ ] **Step 4: Run app tests to confirm they pass**

```
cargo test -p ozmd -- app
```

Expected: all app tests PASS.

- [ ] **Step 5: Update `draw_body` in `ui.rs` to conditionally suppress the fallback**

Replace the current `frame.render_stateful_widget(...)` call in `draw_body` with the conditional branches:

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

The two branches have different generic types (`WebviewWidget<WebviewDefaultPlaceholder>` vs. `WebviewWidget<Block>`), so each must call `render_stateful_widget` independently — they cannot be unified into one branch.

- [ ] **Step 6: Run full build**

```
cargo build
```

Expected: exits 0.

- [ ] **Step 7: Run all tests**

```
cargo test
```

Expected: all tests PASS with no regressions.

- [ ] **Step 8: Commit**

```bash
git add apps/ozmd/src/app.rs apps/ozmd/src/ui.rs
git commit -m "$(cat <<'EOF'
fix(ozmd): suppress loading fallback once webview starts compositing

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>
EOF
)"
```

---

## Verification

After all tasks complete:

1. **Build:** `cargo build` exits 0.
2. **Tests:** `cargo test` — no regressions across the workspace.
3. **Lint:** `cargo clippy --workspace` — no new warnings.
4. **Manual run in an ozmux terminal:**
   ```
   ozmd <some.md>
   ```
   - "loading…" border appears briefly.
   - Once the webview composites, the border disappears and only markdown content is visible.
5. **File reload (`r`):** "loading…" must NOT reappear — the webview stays mounted, `compositing` stays `true`.
6. **Process restart:** Fresh launch shows "loading…" again (compositing resets to `false`).
