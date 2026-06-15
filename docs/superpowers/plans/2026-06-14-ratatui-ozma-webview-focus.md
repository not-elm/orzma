# ratatui-ozma Webview Focus & Navigation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give ratatui-ozma apps an app-owned focus ring across native widgets and embedded webviews, where bare keys reach the focused webview but a reserved nav chord (default `Alt+h/j/k/l`) moves focus between widgets.

**Architecture:** "C fires, A delivers": focus is app-owned; the app drives the host's `FocusedWebview` via a new `focus`/`blur` control-plane op. While a webview is focused, a page-side glue (in the existing `ozmux_bridge.js` preload) intercepts the reserved nav chord and reports DOM focus/blur, forwarding both to the registering app over the existing `window.ozmux.call` RPC channel. `bevy_cef` and the `dispatch_focused_key` host hot path are untouched.

**Tech Stack:** Rust (SDK `sdk/ratatui-ozma` + host `src/control_plane`, `src/extension_render`), `crossbeam-channel` (already a dep), `ratatui` 0.29 (re-exports crossterm), browser JS (`ozmux_bridge.js`), serde NDJSON wire.

**Spec:** `docs/superpowers/specs/2026-06-14-ratatui-ozma-webview-focus-design.md`

**Conventions (from `.claude/rules/`):** no `mod.rs`; comments only `// TODO:` / `// NOTE:` / `// SAFETY:`; `///` doc on every `pub`; module `//!`; all `use` at top, one contiguous block; mutable params before immutable; private items last in a block; English comments only.

**Build/test commands:**
- SDK tests: `cargo test -p ratatui-ozma`
- Host tests: `cargo test -p ozmux-gui` (workspace root binary) or `cargo test` for all
- Lint/format: `cargo clippy --workspace --fix --allow-dirty --allow-staged && cargo fmt`

---

## File Structure

**SDK (`sdk/ratatui-ozma/src/`):**
- `protocol.rs` (modify) — add `ClientMsg::Focus { handle, instance }`.
- `webview.rs` (modify) — `WebviewHandle::focus` / `focus_instance`; reserve `__ozma.*` in `Webview::on`; private `on_reserved`.
- `session.rs` (modify) — `Ozma::blur`; `FramePlacements::record_native` + native rect access.
- `focus.rs` (create) — `FocusManager`, `Direction`, `FocusSync`, `Signal`, `resolve_spatial`, `nav_key`, `focusable`.
- `widget.rs` (modify) — `WebviewWidget::focused(bool)`.
- `lib.rs` (modify) — declare `mod focus;` and re-export the new public types.

**Host (`src/`):**
- `control_plane/protocol.rs` (modify) — add `ClientMsg::Focus { handle, instance }`.
- `control_plane/listener.rs` (modify) — `ControlEvent::SetFocus`; `handle_client_msg` arm.
- `control_plane.rs` (modify) — `apply_control_events` `SetFocus` arm sets `FocusedWebview`.
- `extension_render/ozmux_bridge.js` (modify) — nav-key interception, focus/blur reporting, `__ozma.keys` receipt, default keymap.
- `extension_render/preload.rs` (modify tests) — substring assertions for the new glue.

**Example/docs:**
- `sdk/ratatui-ozma/examples/ratatui_webview.rs` (modify) — demonstrate focus.

---

## Task 1: SDK protocol — `ClientMsg::Focus`

**Files:**
- Modify: `sdk/ratatui-ozma/src/protocol.rs`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `sdk/ratatui-ozma/src/protocol.rs`:

```rust
    #[test]
    fn focus_serializes_with_handle() {
        let line = serde_json::to_string(&ClientMsg::Focus {
            handle: Some("h1".into()),
            instance: None,
        })
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["op"], "focus");
        assert_eq!(v["handle"], "h1");
        assert_eq!(v["instance"], serde_json::Value::Null);
    }

    #[test]
    fn blur_serializes_with_null_handle() {
        let line = serde_json::to_string(&ClientMsg::Focus {
            handle: None,
            instance: None,
        })
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["op"], "focus");
        assert_eq!(v["handle"], serde_json::Value::Null);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ratatui-ozma focus_serializes_with_handle`
Expected: FAIL — `no variant named Focus`.

- [ ] **Step 3: Add the variant**

In `sdk/ratatui-ozma/src/protocol.rs`, add this arm to `enum ClientMsg` (after the `Emit { .. }` variant, before the closing `}`):

```rust
    /// Sets (or clears) the app-owned focus target. `handle: None` blurs any
    /// focused webview back to the app (native widget).
    Focus {
        /// The webview handle to focus, or `None` to blur.
        handle: Option<String>,
        /// The mount instance id, or `None` for the default instance.
        instance: Option<String>,
    },
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ratatui-ozma focus_serializes_with_handle blur_serializes_with_null_handle`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add sdk/ratatui-ozma/src/protocol.rs
git commit -m "feat(ratatui-ozma): add Focus op to SDK control-plane protocol"
```

---

## Task 2: SDK send API — `WebviewHandle::focus` / `Ozma::blur`

**Files:**
- Modify: `sdk/ratatui-ozma/src/webview.rs`
- Modify: `sdk/ratatui-ozma/src/session.rs`

- [ ] **Step 1: Write the failing test (webview.rs)**

Add to `#[cfg(test)] mod tests` in `sdk/ratatui-ozma/src/webview.rs`:

```rust
    #[test]
    fn focus_writes_focus_op_line() {
        use std::io::{BufRead, BufReader};
        use std::os::unix::net::UnixStream;
        let (a, b) = UnixStream::pair().unwrap();
        let writer = std::sync::Arc::new(std::sync::Mutex::new(a));
        let handle = WebviewHandle::new("view-1".to_owned(), writer);
        handle.focus().unwrap();
        let mut line = String::new();
        BufReader::new(b).read_line(&mut line).unwrap();
        let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(v["op"], "focus");
        assert_eq!(v["handle"], "view-1");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ratatui-ozma focus_writes_focus_op_line`
Expected: FAIL — `no method named focus`.

- [ ] **Step 3: Implement `focus` / `focus_instance` on `WebviewHandle`**

In `sdk/ratatui-ozma/src/webview.rs`, inside `impl WebviewHandle`, add these methods AFTER `emit` (keep `pub` API above private items; `emit` is the last public method today, so add these right after it). Reference the existing `emit` body for the write pattern:

```rust
    /// Requests host focus on this webview (default instance).
    ///
    /// The host sets `FocusedWebview` to this handle's mounted inline webview;
    /// keystrokes then reach the page natively until the app blurs or moves
    /// focus.
    pub fn focus(&self) -> OzmaResult<()> {
        self.send_focus(Some(self.id.clone()), None)
    }

    /// Requests host focus on a named mount instance of this webview.
    pub fn focus_instance(&self, instance: &str) -> OzmaResult<()> {
        self.send_focus(Some(self.id.clone()), Some(instance.to_owned()))
    }

    fn send_focus(&self, handle: Option<String>, instance: Option<String>) -> OzmaResult<()> {
        let line = serde_json::to_string(&ClientMsg::Focus { handle, instance })?;
        let mut w = self.writer.lock()?;
        writeln!(w, "{line}")?;
        w.flush()?;
        Ok(())
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p ratatui-ozma focus_writes_focus_op_line`
Expected: PASS.

- [ ] **Step 5: Write the failing test (session.rs)**

Add to `#[cfg(test)] mod tests` in `sdk/ratatui-ozma/src/session.rs` (create the test module if absent; if absent, add `#[cfg(test)] mod tests { use super::*; ... }` at end of file):

```rust
    #[test]
    fn blur_writes_focus_op_with_null_handle() {
        use std::io::{BufRead, BufReader};
        use std::os::unix::net::UnixStream;
        let (a, b) = UnixStream::pair().unwrap();
        let ozma = Ozma::from_writer_for_test(std::sync::Arc::new(std::sync::Mutex::new(a)));
        ozma.blur().unwrap();
        let mut line = String::new();
        BufReader::new(b).read_line(&mut line).unwrap();
        let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(v["op"], "focus");
        assert_eq!(v["handle"], serde_json::Value::Null);
    }
```

- [ ] **Step 6: Run test to verify it fails**

Run: `cargo test -p ratatui-ozma blur_writes_focus_op_with_null_handle`
Expected: FAIL — `no function from_writer_for_test` / `no method blur`.

- [ ] **Step 7: Implement `blur` and a test constructor on `Ozma`**

In `sdk/ratatui-ozma/src/session.rs`, add to `impl Ozma` AFTER `flush` (public methods, keep above any private fns):

```rust
    /// Clears the app-owned focus, blurring any focused webview back to the app.
    pub fn blur(&self) -> OzmaResult<()> {
        let line = serde_json::to_string(&ClientMsg::Focus {
            handle: None,
            instance: None,
        })?;
        let mut w = self.writer.lock()?;
        writeln!(w, "{line}")?;
        w.flush()?;
        Ok(())
    }
```

Then add a `#[cfg(test)]` constructor (place it at the very end of `impl Ozma`, after all real methods):

```rust
    #[cfg(test)]
    pub(crate) fn from_writer_for_test(writer: SharedWriter) -> Self {
        Self {
            writer,
            pending: Arc::new(Mutex::new(VecDeque::new())),
            frame: FramePlacements::default(),
            flush_state: FlushState::default(),
        }
    }
```

(`SharedWriter`, `Arc`, `Mutex`, `VecDeque`, `FramePlacements`, `FlushState` are already imported in `session.rs`.)

- [ ] **Step 8: Run tests to verify they pass**

Run: `cargo test -p ratatui-ozma blur_writes_focus_op_with_null_handle`
Expected: PASS.

- [ ] **Step 9: Commit**

```bash
git add sdk/ratatui-ozma/src/webview.rs sdk/ratatui-ozma/src/session.rs
git commit -m "feat(ratatui-ozma): WebviewHandle::focus and Ozma::blur senders"
```

---

## Task 3: SDK reserved `__ozma.*` namespace guard

**Files:**
- Modify: `sdk/ratatui-ozma/src/webview.rs`

- [ ] **Step 1: Write the failing test**

Add to `#[cfg(test)] mod tests` in `sdk/ratatui-ozma/src/webview.rs`:

```rust
    #[test]
    #[should_panic(expected = "__ozma.")]
    fn user_on_rejects_reserved_namespace() {
        let _ = Webview::inline("x").on("__ozma.nav", |(): ()| Ok::<_, crate::error::RpcError>(()));
    }

    #[test]
    fn on_reserved_installs_handler_under_reserved_name() {
        let wv = Webview::inline("x").on_reserved("__ozma.nav", |(d,): (String,)| {
            Ok::<_, crate::error::RpcError>(format!("nav:{d}"))
        });
        let h = wv.handlers.get("__ozma.nav").expect("reserved handler present");
        assert_eq!(h(vec![serde_json::json!("right")]).unwrap(), serde_json::json!("nav:right"));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ratatui-ozma user_on_rejects_reserved_namespace on_reserved_installs_handler_under_reserved_name`
Expected: FAIL — `user_on_rejects` does not panic; `on_reserved` undefined.

- [ ] **Step 3: Add the guard to `on` and a private `on_reserved`**

In `sdk/ratatui-ozma/src/webview.rs`, replace the body of `Webview::on` to reject the reserved prefix (keep the signature and doc), and add `on_reserved` right after it:

```rust
    /// Registers an RPC handler for `method`. The parameter is a tuple
    /// deserialized from the page's `window.ozmux.call(method, args)` array.
    ///
    /// # Panics
    /// Panics if `method` starts with the reserved `__ozma.` prefix, which is
    /// owned by the SDK's focus glue.
    pub fn on<P, R, F>(self, method: impl Into<String>, f: F) -> Self
    where
        P: DeserializeOwned,
        R: Serialize,
        F: Fn(P) -> Result<R, crate::error::RpcError> + Send + Sync + 'static,
    {
        let method = method.into();
        assert!(
            !method.starts_with("__ozma."),
            "method {method:?} uses the reserved __ozma. namespace"
        );
        self.on_reserved(method, f)
    }

    pub(crate) fn on_reserved<P, R, F>(mut self, method: impl Into<String>, f: F) -> Self
    where
        P: DeserializeOwned,
        R: Serialize,
        F: Fn(P) -> Result<R, crate::error::RpcError> + Send + Sync + 'static,
    {
        self.handlers.insert(method.into(), make_handler(f));
        self
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ratatui-ozma user_on_rejects_reserved_namespace on_reserved_installs_handler_under_reserved_name`
Expected: PASS (2 tests).

- [ ] **Step 5: Run the existing webview tests to confirm no regression**

Run: `cargo test -p ratatui-ozma --lib webview`
Expected: PASS (existing `on_registers_handler` etc. still green).

- [ ] **Step 6: Commit**

```bash
git add sdk/ratatui-ozma/src/webview.rs
git commit -m "feat(ratatui-ozma): reserve __ozma.* RPC namespace for focus glue"
```

---

## Task 4: SDK `focus.rs` — types, ring, `nav_key`

**Files:**
- Create: `sdk/ratatui-ozma/src/focus.rs`
- Modify: `sdk/ratatui-ozma/src/lib.rs`

- [ ] **Step 1: Declare the module and create the file skeleton**

In `sdk/ratatui-ozma/src/lib.rs`, add `mod focus;` to the module list (alphabetical, after `mod error;`):

```rust
mod error;
mod focus;
mod handler;
```

And add to the `pub use` block (only the types that exist after this task; `Signal` and `focusable` are added to this line in Task 6):

```rust
pub use focus::{Direction, FocusManager, FocusSync};
```

Create `sdk/ratatui-ozma/src/focus.rs` with this initial content:

```rust
//! App-owned focus ring across native ratatui widgets and embedded webviews,
//! plus the glue signal channel and the spatial-navigation resolver.

use crate::error::OzmaResult;
use crate::session::Ozma;
use crate::webview::{Webview, WebviewHandle};
use crossbeam_channel::{Receiver, Sender, unbounded};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;

/// A spatial navigation direction (vim `h/j/k/l`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Left (`h`).
    Left,
    /// Down (`j`).
    Down,
    /// Up (`k`).
    Up,
    /// Right (`l`).
    Right,
}

/// What the host must be told after a focus-ring change.
#[derive(Debug, Clone, PartialEq)]
pub enum FocusSync {
    /// Focus did not move; tell the host nothing.
    Unchanged,
    /// Focus moved onto a webview; the app should call `handle.focus()`.
    Focus(WebviewHandle),
    /// Focus moved onto a native widget; the app should call `ozma.blur()`.
    Blur,
}

impl FocusSync {
    /// Applies the sync by sending the matching control-plane op.
    pub fn apply(&self, ozma: &Ozma) -> OzmaResult<()> {
        match self {
            FocusSync::Unchanged => Ok(()),
            FocusSync::Focus(handle) => handle.focus(),
            FocusSync::Blur => ozma.blur(),
        }
    }
}
```

- [ ] **Step 2: Write the failing test for `nav_key`**

Append a test module to `sdk/ratatui-ozma/src/focus.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(c: char, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), mods)
    }

    #[test]
    fn nav_key_maps_alt_hjkl() {
        assert_eq!(FocusManager::nav_key(&key('h', KeyModifiers::ALT)), Some(Direction::Left));
        assert_eq!(FocusManager::nav_key(&key('j', KeyModifiers::ALT)), Some(Direction::Down));
        assert_eq!(FocusManager::nav_key(&key('k', KeyModifiers::ALT)), Some(Direction::Up));
        assert_eq!(FocusManager::nav_key(&key('l', KeyModifiers::ALT)), Some(Direction::Right));
    }

    #[test]
    fn nav_key_ignores_bare_hjkl() {
        assert_eq!(FocusManager::nav_key(&key('h', KeyModifiers::NONE)), None);
        assert_eq!(FocusManager::nav_key(&key('x', KeyModifiers::ALT)), None);
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p ratatui-ozma nav_key_maps_alt_hjkl`
Expected: FAIL — `FocusManager` undefined.

- [ ] **Step 4: Implement `FocusManager` skeleton + `nav_key`**

Insert BEFORE the test module in `sdk/ratatui-ozma/src/focus.rs`:

```rust
/// A glue signal delivered from a webview page over the reserved `__ozma.*` RPC.
#[derive(Debug, Clone, PartialEq)]
enum Signal {
    /// The page requested a directional focus move (handle, direction).
    Nav(String, Direction),
    /// The page reported a DOM focus change (handle, focused?).
    Focus(String, bool),
}

#[derive(Debug, Clone, PartialEq)]
enum ItemKind {
    Native,
    Webview(WebviewHandle),
}

#[derive(Debug, Clone)]
struct Item {
    id: String,
    kind: ItemKind,
    rect: Option<Rect>,
}

/// An app-owned focus ring across native widgets and webviews.
///
/// The app registers focusable items, feeds it nav keys (when a native widget
/// is focused) and drained glue signals (when a webview is focused), and renders
/// using [`FocusManager::is_focused`]. Each transition yields a [`FocusSync`] the
/// app applies to keep the host's `FocusedWebview` in step.
pub struct FocusManager {
    items: Vec<Item>,
    focused: Option<usize>,
    tx: Sender<Signal>,
    rx: Receiver<Signal>,
}

impl FocusManager {
    /// Creates an empty focus ring.
    pub fn new() -> Self {
        let (tx, rx) = unbounded();
        Self {
            items: Vec::new(),
            focused: None,
            tx,
            rx,
        }
    }

    /// Maps a reserved nav chord to a [`Direction`] (default `Alt+h/j/k/l`).
    pub fn nav_key(key: &KeyEvent) -> Option<Direction> {
        if !key.modifiers.contains(KeyModifiers::ALT) {
            return None;
        }
        match key.code {
            KeyCode::Char('h') => Some(Direction::Left),
            KeyCode::Char('j') => Some(Direction::Down),
            KeyCode::Char('k') => Some(Direction::Up),
            KeyCode::Char('l') => Some(Direction::Right),
            _ => None,
        }
    }
}

impl Default for FocusManager {
    fn default() -> Self {
        Self::new()
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p ratatui-ozma nav_key`
Expected: PASS (2 tests). The unused `tx`/`rx`/`Item`/`Signal`/`ItemKind` may warn; that is fine for this task (used in Task 6). Allow warnings for now.

- [ ] **Step 6: Commit**

```bash
git add sdk/ratatui-ozma/src/focus.rs sdk/ratatui-ozma/src/lib.rs
git commit -m "feat(ratatui-ozma): FocusManager skeleton + Alt+hjkl nav_key"
```

---

## Task 5: SDK spatial resolver `resolve_spatial`

**Files:**
- Modify: `sdk/ratatui-ozma/src/focus.rs`

- [ ] **Step 1: Write the failing test**

Add to the `mod tests` block in `sdk/ratatui-ozma/src/focus.rs`:

```rust
    fn rect(x: u16, y: u16, w: u16, h: u16) -> Rect {
        Rect { x, y, width: w, height: h }
    }

    #[test]
    fn resolve_picks_neighbor_in_pushed_direction() {
        let from = rect(0, 0, 10, 5);
        let right = rect(12, 0, 10, 5);
        let down = rect(0, 6, 10, 5);
        let cands = vec![(1usize, right), (2usize, down)];
        assert_eq!(resolve_spatial(&cands, from, Direction::Right), Some(1));
        assert_eq!(resolve_spatial(&cands, from, Direction::Down), Some(2));
    }

    #[test]
    fn resolve_filters_out_half_plane_and_breaks_ties_by_index() {
        let from = rect(10, 10, 10, 5);
        // Two candidates equally to the right; lower index wins.
        let a = rect(22, 10, 4, 5);
        let b = rect(22, 10, 4, 5);
        let cands = vec![(5usize, a), (3usize, b)];
        assert_eq!(resolve_spatial(&cands, from, Direction::Right), Some(3));
        // Nothing to the left -> None.
        assert_eq!(resolve_spatial(&cands, from, Direction::Left), None);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ratatui-ozma resolve_picks_neighbor_in_pushed_direction`
Expected: FAIL — `resolve_spatial` undefined.

- [ ] **Step 3: Implement `resolve_spatial`**

Add to `sdk/ratatui-ozma/src/focus.rs` (private free function, place after the `impl Default for FocusManager` block, before `#[cfg(test)]`):

```rust
/// Orthogonal-displacement weight in the directional cost (smart-TV LRUD tuning).
const ORTHOGONAL_WEIGHT: f32 = 0.4;

/// Resolves the nearest focus candidate in the pushed `dir` from `from`.
///
/// Filters to candidates strictly beyond `from`'s far edge in `dir` (half-plane),
/// then minimizes `primary_gap + ORTHOGONAL_WEIGHT * orthogonal_displacement`.
/// Ties resolve to the lowest candidate index for determinism. Returns the
/// winning candidate's index, or `None` when the half-plane is empty.
fn resolve_spatial(candidates: &[(usize, Rect)], from: Rect, dir: Direction) -> Option<usize> {
    let f = Edges::of(from);
    let mut best: Option<(usize, f32)> = None;
    for (idx, rect) in candidates {
        let c = Edges::of(*rect);
        let cost = match dir {
            Direction::Right if c.left >= f.right => {
                (c.left - f.right) as f32 + ORTHOGONAL_WEIGHT * (c.cy - f.cy).abs()
            }
            Direction::Left if c.right <= f.left => {
                (f.left - c.right) as f32 + ORTHOGONAL_WEIGHT * (c.cy - f.cy).abs()
            }
            Direction::Down if c.top >= f.bottom => {
                (c.top - f.bottom) as f32 + ORTHOGONAL_WEIGHT * (c.cx - f.cx).abs()
            }
            Direction::Up if c.bottom <= f.top => {
                (f.top - c.bottom) as f32 + ORTHOGONAL_WEIGHT * (c.cx - f.cx).abs()
            }
            _ => continue,
        };
        match best {
            Some((_, best_cost)) if cost >= best_cost => {}
            _ => best = Some((*idx, cost)),
        }
    }
    best.map(|(idx, _)| idx)
}

/// Edge coordinates of a rect as floats (centers included), for cost math.
struct Edges {
    left: f32,
    right: f32,
    top: f32,
    bottom: f32,
    cx: f32,
    cy: f32,
}

impl Edges {
    fn of(r: Rect) -> Self {
        let left = r.x as f32;
        let top = r.y as f32;
        let right = left + r.width as f32;
        let bottom = top + r.height as f32;
        Self {
            left,
            right,
            top,
            bottom,
            cx: left + r.width as f32 / 2.0,
            cy: top + r.height as f32 / 2.0,
        }
    }
}
```

NOTE: the tie test relies on strict `>=` so the FIRST-seen lowest cost wins; iterate candidates in their given order, and the test passes index `3` after `5`, so ensure the winner is the lowest-cost-then-first-seen. Because both costs are equal, the first candidate seen (`5`) would win with this code. To make the LOWEST INDEX win on ties, sort candidates by index first. Replace the loop preamble: before iterating, bind `let mut sorted: Vec<(usize, Rect)> = candidates.to_vec(); sorted.sort_by_key(|(i, _)| *i);` and iterate `&sorted`.

- [ ] **Step 4: Apply the tie-break sort**

Edit the start of `resolve_spatial` so the body iterates a by-index-sorted copy:

```rust
fn resolve_spatial(candidates: &[(usize, Rect)], from: Rect, dir: Direction) -> Option<usize> {
    let f = Edges::of(from);
    let mut sorted: Vec<(usize, Rect)> = candidates.to_vec();
    sorted.sort_by_key(|(i, _)| *i);
    let mut best: Option<(usize, f32)> = None;
    for (idx, rect) in &sorted {
        // ... (loop body unchanged) ...
    }
    best.map(|(idx, _)| idx)
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p ratatui-ozma resolve_picks_neighbor_in_pushed_direction resolve_filters_out_half_plane_and_breaks_ties_by_index`
Expected: PASS (2 tests).

- [ ] **Step 6: Commit**

```bash
git add sdk/ratatui-ozma/src/focus.rs
git commit -m "feat(ratatui-ozma): spatial focus resolver (half-plane + weighted cost)"
```

---

## Task 6: SDK FocusManager ring ops, glue instrumentation, drain

**Files:**
- Modify: `sdk/ratatui-ozma/src/focus.rs`

- [ ] **Step 1: Write the failing tests**

Add to the `mod tests` block in `sdk/ratatui-ozma/src/focus.rs`:

```rust
    fn pair_handle(id: &str) -> WebviewHandle {
        use std::os::unix::net::UnixStream;
        use std::sync::{Arc, Mutex};
        let (a, _b) = UnixStream::pair().unwrap();
        WebviewHandle::new(id.to_owned(), Arc::new(Mutex::new(a)))
    }

    #[test]
    fn navigate_native_to_webview_returns_focus_sync() {
        let mut fm = FocusManager::new();
        fm.add_native_at("left", rect(0, 0, 10, 5));
        fm.add_webview_at("right", pair_handle("view-r"), rect(12, 0, 10, 5));
        // Focus starts on the first item (native "left").
        assert!(fm.is_focused("left"));
        assert!(fm.focused_is_native());
        let sync = fm.navigate(Direction::Right);
        assert!(matches!(sync, FocusSync::Focus(_)));
        assert!(fm.is_focused("right"));
        assert!(!fm.focused_is_native());
    }

    #[test]
    fn navigate_webview_to_native_returns_blur() {
        let mut fm = FocusManager::new();
        fm.add_webview_at("left", pair_handle("view-l"), rect(0, 0, 10, 5));
        fm.add_native_at("right", rect(12, 0, 10, 5));
        let _ = fm.navigate(Direction::Right); // -> native "right"
        let sync = fm.navigate(Direction::Left); // back to webview
        assert!(matches!(sync, FocusSync::Focus(_)));
        let blur = fm.navigate(Direction::Right);
        assert_eq!(blur, FocusSync::Blur);
    }

    #[test]
    fn navigate_no_neighbor_is_unchanged() {
        let mut fm = FocusManager::new();
        fm.add_native_at("only", rect(0, 0, 10, 5));
        assert_eq!(fm.navigate(Direction::Right), FocusSync::Unchanged);
    }

    #[test]
    fn drain_applies_nav_signal_from_focused_webview() {
        let mut fm = FocusManager::new();
        fm.add_webview_at("wv", pair_handle("view-x"), rect(0, 0, 10, 5));
        fm.add_native_at("native", rect(12, 0, 10, 5));
        let _ = fm.navigate(Direction::Right); // focus native first? no: start is wv
        // Reset: start focus is "wv". Push a nav-right signal from the page.
        fm.signal_sender()
            .send(Signal::Nav("view-x".into(), Direction::Right))
            .unwrap();
        let syncs = fm.drain();
        assert_eq!(syncs, vec![FocusSync::Blur]);
        assert!(fm.is_focused("native"));
    }

    #[test]
    fn drain_focus_report_reconciles_click() {
        let mut fm = FocusManager::new();
        fm.add_native_at("native", rect(0, 0, 10, 5));
        fm.add_webview_at("wv", pair_handle("view-x"), rect(12, 0, 10, 5));
        assert!(fm.is_focused("native"));
        fm.signal_sender()
            .send(Signal::Focus("view-x".into(), true))
            .unwrap();
        let _ = fm.drain();
        assert!(fm.is_focused("wv"));
    }
```

NOTE: the `drain_applies_nav_signal_from_focused_webview` test comment about navigate is misleading — delete the `let _ = fm.navigate(...)` line; the first-added item ("wv") is already focused. Keep only the signal push + drain.

- [ ] **Step 2: Fix the test as noted**

Remove the `let _ = fm.navigate(Direction::Right); // focus native first? no: start is wv` line from `drain_applies_nav_signal_from_focused_webview` so the test reads: add items, push `Signal::Nav("view-x", Right)`, `drain()`, assert `vec![FocusSync::Blur]` and `is_focused("native")`.

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p ratatui-ozma navigate_native_to_webview_returns_focus_sync`
Expected: FAIL — `add_native_at` undefined.

- [ ] **Step 4: Implement ring ops + drain + `signal_sender`**

Add these methods inside `impl FocusManager` (after `nav_key`, keeping public methods grouped before private; all of these are `pub` except where noted):

```rust
    /// Registers a native widget without geometry (Tab-style only).
    pub fn add_native(&mut self, id: impl Into<String>) {
        self.push(Item { id: id.into(), kind: ItemKind::Native, rect: None });
    }

    /// Registers a native widget with its current layout rect (spatial nav).
    pub fn add_native_at(&mut self, id: impl Into<String>, rect: Rect) {
        self.push(Item { id: id.into(), kind: ItemKind::Native, rect: Some(rect) });
    }

    /// Registers a webview widget with its current layout rect.
    pub fn add_webview_at(&mut self, id: impl Into<String>, handle: WebviewHandle, rect: Rect) {
        self.push(Item { id: id.into(), kind: ItemKind::Webview(handle), rect: Some(rect) });
    }

    /// Updates the recorded rect of a registered item (call each frame).
    pub fn set_rect(&mut self, id: &str, rect: Rect) {
        if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
            item.rect = Some(rect);
        }
    }

    /// Returns a sender for reserved-glue signals; clone into webview handlers.
    pub fn signal_sender(&self) -> Sender<Signal> {
        self.tx.clone()
    }

    /// Whether `id` is the currently-focused item.
    pub fn is_focused(&self, id: &str) -> bool {
        self.focused.is_some_and(|i| self.items[i].id == id)
    }

    /// Whether the focused item is a native widget (or nothing is focused).
    pub fn focused_is_native(&self) -> bool {
        match self.focused {
            Some(i) => matches!(self.items[i].kind, ItemKind::Native),
            None => true,
        }
    }

    /// Moves focus spatially in `dir`, returning the host sync to apply.
    pub fn navigate(&mut self, dir: Direction) -> FocusSync {
        let Some(from_idx) = self.focused else {
            return FocusSync::Unchanged;
        };
        let Some(from) = self.items[from_idx].rect else {
            return FocusSync::Unchanged;
        };
        let candidates: Vec<(usize, Rect)> = self
            .items
            .iter()
            .enumerate()
            .filter(|(i, item)| *i != from_idx && item.rect.is_some())
            .map(|(i, item)| (i, item.rect.unwrap()))
            .collect();
        match resolve_spatial(&candidates, from, dir) {
            Some(next) => self.focus_index(next),
            None => FocusSync::Unchanged,
        }
    }

    /// Drains queued glue signals, applying each to the ring; returns the syncs
    /// (in order) the app must apply to the host.
    pub fn drain(&mut self) -> Vec<FocusSync> {
        let mut out = Vec::new();
        while let Ok(signal) = self.rx.try_recv() {
            let sync = match signal {
                Signal::Nav(handle, dir) => {
                    if self.is_focused_handle(&handle) {
                        self.navigate(dir)
                    } else {
                        FocusSync::Unchanged
                    }
                }
                Signal::Focus(handle, true) => match self.index_of_handle(&handle) {
                    Some(idx) => self.focus_index(idx),
                    None => FocusSync::Unchanged,
                },
                Signal::Focus(handle, false) => {
                    if self.is_focused_handle(&handle) {
                        self.focus_first_native()
                    } else {
                        FocusSync::Unchanged
                    }
                }
            };
            if !matches!(sync, FocusSync::Unchanged) {
                out.push(sync);
            }
        }
        out
    }
```

Then add the private helpers at the BOTTOM of `impl FocusManager` (private items last):

```rust
    fn push(&mut self, item: Item) {
        self.items.push(item);
        if self.focused.is_none() {
            self.focused = Some(self.items.len() - 1);
        }
    }

    fn focus_index(&mut self, idx: usize) -> FocusSync {
        if self.focused == Some(idx) {
            return FocusSync::Unchanged;
        }
        self.focused = Some(idx);
        match &self.items[idx].kind {
            ItemKind::Webview(handle) => FocusSync::Focus(handle.clone()),
            ItemKind::Native => FocusSync::Blur,
        }
    }

    fn focus_first_native(&mut self) -> FocusSync {
        match self.items.iter().position(|i| matches!(i.kind, ItemKind::Native)) {
            Some(idx) => self.focus_index(idx),
            None => FocusSync::Unchanged,
        }
    }

    fn index_of_handle(&self, handle: &str) -> Option<usize> {
        self.items.iter().position(|i| match &i.kind {
            ItemKind::Webview(h) => h.id() == handle,
            ItemKind::Native => false,
        })
    }

    fn is_focused_handle(&self, handle: &str) -> bool {
        self.focused
            .and_then(|i| match &self.items[i].kind {
                ItemKind::Webview(h) => Some(h.id() == handle),
                ItemKind::Native => Some(false),
            })
            .unwrap_or(false)
    }
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p ratatui-ozma --lib focus`
Expected: PASS (all focus tests).

- [ ] **Step 6: Implement `focusable` glue-instrumentation helper**

Add this PUBLIC free function to `sdk/ratatui-ozma/src/focus.rs` (after the `Edges` impl, before `#[cfg(test)]`):

```rust
/// Instruments a [`Webview`] with the reserved `__ozma.nav` / `__ozma.focus`
/// handlers that forward page glue signals into a [`FocusManager`].
///
/// Pass `fm.signal_sender()`. Call before `Ozma::register`. The page's
/// `location.hostname` (its minted handle) is the first RPC arg, so signals
/// route to the right ring item even with multiple webviews.
pub fn focusable(view: Webview, tx: Sender<Signal>) -> Webview {
    let nav_tx = tx.clone();
    let view = view.on_reserved("__ozma.nav", move |(handle, dir): (String, String)| {
        let direction = match dir.as_str() {
            "left" => Direction::Left,
            "down" => Direction::Down,
            "up" => Direction::Up,
            "right" => Direction::Right,
            other => return Err(crate::error::RpcError::new(format!("bad dir: {other}"))),
        };
        let _ = nav_tx.send(Signal::Nav(handle, direction));
        Ok::<_, crate::error::RpcError>(())
    });
    view.on_reserved("__ozma.focus", move |(handle, focused): (String, bool)| {
        let _ = tx.send(Signal::Focus(handle, focused));
        Ok::<_, crate::error::RpcError>(())
    })
}
```

NOTE: `Signal` is private to the module but `focusable` and `signal_sender` both expose `Sender<Signal>` in their public signatures. Make `Signal` `pub` (change `enum Signal` to `pub enum Signal`) and re-export it: add `Signal` to the `pub use focus::{...}` line in `lib.rs`. Its variants stay an opaque detail for users (they only ever pass the sender through), but the type must be nameable.

- [ ] **Step 7: Make `Signal` public and re-export**

In `sdk/ratatui-ozma/src/focus.rs` change `enum Signal` to `pub enum Signal` and add a doc line `/// An opaque glue signal forwarded from a webview page to a `FocusManager`.` above it, with `///` docs on each variant (e.g. `/// A directional focus-move request.` and `/// A DOM focus-change report.`).

In `sdk/ratatui-ozma/src/lib.rs` update the focus re-export:

```rust
pub use focus::{Direction, FocusManager, FocusSync, Signal, focusable};
```

- [ ] **Step 8: Write the failing test for `focusable`**

Add to the `mod tests` block:

```rust
    #[test]
    fn focusable_installs_reserved_handlers_that_feed_the_channel() {
        let fm = FocusManager::new();
        let view = focusable(Webview::inline("x"), fm.signal_sender());
        let nav = view.handlers_for_test().get("__ozma.nav").expect("nav handler");
        nav(vec![serde_json::json!("view-x"), serde_json::json!("right")]).unwrap();
        match fm.rx_for_test().try_recv().unwrap() {
            Signal::Nav(h, d) => {
                assert_eq!(h, "view-x");
                assert_eq!(d, Direction::Right);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }
```

- [ ] **Step 9: Add the test accessors**

In `sdk/ratatui-ozma/src/webview.rs`, add to `impl Webview` (test-only, at the very end of the impl):

```rust
    #[cfg(test)]
    pub(crate) fn handlers_for_test(
        &self,
    ) -> &std::collections::HashMap<String, crate::handler::BoxedHandler> {
        &self.handlers
    }
```

In `sdk/ratatui-ozma/src/focus.rs`, add to `impl FocusManager` (test-only, at the very end of the impl):

```rust
    #[cfg(test)]
    pub(crate) fn rx_for_test(&self) -> &Receiver<Signal> {
        &self.rx
    }
```

- [ ] **Step 10: Run tests to verify they pass**

Run: `cargo test -p ratatui-ozma --lib focus`
Expected: PASS (all focus tests incl. `focusable_installs_reserved_handlers_that_feed_the_channel`).

- [ ] **Step 11: Commit**

```bash
git add sdk/ratatui-ozma/src/focus.rs sdk/ratatui-ozma/src/webview.rs sdk/ratatui-ozma/src/lib.rs
git commit -m "feat(ratatui-ozma): FocusManager ring ops, drain, and focusable glue wiring"
```

---

## Task 7: SDK native-rect recording + `WebviewWidget::focused`

**Files:**
- Modify: `sdk/ratatui-ozma/src/session.rs`
- Modify: `sdk/ratatui-ozma/src/widget.rs`

- [ ] **Step 1: Write the failing test (widget focused flag)**

Add to `#[cfg(test)] mod tests` in `sdk/ratatui-ozma/src/widget.rs`:

```rust
    #[test]
    fn focused_widget_constructs() {
        let area = Rect { x: 0, y: 0, width: 4, height: 1 };
        let mut buf = Buffer::empty(area);
        let mut state = FramePlacements::default();
        WebviewWidget::new("v").focused(true).render(area, &mut buf, &mut state);
        assert_eq!(state.placements_for_test().len(), 1);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ratatui-ozma focused_widget_constructs`
Expected: FAIL — `no method named focused`.

- [ ] **Step 3: Add the `focused` builder to `WebviewWidget`**

In `sdk/ratatui-ozma/src/widget.rs`, add a `focused: bool` field to the struct and the builder. Update the struct:

```rust
pub struct WebviewWidget<'a, W = Blank> {
    handle: &'a str,
    fallback: W,
    focused: bool,
}
```

Update `WebviewWidget::new` and `fallback` to carry `focused`, and add the `focused` builder:

```rust
impl<'a> WebviewWidget<'a, Blank> {
    /// Creates a widget for the given webview handle id.
    pub fn new(handle: &'a str) -> Self {
        Self {
            handle,
            fallback: Blank,
            focused: false,
        }
    }
}

impl<'a, W> WebviewWidget<'a, W> {
    /// Sets a fallback widget painted into the cells under the webview.
    pub fn fallback<W2: Widget>(self, widget: W2) -> WebviewWidget<'a, W2> {
        WebviewWidget {
            handle: self.handle,
            fallback: widget,
            focused: self.focused,
        }
    }

    /// Marks the widget focused, a hint for drawing a focus frame/title around
    /// the webview (the page content itself is composited by the host).
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// Whether this widget is currently focused.
    pub fn is_focused(&self) -> bool {
        self.focused
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p ratatui-ozma focused_widget_constructs`
Expected: PASS. Also run `cargo test -p ratatui-ozma --lib widget` to confirm `records_placement_and_blanks_cells` and `fallback_is_painted` still pass.

- [ ] **Step 5: Write the failing test (native rect recording)**

Add to `#[cfg(test)] mod tests` in `sdk/ratatui-ozma/src/session.rs`:

```rust
    #[test]
    fn record_native_collects_native_rects() {
        let mut frame = FramePlacements::default();
        frame.record_native("editor".into(), Rect { x: 1, y: 2, width: 3, height: 4 });
        let natives = frame.native_rects_for_test();
        assert_eq!(natives.len(), 1);
        assert_eq!(natives[0].0, "editor");
        assert_eq!(natives[0].1, Rect { x: 1, y: 2, width: 3, height: 4 });
    }
```

(If `Rect` is not imported in the session test module, add `use ratatui::layout::Rect;` to that test module's `use` lines.)

- [ ] **Step 6: Run test to verify it fails**

Run: `cargo test -p ratatui-ozma record_native_collects_native_rects`
Expected: FAIL — `no method named record_native`.

- [ ] **Step 7: Add native-rect recording to `FramePlacements`**

In `sdk/ratatui-ozma/src/session.rs`, extend `FramePlacements`. Update the struct and the `record`/clear paths:

```rust
/// The per-frame collector handed to the [`crate::WebviewWidget`] as its state.
#[derive(Debug, Default)]
pub struct FramePlacements {
    placements: Vec<Placement>,
    natives: Vec<(String, Rect)>,
}

impl FramePlacements {
    pub(crate) fn record(&mut self, handle: String, area: Rect) {
        self.placements.push(Placement { handle, area });
    }

    /// Records a native widget's rect this frame (for spatial focus resolution).
    pub fn record_native(&mut self, id: String, area: Rect) {
        self.natives.push((id, area));
    }

    /// Returns the native widget rects recorded this frame.
    pub fn native_rects(&self) -> &[(String, Rect)] {
        &self.natives
    }

    #[cfg(test)]
    pub(crate) fn native_rects_for_test(&self) -> &[(String, Rect)] {
        &self.natives
    }

    #[cfg(test)]
    pub(crate) fn placements_for_test(&self) -> &[Placement] {
        &self.placements
    }
}
```

Then find where `Ozma::frame` clears placements (`self.frame.placements.clear();`) and ALSO clear natives:

```rust
    /// Returns the per-frame placement collector, cleared, for `render_stateful_widget`.
    pub fn frame(&mut self) -> &mut FramePlacements {
        self.frame.placements.clear();
        self.frame.natives.clear();
        &mut self.frame
    }
```

- [ ] **Step 8: Run tests to verify they pass**

Run: `cargo test -p ratatui-ozma --lib`
Expected: PASS (whole SDK lib suite).

- [ ] **Step 9: Commit**

```bash
git add sdk/ratatui-ozma/src/session.rs sdk/ratatui-ozma/src/widget.rs
git commit -m "feat(ratatui-ozma): native rect recording + WebviewWidget::focused"
```

---

## Task 8: Host protocol — `ClientMsg::Focus`

**Files:**
- Modify: `src/control_plane/protocol.rs`

- [ ] **Step 1: Write the failing test**

Add to `#[cfg(test)] mod tests` in `src/control_plane/protocol.rs`:

```rust
    #[test]
    fn parses_focus_with_handle() {
        let m: ClientMsg =
            serde_json::from_str(r#"{"op":"focus","handle":"h1","instance":null}"#).unwrap();
        assert_eq!(
            m,
            ClientMsg::Focus {
                handle: Some("h1".into()),
                instance: None,
            }
        );
    }

    #[test]
    fn parses_blur_with_null_handle() {
        let m: ClientMsg = serde_json::from_str(r#"{"op":"focus","handle":null}"#).unwrap();
        assert_eq!(
            m,
            ClientMsg::Focus {
                handle: None,
                instance: None,
            }
        );
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ozmux-gui parses_focus_with_handle`
Expected: FAIL — `no variant named Focus`.

- [ ] **Step 3: Add the variant**

In `src/control_plane/protocol.rs`, add to `enum ClientMsg` (after `Emit { .. }`):

```rust
    /// Sets (or clears, with `handle: None`) the app-owned focus target for
    /// this connection's surface.
    Focus {
        /// The handle to focus, or `None` to blur.
        #[serde(default)]
        handle: Option<String>,
        /// The mount instance id, or `None` for the default instance.
        #[serde(default)]
        instance: Option<String>,
    },
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ozmux-gui parses_focus_with_handle parses_blur_with_null_handle`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add src/control_plane/protocol.rs
git commit -m "feat(control-plane): parse Focus op"
```

---

## Task 9: Host listener — `ControlEvent::SetFocus`

**Files:**
- Modify: `src/control_plane/listener.rs`

- [ ] **Step 1: Write the failing test**

Add to `#[cfg(test)] mod tests` in `src/control_plane/listener.rs`:

```rust
    #[test]
    fn client_focus_line_emits_a_set_focus_event() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("ctl.sock");
        let tokens = TokenRegistry::default();
        let surface = Entity::from_bits(7);
        tokens.insert("tok", surface);
        let events = spawn_listener(&sock, tokens, ConnectionWriters::default()).unwrap();

        let mut client = UnixStream::connect(&sock).unwrap();
        writeln!(client, r#"{{"op":"hello","token":"tok"}}"#).unwrap();
        writeln!(client, r#"{{"op":"focus","handle":"h1","instance":null}}"#).unwrap();
        client.flush().unwrap();

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Ok(ControlEvent::SetFocus {
                owner_surface,
                handle,
                ..
            }) = events.recv_timeout(Duration::from_millis(50))
            {
                assert_eq!(owner_surface, surface);
                assert_eq!(handle.as_deref(), Some("h1"));
                break;
            }
            assert!(Instant::now() < deadline, "no SetFocus within 2s");
        }
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ozmux-gui client_focus_line_emits_a_set_focus_event`
Expected: FAIL — `no variant SetFocus`.

- [ ] **Step 3: Add the `SetFocus` event variant**

In `src/control_plane/listener.rs`, add to `enum ControlEvent` (after the `Emit { .. }` variant):

```rust
    /// An app-owned focus set/clear for the connection's surface.
    SetFocus {
        /// Connection id (ownership check in apply).
        connection_id: u64,
        /// The surface the connection's token resolved to.
        owner_surface: Entity,
        /// The handle to focus, or `None` to blur.
        handle: Option<String>,
        /// The mount instance id, or `None` for the default instance.
        instance: Option<String>,
    },
```

- [ ] **Step 4: Handle the `Focus` message in `handle_client_msg`**

In `handle_client_msg`, add an arm (after the `ClientMsg::Emit { .. }` arm):

```rust
        ClientMsg::Focus { handle, instance } => {
            let _ = events.send(ControlEvent::SetFocus {
                connection_id,
                owner_surface,
                handle,
                instance,
            });
        }
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p ozmux-gui client_focus_line_emits_a_set_focus_event`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/control_plane/listener.rs
git commit -m "feat(control-plane): emit SetFocus event from focus op"
```

---

## Task 10: Host apply — set `FocusedWebview`

**Files:**
- Modify: `src/control_plane.rs`

- [ ] **Step 1: Write the failing test**

Add a test to `src/control_plane.rs` (in its `#[cfg(test)]` area; if the file's existing tests live in `mod token_tests`, add a new `#[cfg(test)] mod focus_tests { ... }` at the end of the file). Use the inline-webview test harness pattern (`MultiplexerPlugin`, mount an inline webview, then drive a `SetFocus` event through `apply_control_events`):

```rust
#[cfg(test)]
mod focus_tests {
    use super::*;
    use crate::control_plane::listener::ControlEvent;
    use crate::inline_webview::InlineWebview;
    use bevy::ecs::system::RunSystemOnce;
    use bevy_cef::prelude::FocusedWebview;
    use crossbeam_channel::unbounded;

    #[test]
    fn set_focus_points_focused_webview_at_the_owned_inline_child() {
        let mut app = bevy::app::App::new();
        app.add_plugins(bevy::MinimalPlugins)
            .add_plugins(ozmux_multiplexer::MultiplexerPlugin)
            .init_resource::<DynamicRegistry>()
            .init_resource::<OzmuxRpc>()
            .init_resource::<FocusedWebview>()
            .init_resource::<DynAssetRegistryRes>();

        // Spawn a surface and an owned inline webview child of it.
        let surface = app
            .world_mut()
            .run_system_once(|mut mux: ozmux_multiplexer::MultiplexerCommands| {
                mux.create_workspace(Some("t".into())).surface
            })
            .unwrap();
        app.world_mut().flush();

        app.world_mut().resource_mut::<DynamicRegistry>().insert(
            "h1".into(),
            DynamicView {
                source: DynSource::Inline("<h1>x</h1>".into()),
                entry: "index.html".into(),
                interactive: true,
                owner_surface: surface,
                connection_id: 1,
            },
        );
        let child = app
            .world_mut()
            .spawn((
                ChildOf(surface),
                InlineWebview { view_id: "h1".into(), instance_id: None, slot: 0 },
            ))
            .id();

        // Drive a SetFocus through the events channel + apply system.
        let (tx, rx) = unbounded::<ControlEvent>();
        app.insert_resource(ControlEvents(rx));
        tx.send(ControlEvent::SetFocus {
            connection_id: 1,
            owner_surface: surface,
            handle: Some("h1".into()),
            instance: None,
        })
        .unwrap();
        app.world_mut().run_system_once(apply_control_events).unwrap();

        assert_eq!(
            app.world().resource::<FocusedWebview>().0,
            Some(child),
            "SetFocus must point FocusedWebview at the owned inline child"
        );

        // Blur clears it.
        tx.send(ControlEvent::SetFocus {
            connection_id: 1,
            owner_surface: surface,
            handle: None,
            instance: None,
        })
        .unwrap();
        app.world_mut().run_system_once(apply_control_events).unwrap();
        assert_eq!(app.world().resource::<FocusedWebview>().0, None);
    }

    #[test]
    fn set_focus_rejects_unowned_handle() {
        let mut app = bevy::app::App::new();
        app.add_plugins(bevy::MinimalPlugins)
            .add_plugins(ozmux_multiplexer::MultiplexerPlugin)
            .init_resource::<DynamicRegistry>()
            .init_resource::<OzmuxRpc>()
            .init_resource::<FocusedWebview>()
            .init_resource::<DynAssetRegistryRes>();
        let surface = app
            .world_mut()
            .run_system_once(|mut mux: ozmux_multiplexer::MultiplexerCommands| {
                mux.create_workspace(Some("t".into())).surface
            })
            .unwrap();
        app.world_mut().flush();
        app.world_mut().resource_mut::<DynamicRegistry>().insert(
            "h1".into(),
            DynamicView {
                source: DynSource::Inline("<h1>x</h1>".into()),
                entry: "index.html".into(),
                interactive: true,
                owner_surface: surface,
                connection_id: 99, // different owner
            },
        );
        let (tx, rx) = unbounded::<ControlEvent>();
        app.insert_resource(ControlEvents(rx));
        tx.send(ControlEvent::SetFocus {
            connection_id: 1,
            owner_surface: surface,
            handle: Some("h1".into()),
            instance: None,
        })
        .unwrap();
        app.world_mut().run_system_once(apply_control_events).unwrap();
        assert_eq!(app.world().resource::<FocusedWebview>().0, None);
    }
}
```

NOTE: confirm the exact resource name for the dyn-asset registry (`DynAssetRegistryRes`) and `OzmuxRpc` from the top of `control_plane.rs`; adjust `init_resource` lines to match the real type names if they differ.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ozmux-gui set_focus_points_focused_webview_at_the_owned_inline_child`
Expected: FAIL — `no variant SetFocus` handled / compile error on missing system params.

- [ ] **Step 3: Add imports and system params**

At the top of `src/control_plane.rs`, add to the existing import block (no blank lines between groups):

```rust
use crate::osc_webview::NonInteractive;
use bevy_cef::prelude::FocusedWebview;
```

Extend `apply_control_events`'s signature with the params needed to resolve and set focus (add to the existing param list; mutable params first per rules):

```rust
fn apply_control_events(
    mut commands: Commands,
    mut registry: ResMut<DynamicRegistry>,
    mut rpc: ResMut<OzmuxRpc>,
    mut focused: Option<ResMut<FocusedWebview>>,
    events: Option<Res<ControlEvents>>,
    dyn_assets: Res<DynAssetRegistryRes>,
    inline: Query<(Entity, &InlineWebview)>,
    child_of: Query<&ChildOf>,
    non_interactive: Query<(), With<NonInteractive>>,
) {
```

- [ ] **Step 4: Add the `SetFocus` match arm**

Inside the `match event` block in `apply_control_events`, add (after the `ControlEvent::Emit { .. }` arm):

```rust
            ControlEvent::SetFocus {
                connection_id,
                owner_surface,
                handle,
                instance,
            } => {
                let Some(focused) = focused.as_mut() else {
                    continue;
                };
                match handle {
                    Some(h) => {
                        let owned = registry
                            .get(&h)
                            .is_some_and(|v| v.connection_id == connection_id);
                        if !owned {
                            tracing::debug!(handle = %h, "focus op for unowned handle, dropping");
                            continue;
                        }
                        let target = inline.iter().find(|(entity, view)| {
                            view.view_id == h
                                && view.instance_id.as_deref() == instance.as_deref()
                                && child_of.get(*entity).map(ChildOf::parent) == Ok(owner_surface)
                                && !non_interactive.contains(*entity)
                        });
                        match target {
                            Some((entity, _)) => focused.0 = Some(entity),
                            None => tracing::debug!(
                                handle = %h,
                                "focus op for unmounted/non-interactive view, dropping"
                            ),
                        }
                    }
                    None => {
                        // Blur: only clear focus if it currently belongs to this
                        // connection's surface (don't clobber another pane's focus).
                        let owned_current = focused.0.is_some_and(|e| {
                            child_of.get(e).map(ChildOf::parent) == Ok(owner_surface)
                        });
                        if owned_current {
                            focused.0 = None;
                        }
                    }
                }
            }
```

NOTE: `ChildOf::parent` is a method returning the parent `Entity`; `child_of.get(e)` returns `Result<&ChildOf, _>`, so `.map(ChildOf::parent)` yields `Result<Entity, _>` comparable to `Ok(owner_surface)`. If the borrow checker rejects `.map(ChildOf::parent)` on a reference, use `.map(|c| c.parent())`.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p ozmux-gui set_focus_points_focused_webview_at_the_owned_inline_child set_focus_rejects_unowned_handle`
Expected: PASS (2 tests).

- [ ] **Step 6: Run the broader control-plane + inline suites for regressions**

Run: `cargo test -p ozmux-gui control_plane inline_webview`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add src/control_plane.rs
git commit -m "feat(control-plane): apply SetFocus to bevy_cef FocusedWebview with ownership guards"
```

---

## Task 11: Host glue — nav interception + focus/blur reporting in `ozmux_bridge.js`

**Files:**
- Modify: `src/extension_render/ozmux_bridge.js`
- Modify: `src/extension_render/preload.rs` (tests)

- [ ] **Step 1: Write the failing substring tests**

Add to `#[cfg(test)] mod tests` in `src/extension_render/preload.rs`:

```rust
    #[test]
    fn bridge_includes_focus_glue() {
        assert!(OZMUX_BRIDGE_JS.contains("__ozma.nav"), "nav forwarding present");
        assert!(OZMUX_BRIDGE_JS.contains("__ozma.focus"), "focus report present");
        assert!(OZMUX_BRIDGE_JS.contains("__ozma.keys"), "keymap receipt present");
        assert!(
            OZMUX_BRIDGE_JS.contains("location.hostname"),
            "glue must tag signals with its own handle (origin hostname)"
        );
        assert!(
            OZMUX_BRIDGE_JS.contains("altKey"),
            "default reserved chord is Alt-modified (IME-safe)"
        );
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ozmux-gui bridge_includes_focus_glue`
Expected: FAIL — substrings absent.

- [ ] **Step 3: Add the glue inside `ozmux_bridge.js`**

In `src/extension_render/ozmux_bridge.js`, insert this block immediately BEFORE the final `Object.defineProperty(window, 'ozmux', ...)` line (so `api` is defined and in scope):

```javascript
  // Focus glue: forward the reserved nav chord and DOM focus/blur to the
  // registering app over window.ozmux.call. The minted handle is this page's
  // origin hostname (ozmux-dyn://<handle>/), tagged on every signal so the app
  // routes it to the right widget. Default reserved chord: Alt+h/j/k/l.
  // NOTE: only Alt-modified keys are intercepted, so IME composition keys
  // (bare keys, keyCode 229) are never swallowed.
  var handle = location.hostname;
  var navMap = { h: 'left', j: 'down', k: 'up', l: 'right' };
  var keymap = { mods: ['alt'], keys: navMap };
  function matchNav(e) {
    if (!keymap.mods.every(function (m) {
      return m === 'alt' ? e.altKey : m === 'ctrl' ? e.ctrlKey : m === 'shift' ? e.shiftKey : m === 'meta' ? e.metaKey : false;
    })) return null;
    var k = (e.key || '').toLowerCase();
    return Object.prototype.hasOwnProperty.call(keymap.keys, k) ? keymap.keys[k] : null;
  }
  window.addEventListener('keydown', function (e) {
    var dir = matchNav(e);
    if (dir) {
      e.preventDefault();
      e.stopPropagation();
      api.call('__ozma.nav', [handle, dir]);
    }
  }, true);
  window.addEventListener('focus', function () { api.call('__ozma.focus', [handle, true]); });
  window.addEventListener('blur', function () { api.call('__ozma.focus', [handle, false]); });
  api.on('__ozma.keys', function (set) {
    if (set && set.keys) keymap = { mods: set.mods || ['alt'], keys: set.keys };
  });
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ozmux-gui bridge_includes_focus_glue`
Expected: PASS. Also run `cargo test -p ozmux-gui preload` to confirm `dynamic_preload_injects_context_and_ozmux_bridge` still passes.

- [ ] **Step 5: Lint the JS**

Run: `pnpm lint` (biome scans `sdk/**`; if it does not scan `src/extension_render`, run `pnpm exec biome check src/extension_render/ozmux_bridge.js` to format-check the edited file). Fix any reported issues with `pnpm exec biome check --write src/extension_render/ozmux_bridge.js`.

- [ ] **Step 6: Commit**

```bash
git add src/extension_render/ozmux_bridge.js src/extension_render/preload.rs
git commit -m "feat(extension-render): focus glue (nav chord + focus/blur report) in ozmux bridge"
```

---

## Task 12: Example — demonstrate focus

**Files:**
- Modify: `sdk/ratatui-ozma/examples/ratatui_webview.rs`

- [ ] **Step 1: Update the example to use FocusManager**

Rewrite `sdk/ratatui-ozma/examples/ratatui_webview.rs`'s `main`/`run` to add a native widget (a status line acting as a focusable panel) alongside the webview, register the webview via `focusable`, and drive focus. Replace the `register` call and `run` loop. Full replacement of the relevant parts:

In `main`, change the registration to instrument the view:

```rust
    let mut focus = ratatui_ozma::FocusManager::new();
    let view = ozma.register(ratatui_ozma::focusable(
        Webview::inline(html).on("ping", |(arg,): (String,)| {
            Ok::<_, RpcError>(format!("pong:{arg}"))
        }),
        focus.signal_sender(),
    ))?;
```

Pass `focus` into `run` (add a `focus: &mut ratatui_ozma::FocusManager` parameter) and register ring items once before the loop:

```rust
    focus.add_webview_at("web", view.clone(), ratatui::layout::Rect::default());
    focus.add_native_at("status", ratatui::layout::Rect::default());
```

Inside the loop, before/after `terminal.draw`, drive focus:

```rust
        // Apply any glue-driven focus moves from the page.
        for sync in focus.drain() {
            sync.apply(ozma)?;
        }
```

In the draw closure, set the webview rect each frame and render the focus state:

```rust
        terminal.draw(|f| {
            let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(f.area());
            f.render_widget(Paragraph::new("Alt+h/l to move focus, q to quit"), rows[0]);
            let cols = Layout::horizontal([Constraint::Percentage(60), Constraint::Min(0)]).split(rows[1]);
            focus.set_rect("web", cols[0]);
            focus.set_rect("status", cols[1]);
            f.render_stateful_widget(
                WebviewWidget::new(view.id())
                    .focused(focus.is_focused("web"))
                    .fallback(Block::bordered().title("loading…")),
                cols[0],
                ozma.frame(),
            );
            let style = if focus.is_focused("status") {
                ratatui::style::Style::default().fg(ratatui::style::Color::Yellow)
            } else {
                ratatui::style::Style::default()
            };
            f.render_widget(Paragraph::new("status panel").style(style), cols[1]);
        })?;
```

In the key handler, route nav keys when a native widget is focused; quit on `q`:

```rust
        if event::poll(Duration::from_millis(50))?
            && let Event::Key(k) = event::read()?
        {
            if k.code == KeyCode::Char('q') {
                return Ok(());
            }
            if focus.focused_is_native()
                && let Some(dir) = ratatui_ozma::FocusManager::nav_key(&k)
            {
                focus.navigate(dir).apply(ozma)?;
            }
        }
```

Add the needed imports at the top (`KeyEvent` not required; `KeyCode` already imported). Ensure `WebviewHandle` derives/`Clone` is available — `WebviewHandle` already `#[derive(Clone)]` (see `webview.rs`).

- [ ] **Step 2: Build the example**

Run: `cargo build -p ratatui-ozma --example ratatui_webview`
Expected: compiles cleanly.

- [ ] **Step 3: Commit**

```bash
git add sdk/ratatui-ozma/examples/ratatui_webview.rs
git commit -m "docs(ratatui-ozma): demonstrate FocusManager in the example"
```

---

## Task 13: Full-suite verification + lint

**Files:** none (verification only)

- [ ] **Step 1: Run the whole workspace test suite**

Run: `cargo test`
Expected: PASS (all crates, including the new SDK and host tests).

- [ ] **Step 2: Lint and format**

Run: `cargo clippy --workspace --fix --allow-dirty --allow-staged && cargo fmt`
Then re-run: `cargo test -p ratatui-ozma && cargo test -p ozmux-gui`
Expected: PASS; no clippy warnings introduced.

- [ ] **Step 3: TypeScript lint (glue)**

Run: `pnpm lint` (and `pnpm exec biome check src/extension_render/ozmux_bridge.js` if needed).
Expected: clean.

- [ ] **Step 4: Commit any lint fixups**

```bash
git add -A
git commit -m "chore(ratatui-ozma): clippy/fmt/biome fixups for focus feature"
```

---

## Notes for the implementer

- **Unverified runtime path (spec §4 note):** the page `window` `focus`/`blur` DOM events may not fire reliably in CEF OSR. The plan wires them, but the load-bearing reconciliation is `FocusManager::drain` applying `Signal::Nav`; the click/blur `Signal::Focus` path is best-effort. Do not block the feature on OSR blur-event reliability — that needs a manual integration check in the running app (`cargo run`, mount a webview, click it, Alt+h to move focus out).
- **Reserved namespace:** never register `__ozma.*` via `Webview::on` (it panics); the SDK uses `on_reserved` internally via `focusable`.
- **Nav-chord vs ozmux globals:** `Alt+h/j/k/l` is free against ozmux defaults (`Cmd+H/J/K/L` for pane focus). If a user binds `Alt+hjkl` as an ozmux shortcut, `bindings.lookup` wins and the glue never sees it — documented limitation.
- **Type names to confirm before Task 10:** `DynAssetRegistryRes`, `OzmuxRpc`, `ControlEvents`, `DynamicView`, `DynSource` — all in `src/control_plane.rs`. Adjust the test `init_resource` lines if the real names differ.
