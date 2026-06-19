# ozma_terminal Mouse Input Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the self-contained `ozma_terminal` crate working mouse support — app reporting (SGR/X10), text selection + copy, wheel scrollback, and Cmd-click hyperlinks — by adding Bevy glue over the engine's existing pure routers.

**Architecture:** The engine (`ozma_tty_engine`) already owns the per-event decisions (`ButtonAction::route`, `WheelAction::route`, SGR/X10 encoder). The crate adds Bevy systems that read input, hit-test the cursor to a cell, track click-count + drag state, and translate router output into a small private `MouseEffect` list that a thin apply step writes to the `TerminalHandle` / `Clipboard`. The decision logic is pure (no `TerminalHandle`, which has no public constructor) so it is unit-testable without a PTY; protocol-byte correctness is already covered by engine tests.

**Tech Stack:** Rust 2024, Bevy 0.18, `alacritty_terminal` (via the engine), `arboard` (clipboard, existing), `open` (URL launch, new dep).

**Spec:** `docs/superpowers/specs/2026-06-19-ozma-terminal-mouse-input-design.md`

## Global Constraints

- Rust edition 2024, toolchain 1.95. Every `pub` item gets a `///` doc comment; every module file gets a `//!` header.
- No `mod.rs` (use `foo.rs` + `foo/bar.rs`). Comments restricted to `// TODO:` / `// NOTE:` / `// SAFETY:`; all comments in English.
- Imports: single contiguous `use` block at the top; no inline fully-qualified paths in signatures/bodies.
- Bevy systems: mutable `SystemParam`s declared before immutable ones; gate whole-system change checks with `run_if`, not in-body early returns; `Plugin::build` is one method chain.
- `Query` params use descriptive nouns, never a `_q` suffix.
- The `ozma_terminal` crate MUST NOT depend on `ozmux_configs`, `ozmux_tmux`, or `bevy_cef`. (`FineModifier` is therefore a crate-local enum, not the `ozmux_configs` one.)
- Selection copies to the system clipboard **on Left release** (matching the repo's tmux VT path).
- `ButtonConfig` derives `Default` with `max_protocol_events_per_frame = 0`, which makes `route()` drop every forwarded button event — `OzmaMouseConfig::default` MUST set the buttons cap to `8` explicitly.
- Wheel sign: Bevy `MouseWheel.y > 0` is wheel-up; `WheelAction::route` treats **negative notches as up/older**, so the accumulator output is negated before calling `route`.
- Cell coordinates: hit-test produces **1-indexed** `CellCoord` (what SGR/X10 want); `to_viewport_point` subtracts 1 for the engine's viewport-relative selection `Point`.

---

## File Structure

| File | Responsibility |
| --- | --- |
| `crates/ozma_tty_renderer/src/schema/hyperlink.rs` (modify) | Add the shared URL-scheme allowlist (`is_allowed` pub, `scheme_of` private). |
| `src/input/hyperlink.rs` (modify) | Delete its `is_allowed`/`scheme_of`; delegate to `schema::is_allowed`. |
| `crates/ozma_terminal/Cargo.toml` (modify) | Add `open = { workspace = true }`. |
| `crates/ozma_terminal/src/mouse.rs` (create) | `OzmaMousePlugin`, `OzmaMouseConfig`, `FineModifier`, `OzmaTerminalMouseSet`, `OzmaMouseGesture`, `ClickTracker`, hit-test + `to_viewport_point` helpers, `MouseEffect`, `decide_button`, `decide_wheel`, the two dispatch systems, `apply_effect`. |
| `crates/ozma_terminal/src/hyperlink.rs` (create) | `link_modifier_held`, `try_open_uri`, `hyperlink_hover_cursor` system, `cursor_decision`. |
| `crates/ozma_terminal/src/lib.rs` (modify) | `mod mouse; mod hyperlink;`; add `OzmaMousePlugin`; re-export `OzmaMouseConfig`, `FineModifier`, `OzmaTerminalMouseSet`. |
| `crates/ozma_terminal/src/input.rs` (modify) | Promote `current_terminal_modifiers` to `pub(crate)` (reused by mouse/hyperlink). |
| `src/input/shortcuts.rs` (modify) | Add `populate_mouse_config` Startup system mapping `OzmuxConfigsResource.mouse` → `OzmaMouseConfig`. |
| `src/input.rs` (modify) | Register `populate_mouse_config`. |
| `src/ozma_input.rs` (modify) | `maintain_input_disabled` also runs `.before(OzmaTerminalMouseSet)`. |

> Start with a flat `mouse.rs`. Split into `mouse/buttons.rs` + `mouse/wheel.rs` only if it grows past ~350 lines.

---

## Task 1: Relocate the URL-scheme allowlist into the renderer schema

**Files:**
- Modify: `crates/ozma_tty_renderer/src/schema/hyperlink.rs`
- Modify: `src/input/hyperlink.rs:79-102` (delete copies), `:48-56` (delegate)

**Interfaces:**
- Produces: `ozma_tty_renderer::schema::is_allowed(uri: &str) -> bool` (the `http`/`https`/`mailto`/`ftp` allowlist, case-insensitive, rejecting `javascript:`/`file:`/`data:`).

- [ ] **Step 1: Add the failing test in the schema crate**

Append to `crates/ozma_tty_renderer/src/schema/hyperlink.rs`'s `#[cfg(test)] mod tests` (create the module if absent):

```rust
#[test]
fn is_allowed_accepts_canonical_schemes_case_insensitive() {
    assert!(is_allowed("http://example.com"));
    assert!(is_allowed("HTTPS://example.com"));
    assert!(is_allowed("Mailto:foo@example"));
    assert!(is_allowed("ftp://example.com"));
}

#[test]
fn is_allowed_rejects_dangerous_or_unknown_schemes() {
    assert!(!is_allowed("javascript:alert(1)"));
    assert!(!is_allowed("file:///etc/passwd"));
    assert!(!is_allowed("data:text/html,<script>"));
    assert!(!is_allowed(""));
    assert!(!is_allowed("no-colon-here"));
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p ozma_tty_renderer is_allowed`
Expected: FAIL — `cannot find function is_allowed`.

- [ ] **Step 3: Implement the allowlist in the schema crate**

Add to `crates/ozma_tty_renderer/src/schema/hyperlink.rs` (above the test module):

```rust
const ALLOWED_SCHEMES: &[&str] = &["http", "https", "mailto", "ftp"];

/// Returns `true` when `uri` carries a scheme on the v1 allowlist
/// (`http`, `https`, `mailto`, `ftp`), case-insensitive.
pub fn is_allowed(uri: &str) -> bool {
    scheme_of(uri)
        .map(|s| s.to_ascii_lowercase())
        .is_some_and(|s| ALLOWED_SCHEMES.contains(&s.as_str()))
}

/// Parses an RFC 3986 scheme: first byte ALPHA, continuation
/// ALPHA / DIGIT / `+` / `-` / `.`. Returns `None` for malformed input.
fn scheme_of(uri: &str) -> Option<&str> {
    let (scheme, _) = uri.split_once(':')?;
    let mut bytes = scheme.bytes();
    let first = bytes.next()?;
    if !first.is_ascii_alphabetic() {
        return None;
    }
    if !bytes.all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'-' || b == b'.') {
        return None;
    }
    Some(scheme)
}
```

If `schema/hyperlink.rs` has no `//!` header, it already declares a module (it's re-exported via `pub use hyperlink::*` in `schema.rs`), so a header exists; otherwise add `//! OSC 8 hyperlink schema types and the URL-scheme allowlist.`

- [ ] **Step 4: Run the schema tests**

Run: `cargo test -p ozma_tty_renderer is_allowed`
Expected: PASS.

- [ ] **Step 5: Delete the copies in `src/input/hyperlink.rs` and delegate**

In `src/input/hyperlink.rs`: delete `const ALLOWED_SCHEMES`, `fn scheme_of`, and `fn is_allowed` (lines ~79-102) and their `scheme_of`/`is_allowed` tests. Change `try_open_uri` to call the schema version:

```rust
if !ozma_tty_renderer::schema::is_allowed(uri) {
    debug!("hyperlink: dropping disallowed uri {}", uri);
    return;
}
```

Add `use ozma_tty_renderer::schema::is_allowed;` to the top `use` block and write `is_allowed(uri)` (not the inline path). Keep the `should_open_at` function in `src/input/hyperlink.rs` unchanged (it stays per-host).

- [ ] **Step 6: Build + test the workspace**

Run: `cargo test -p ozma_tty_renderer && cargo build && cargo test -p ozmux-gui hyperlink`
Expected: PASS; no references to a removed `is_allowed` in `src/`.

- [ ] **Step 7: Commit**

```bash
git add crates/ozma_tty_renderer/src/schema/hyperlink.rs src/input/hyperlink.rs
git commit -m "refactor(renderer): share URL-scheme allowlist via schema; src delegates

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Crate foundation — config, types, helpers, plugin skeleton

**Files:**
- Modify: `crates/ozma_terminal/Cargo.toml`
- Create: `crates/ozma_terminal/src/mouse.rs`
- Modify: `crates/ozma_terminal/src/lib.rs`

**Interfaces:**
- Produces: `OzmaMouseConfig` (Resource), `FineModifier` (enum), `OzmaTerminalMouseSet` (SystemSet), `OzmaMousePlugin` (Plugin), `OzmaMouseGesture`/`DragGesture`/`DragPhase`, `ClickTracker`, `cell_at_local(local: Vec2, cell_w: f32, cell_h: f32, cols: u16, rows: u16) -> (CellCoord, Side)`, `cell_at_cursor(...) -> Option<(CellCoord, Side)>`, `to_viewport_point(cell: CellCoord) -> Point`, `protocol_mods(keys: &ButtonInput<KeyCode>) -> ProtocolModifiers`.

- [ ] **Step 1: Add the `open` dependency**

In `crates/ozma_terminal/Cargo.toml` `[dependencies]`, add (alphabetical order is not enforced here; place near the others):

```toml
open = { workspace = true }
```

- [ ] **Step 2: Write the failing foundation tests**

Create `crates/ozma_terminal/src/mouse.rs` with only the `use` block + a `#[cfg(test)] mod tests` containing:

```rust
#[test]
fn default_config_sets_button_cap_explicitly() {
    let cfg = OzmaMouseConfig::default();
    assert_eq!(cfg.buttons.max_protocol_events_per_frame, 8, "must NOT be ButtonConfig::default()'s 0");
    assert_eq!(cfg.wheel.max_protocol_events_per_frame, 8);
    assert_eq!(cfg.cells_per_notch, 0.5);
    assert_eq!(cfg.double_click_timeout, std::time::Duration::from_millis(400));
    assert_eq!(cfg.click_drift_px, 8.0);
    assert_eq!(cfg.fine_modifier, FineModifier::Alt);
}

#[test]
fn cell_at_local_is_one_indexed_and_clamped() {
    let (cell, side) = cell_at_local(Vec2::new(0.0, 0.0), 10.0, 20.0, 80, 24);
    assert_eq!((cell.col, cell.row), (1, 1));
    assert_eq!(side, Side::Left);
    let (cell, _) = cell_at_local(Vec2::new(10_000.0, 10_000.0), 10.0, 20.0, 80, 24);
    assert_eq!((cell.col, cell.row), (80, 24));
    let (cell, side) = cell_at_local(Vec2::new(17.0, 5.0), 10.0, 20.0, 80, 24);
    assert_eq!(cell.col, 2);
    assert_eq!(side, Side::Right);
}

#[test]
fn to_viewport_point_zero_indexes_the_one_indexed_cell() {
    let p = to_viewport_point(CellCoord { col: 5, row: 3 });
    assert_eq!(p.line.0, 2);
    assert_eq!(p.column.0, 4);
}

#[test]
fn click_tracker_counts_within_timeout_and_drift() {
    let mut t = ClickTracker::default();
    let cfg = (std::time::Duration::from_millis(400), 8.0f32);
    assert_eq!(t.register(std::time::Duration::from_millis(0), Vec2::new(10.0, 10.0), cfg), 1);
    assert_eq!(t.register(std::time::Duration::from_millis(200), Vec2::new(11.0, 11.0), cfg), 2);
    assert_eq!(t.register(std::time::Duration::from_millis(350), Vec2::new(12.0, 10.0), cfg), 3);
    assert_eq!(t.register(std::time::Duration::from_millis(900), Vec2::new(12.0, 10.0), cfg), 1);
}
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p ozma_terminal --lib mouse::`
Expected: FAIL — types/functions not found.

- [ ] **Step 4: Implement the foundation**

Write the rest of `crates/ozma_terminal/src/mouse.rs`:

```rust
//! Default mouse handler for the Ozma terminal: app reporting, local text
//! selection + copy, wheel scrollback, and Cmd-click hyperlink open. Reads Bevy
//! mouse input, hit-tests the cursor to a cell, and drives the engine's pure
//! `ButtonAction` / `WheelAction` routers, applying the result to the
//! `TerminalHandle` / `Clipboard`. Gated per entity by `InputDisabled`.

use crate::clipboard::Clipboard;
use crate::hyperlink::{link_modifier_held, try_open_uri};
use crate::input::{InputDisabled, current_terminal_modifiers};
use crate::spawn::OzmaTerminal;
use bevy::input::ButtonState;
use bevy::input::mouse::{MouseButton, MouseButtonInput, MouseScrollUnit, MouseWheel};
use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::window::PrimaryWindow;
use ozma_tty_engine::{
    ButtonAction, ButtonConfig, ButtonEvent, ButtonEventKind, CellCoord, Coalescer, Column, Line,
    MouseButtonKind, Point, ProtocolModifiers, PtyHandle, SelectionType, Side, TermMode,
    TerminalHandle, WheelAction, WheelConfig, WheelModifiers,
};
use ozma_tty_renderer::TerminalCellMetricsResource;
use ozma_tty_renderer::schema::TerminalGrid;
use std::time::Duration;

/// Which modifier activates "fine" (1 line per notch) wheel scrolling.
/// Crate-local mirror of the host config enum (the crate must not depend on
/// `ozmux_configs`). Default `Alt`: on macOS Shift+wheel becomes horizontal
/// scroll at the OS level, so Shift never reaches the app as vertical `y`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FineModifier {
    Shift,
    Ctrl,
    #[default]
    Alt,
    None,
}

/// Host-supplied mouse policy. `Default` is a working spawn-and-go config; the
/// host overrides it from `ozmux_configs`.
#[derive(Resource)]
pub struct OzmaMouseConfig {
    /// Button-report burst cap. MUST be non-zero or forwarded clicks are dropped.
    pub buttons: ButtonConfig,
    /// Wheel routing config (lines-per-notch, fine lines, burst cap).
    pub wheel: WheelConfig,
    /// Cells of wheel travel per emitted notch (smooth-scroll accumulation).
    pub cells_per_notch: f32,
    /// Max gap between clicks counted as a double / triple click.
    pub double_click_timeout: Duration,
    /// Max cursor drift (logical px) between clicks of one chord.
    pub click_drift_px: f32,
    /// Which modifier activates fine scrolling.
    pub fine_modifier: FineModifier,
}

impl Default for OzmaMouseConfig {
    fn default() -> Self {
        Self {
            buttons: ButtonConfig {
                max_protocol_events_per_frame: 8,
            },
            wheel: WheelConfig::default(),
            cells_per_notch: 0.5,
            double_click_timeout: Duration::from_millis(400),
            click_drift_px: 8.0,
            fine_modifier: FineModifier::Alt,
        }
    }
}

/// System set for the crate's three mouse systems. Hosts maintaining
/// `InputDisabled` should schedule their maintainer `.before(OzmaTerminalMouseSet)`.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct OzmaTerminalMouseSet;

/// Phase of an in-progress left-drag: `Armed` after a single-click press (no
/// selection started yet), `Started` once the pointer crossed into another cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DragPhase {
    Armed,
    Started,
}

/// An in-progress button gesture: the held button, the selection anchor, and the
/// last cell a drag reached (dedup + lazy materialization).
pub(crate) struct DragGesture {
    pub(crate) button: MouseButtonKind,
    pub(crate) origin: CellCoord,
    pub(crate) side: Side,
    pub(crate) ty: SelectionType,
    pub(crate) phase: DragPhase,
    pub(crate) last_cell: CellCoord,
}

/// Tracks the current mouse gesture and consecutive-click count.
#[derive(Resource, Default)]
pub(crate) struct OzmaMouseGesture {
    pub(crate) click: ClickTracker,
    pub(crate) drag: Option<DragGesture>,
}

/// Consecutive-click counter using a timeout + positional-drift gate.
#[derive(Default)]
pub(crate) struct ClickTracker {
    last: Option<(Duration, Vec2, u8)>,
}

impl ClickTracker {
    /// Registers a press at `now` / logical `pos`, returning the click count
    /// (1..=3). `cfg` is `(timeout, drift_px)`.
    pub(crate) fn register(&mut self, now: Duration, pos: Vec2, cfg: (Duration, f32)) -> u8 {
        let (timeout, drift) = cfg;
        let count = match self.last {
            Some((t, p, c)) if now.saturating_sub(t) <= timeout && p.distance(pos) <= drift => {
                (c + 1).min(3)
            }
            _ => 1,
        };
        self.last = Some((now, pos, count));
        count
    }
}

/// 1-indexed `(CellCoord, Side)` of the cell at pane-local physical `local`,
/// clamped to `1..=cols` × `1..=rows`. `Side` is `Left` in the left half.
pub(crate) fn cell_at_local(
    local: Vec2,
    cell_w: f32,
    cell_h: f32,
    cols: u16,
    rows: u16,
) -> (CellCoord, Side) {
    let col_f = (local.x / cell_w).max(0.0);
    let row_f = (local.y / cell_h).max(0.0);
    let col = (col_f.floor() as u32 + 1).min(cols as u32).max(1);
    let row = (row_f.floor() as u32 + 1).min(rows as u32).max(1);
    let side = if col_f - col_f.floor() < 0.5 {
        Side::Left
    } else {
        Side::Right
    };
    (CellCoord { col, row }, side)
}

/// Resolves the window-space physical cursor to a cell on the terminal node, or
/// `None` when the cursor is outside the node.
pub(crate) fn cell_at_cursor(
    node: &ComputedNode,
    transform: &UiGlobalTransform,
    cursor_phys: Vec2,
    cell_w: f32,
    cell_h: f32,
    cols: u16,
    rows: u16,
) -> Option<(CellCoord, Side)> {
    let local = node
        .normalize_point(*transform, cursor_phys)
        .map(|n| (n + Vec2::splat(0.5)) * node.size)?;
    Some(cell_at_local(local, cell_w, cell_h, cols, rows))
}

/// Converts a 1-indexed protocol `CellCoord` into the engine's viewport-relative
/// selection `Point` (row 0 = top of viewport; the engine translates for scroll).
pub(crate) fn to_viewport_point(cell: CellCoord) -> Point {
    Point::new(Line(cell.row as i32 - 1), Column(cell.col as usize - 1))
}

/// Builds `ProtocolModifiers` from the held keys.
pub(crate) fn protocol_mods(keys: &ButtonInput<KeyCode>) -> ProtocolModifiers {
    let m = current_terminal_modifiers(keys);
    ProtocolModifiers {
        shift: m.shift,
        ctrl: m.ctrl,
        alt: m.alt,
        meta: m.meta,
    }
}

/// Registers the crate's mouse systems and resources.
pub(crate) struct OzmaMousePlugin;

impl Plugin for OzmaMousePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<OzmaMouseConfig>()
            .init_resource::<OzmaMouseGesture>()
            .add_message::<MouseButtonInput>()
            .add_message::<MouseWheel>()
            .add_message::<CursorMoved>();
    }
}
```

> The `add_message::<...>()` calls are REQUIRED: later tasks register systems gated on `run_if(on_message::<MouseButtonInput>)` / `MouseWheel` / `CursorMoved`, and under `MinimalPlugins` (the crate's lib test path) those `Messages<T>` resources do not otherwise exist, so the run conditions would panic. This mirrors `OzmaInputPlugin`'s `add_message::<KeyboardInput>()`. They use the `MouseButtonInput` / `MouseWheel` / `CursorMoved` imports, so add `use bevy::input::mouse::{MouseButtonInput, MouseWheel};` and `use bevy::window::CursorMoved;` to the `use` block in this task (the other input imports are added in Tasks 5–6 as their code lands — keep this task's `use` block to names actually referenced here so the build stays warning-free).

Note `OzmaMouseConfig` cannot derive `Default` via `#[derive]` because `Duration` is fine but the explicit cap matters — the hand-written `impl Default` above is required. Leave the unused-import warnings for items consumed in later tasks by prefixing the `use` of not-yet-used names is not allowed; instead, only import what Step 4's code references and add the rest in Tasks 3–6. (Practically: trim the `use` block to the names actually used here — `ButtonConfig`, `WheelConfig`, `CellCoord`, `Column`, `Line`, `Point`, `Side`, `ProtocolModifiers`, `KeyCode`, `ButtonInput`, `Vec2`, `Duration`, `ComputedNode`, `UiGlobalTransform`, the plugin/`App`/`Resource`/`SystemSet` prelude items, and `current_terminal_modifiers` — and grow it per task.)

- [ ] **Step 5: Wire the module into the crate**

In `crates/ozma_terminal/src/lib.rs`: add `mod mouse;` (and `mod hyperlink;` will be added in Task 6 — add it now as an empty module to keep `mouse.rs`'s `use crate::hyperlink::...` resolvable, OR defer the `use crate::hyperlink` line until Task 6). Simplest: in this task, comment out (omit) the `use crate::hyperlink::{...}` import and the `Clipboard` import until they're used in Tasks 5–6. Add to `lib.rs`:

```rust
mod mouse;
```

and in the re-export list:

```rust
pub use mouse::{FineModifier, OzmaMouseConfig, OzmaTerminalMouseSet};
```

and add `OzmaMousePlugin` to the `OzmaTerminalPlugin::build` plugin tuple:

```rust
.add_plugins((ExitPlugin, LayoutPlugin, OzmaActionPlugin, OzmaInputPlugin, OzmaMousePlugin))
```

(add `use crate::mouse::OzmaMousePlugin;` to the `use` block).

- [ ] **Step 6: Run the foundation tests**

Run: `cargo test -p ozma_terminal --lib mouse::`
Expected: PASS (4 tests).

- [ ] **Step 7: Commit**

```bash
git add crates/ozma_terminal/Cargo.toml crates/ozma_terminal/src/mouse.rs crates/ozma_terminal/src/lib.rs
git commit -m "feat(ozma_terminal): mouse config, gesture state, hit-test helpers

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: `MouseEffect` + `decide_button` (pure)

**Files:**
- Modify: `crates/ozma_terminal/src/mouse.rs`

**Interfaces:**
- Consumes: `OzmaMouseGesture`, `cell_at_local`/`to_viewport_point`, `OzmaMouseConfig`, engine `ButtonAction::route`.
- Produces: `enum MouseEffect`; `fn decide_button(gesture: &mut OzmaMouseGesture, modes: TermMode, evt: ButtonEvent, mods: ProtocolModifiers, modifier_held: bool, link_at_cell: Option<String>, cfg: &ButtonConfig) -> Vec<MouseEffect>`.

- [ ] **Step 1: Write the failing tests**

Add to `mouse.rs` `mod tests`:

```rust
use ozma_tty_engine::{ButtonEvent, ButtonEventKind, MouseButtonKind};

fn ev(kind: ButtonEventKind, col: u32, row: u32, count: u8) -> ButtonEvent {
    ButtonEvent {
        kind,
        button: MouseButtonKind::Left,
        cell: CellCoord { col, row },
        side: Side::Left,
        click_count: count,
    }
}

#[test]
fn local_single_press_arms_drag_and_clears() {
    let mut g = OzmaMouseGesture::default();
    let fx = decide_button(&mut g, TermMode::empty(), ev(ButtonEventKind::Press, 5, 5, 1),
        ProtocolModifiers::default(), false, None, &ButtonConfig { max_protocol_events_per_frame: 8 });
    assert_eq!(fx, vec![MouseEffect::SelClear]);
    assert!(matches!(g.drag, Some(DragGesture { phase: DragPhase::Armed, .. })));
}

#[test]
fn local_drag_materializes_then_extends() {
    let mut g = OzmaMouseGesture::default();
    let cfg = ButtonConfig { max_protocol_events_per_frame: 8 };
    decide_button(&mut g, TermMode::empty(), ev(ButtonEventKind::Press, 5, 5, 1), ProtocolModifiers::default(), false, None, &cfg);
    let fx = decide_button(&mut g, TermMode::empty(), ev(ButtonEventKind::Drag, 7, 5, 1), ProtocolModifiers::default(), false, None, &cfg);
    assert_eq!(fx, vec![
        MouseEffect::SelStart { point: to_viewport_point(CellCoord { col: 5, row: 5 }), side: Side::Left, ty: SelectionType::Simple },
        MouseEffect::SelUpdate { point: to_viewport_point(CellCoord { col: 7, row: 5 }), side: Side::Left },
    ]);
    let fx2 = decide_button(&mut g, TermMode::empty(), ev(ButtonEventKind::Drag, 9, 5, 1), ProtocolModifiers::default(), false, None, &cfg);
    assert_eq!(fx2, vec![MouseEffect::SelUpdate { point: to_viewport_point(CellCoord { col: 9, row: 5 }), side: Side::Left }]);
}

#[test]
fn release_after_drag_copies() {
    let mut g = OzmaMouseGesture::default();
    let cfg = ButtonConfig { max_protocol_events_per_frame: 8 };
    decide_button(&mut g, TermMode::empty(), ev(ButtonEventKind::Press, 5, 5, 1), ProtocolModifiers::default(), false, None, &cfg);
    decide_button(&mut g, TermMode::empty(), ev(ButtonEventKind::Drag, 7, 5, 1), ProtocolModifiers::default(), false, None, &cfg);
    let fx = decide_button(&mut g, TermMode::empty(), ev(ButtonEventKind::Release, 7, 5, 1), ProtocolModifiers::default(), false, None, &cfg);
    assert_eq!(fx, vec![MouseEffect::Copy]);
    assert!(g.drag.is_none());
}

#[test]
fn release_after_bare_click_does_not_copy() {
    let mut g = OzmaMouseGesture::default();
    let cfg = ButtonConfig { max_protocol_events_per_frame: 8 };
    decide_button(&mut g, TermMode::empty(), ev(ButtonEventKind::Press, 5, 5, 1), ProtocolModifiers::default(), false, None, &cfg);
    let fx = decide_button(&mut g, TermMode::empty(), ev(ButtonEventKind::Release, 5, 5, 1), ProtocolModifiers::default(), false, None, &cfg);
    assert_eq!(fx, vec![]);
    assert!(g.drag.is_none());
}

#[test]
fn double_click_starts_word_selection() {
    let mut g = OzmaMouseGesture::default();
    let cfg = ButtonConfig { max_protocol_events_per_frame: 8 };
    let fx = decide_button(&mut g, TermMode::empty(), ev(ButtonEventKind::Press, 5, 5, 2), ProtocolModifiers::default(), false, None, &cfg);
    assert_eq!(fx, vec![MouseEffect::SelStart { point: to_viewport_point(CellCoord { col: 5, row: 5 }), side: Side::Left, ty: SelectionType::Semantic }]);
}

#[test]
fn app_capture_press_forwards_sgr_bytes() {
    let mut g = OzmaMouseGesture::default();
    let modes = TermMode::MOUSE_DRAG | TermMode::SGR_MOUSE;
    let fx = decide_button(&mut g, modes, ev(ButtonEventKind::Press, 5, 5, 1), ProtocolModifiers::default(), false, None, &ButtonConfig { max_protocol_events_per_frame: 8 });
    assert_eq!(fx, vec![MouseEffect::SelClear, MouseEffect::Write(b"\x1b[<0;5;5M".to_vec())]);
}

#[test]
fn shift_bypass_selects_even_when_captured() {
    let mut g = OzmaMouseGesture::default();
    let modes = TermMode::MOUSE_DRAG | TermMode::SGR_MOUSE;
    let mods = ProtocolModifiers { shift: true, ..Default::default() };
    let fx = decide_button(&mut g, modes, ev(ButtonEventKind::Press, 5, 5, 1), mods, false, None, &ButtonConfig { max_protocol_events_per_frame: 8 });
    assert_eq!(fx, vec![MouseEffect::SelClear]);
    assert!(matches!(g.drag, Some(DragGesture { phase: DragPhase::Armed, .. })));
}

#[test]
fn cmd_click_on_link_opens_and_consumes() {
    let mut g = OzmaMouseGesture::default();
    let fx = decide_button(&mut g, TermMode::empty(), ev(ButtonEventKind::Press, 5, 5, 1),
        ProtocolModifiers { meta: true, ..Default::default() }, true, Some("https://example.com".into()),
        &ButtonConfig { max_protocol_events_per_frame: 8 });
    assert_eq!(fx, vec![MouseEffect::OpenUri("https://example.com".into())]);
    assert!(g.drag.is_none(), "a link-open press must not arm a drag");
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p ozma_terminal --lib mouse::`
Expected: FAIL — `MouseEffect` / `decide_button` not found.

- [ ] **Step 3: Implement `MouseEffect` + `decide_button`**

Add to `mouse.rs` (and extend the `use` block with `ButtonAction`, `ButtonEvent`, `ButtonEventKind`, `MouseButtonKind`, `SelectionType`):

```rust
/// A resolved intent the apply step writes to the handle / clipboard. Kept
/// separate from application so the decision logic is unit-testable without a
/// `TerminalHandle` (which has no public constructor).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum MouseEffect {
    Write(Vec<u8>),
    SelStart { point: Point, side: Side, ty: SelectionType },
    SelUpdate { point: Point, side: Side },
    SelClear,
    Copy,
    Scroll(i32),
    OpenUri(String),
}

/// Pure per-event decision for a mouse button. Mutates `gesture` (drag phase /
/// click state) and returns the effects to apply. A Cmd/Ctrl-click on a linked
/// cell opens the URL and consumes the event; otherwise the engine's
/// `ButtonAction::route` decides forward-to-app vs local selection.
pub(crate) fn decide_button(
    gesture: &mut OzmaMouseGesture,
    modes: TermMode,
    evt: ButtonEvent,
    mods: ProtocolModifiers,
    modifier_held: bool,
    link_at_cell: Option<String>,
    cfg: &ButtonConfig,
) -> Vec<MouseEffect> {
    if evt.kind == ButtonEventKind::Press
        && evt.button == MouseButtonKind::Left
        && modifier_held
        && let Some(uri) = link_at_cell
    {
        return vec![MouseEffect::OpenUri(uri)];
    }

    let mut effects = match ButtonAction::route(modes, evt, mods, cfg) {
        ButtonAction::Noop => Vec::new(),
        ButtonAction::WriteToPty(b) => vec![MouseEffect::Write(b)],
        ButtonAction::ClearAndWriteToPty(b) => vec![MouseEffect::SelClear, MouseEffect::Write(b)],
        ButtonAction::ArmDrag { ty, cell, side } => {
            gesture.drag = Some(DragGesture {
                button: evt.button,
                origin: cell,
                side,
                ty,
                phase: DragPhase::Armed,
                last_cell: cell,
            });
            vec![MouseEffect::SelClear]
        }
        ButtonAction::StartLocalSelection { ty, cell, side } => {
            gesture.drag = Some(DragGesture {
                button: evt.button,
                origin: cell,
                side,
                ty,
                phase: DragPhase::Started,
                last_cell: cell,
            });
            vec![MouseEffect::SelStart { point: to_viewport_point(cell), side, ty }]
        }
        ButtonAction::UpdateLocalSelection { cell, side } => update_selection(gesture, cell, side),
        ButtonAction::ClearLocalSelection => {
            gesture.drag = None;
            vec![MouseEffect::SelClear]
        }
    };

    if evt.kind == ButtonEventKind::Release && evt.button == MouseButtonKind::Left {
        if effects.is_empty()
            && matches!(&gesture.drag, Some(d) if d.phase == DragPhase::Started)
        {
            effects.push(MouseEffect::Copy);
        }
        gesture.drag = None;
    }
    effects
}

/// Lazily materializes an armed selection on the first cell change, then extends.
fn update_selection(gesture: &mut OzmaMouseGesture, cell: CellCoord, side: Side) -> Vec<MouseEffect> {
    let Some(drag) = gesture.drag.as_mut() else {
        return Vec::new();
    };
    match drag.phase {
        DragPhase::Armed => {
            if cell == drag.origin {
                return Vec::new();
            }
            let origin = drag.origin;
            let ty = drag.ty;
            let origin_side = drag.side;
            drag.phase = DragPhase::Started;
            drag.last_cell = cell;
            vec![
                MouseEffect::SelStart { point: to_viewport_point(origin), side: origin_side, ty },
                MouseEffect::SelUpdate { point: to_viewport_point(cell), side },
            ]
        }
        DragPhase::Started => {
            drag.last_cell = cell;
            vec![MouseEffect::SelUpdate { point: to_viewport_point(cell), side }]
        }
    }
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p ozma_terminal --lib mouse::`
Expected: PASS (all decide_button tests).

- [ ] **Step 5: Commit**

```bash
git add crates/ozma_terminal/src/mouse.rs
git commit -m "feat(ozma_terminal): pure decide_button + MouseEffect

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Wheel accumulation + `decide_wheel` (pure)

**Files:**
- Modify: `crates/ozma_terminal/src/mouse.rs`

**Interfaces:**
- Consumes: engine `WheelAction::route`, `WheelConfig`, `WheelModifiers`, `CellCoord`.
- Produces: `WheelAccumulator` (Resource); `fn accumulate_notches(acc: &mut WheelAccumulator, delta_cells: f32, cells_per_notch: f32) -> i32`; `fn wheel_delta_cells(unit: MouseScrollUnit, y: f32, cell_h: f32) -> f32`; `fn decide_wheel(modes: TermMode, notches: i32, cell: CellCoord, mods: WheelModifiers, cfg: &WheelConfig) -> Vec<MouseEffect>`.

- [ ] **Step 1: Write the failing tests**

Add to `mouse.rs` `mod tests`:

```rust
use ozma_tty_engine::{WheelConfig, WheelModifiers};

#[test]
fn line_delta_is_direct_pixel_divides_by_cell_height() {
    assert_eq!(wheel_delta_cells(MouseScrollUnit::Line, 2.0, 16.0), 2.0);
    assert_eq!(wheel_delta_cells(MouseScrollUnit::Pixel, 32.0, 16.0), 2.0);
}

#[test]
fn accumulator_emits_on_threshold_and_carries_remainder() {
    let mut acc = WheelAccumulator::default();
    assert_eq!(accumulate_notches(&mut acc, 0.3, 0.5), 0);
    assert_eq!(accumulate_notches(&mut acc, 0.3, 0.5), 1);
    assert_eq!(accumulate_notches(&mut acc, -1.0, 0.5), -1);
}

#[test]
fn scrollback_up_returns_positive_viewport_scroll() {
    // Bevy +y (wheel up) → caller negates → engine notches negative → into history.
    let fx = decide_wheel(TermMode::empty(), -1, CellCoord { col: 1, row: 1 }, WheelModifiers::default(), &WheelConfig::default());
    assert_eq!(fx, vec![MouseEffect::Scroll(3)]);
}

#[test]
fn app_capture_wheel_forwards_bytes() {
    let modes = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
    let fx = decide_wheel(modes, -1, CellCoord { col: 1, row: 1 }, WheelModifiers::default(), &WheelConfig::default());
    assert!(matches!(fx.as_slice(), [MouseEffect::Write(b)] if !b.is_empty()));
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p ozma_terminal --lib mouse::`
Expected: FAIL — wheel items not found.

- [ ] **Step 3: Implement the wheel decision**

Add to `mouse.rs` (extend `use` with `WheelAction`, `WheelModifiers`, `MouseScrollUnit`):

```rust
/// Carries the sub-notch wheel remainder across frames.
#[derive(Resource, Default)]
pub(crate) struct WheelAccumulator {
    residual_cells: f32,
}

/// Cells of scroll for one wheel event: `Line` units count directly, `Pixel`
/// units divide by the cell height. Positive = wheel-up (toward older lines).
pub(crate) fn wheel_delta_cells(unit: MouseScrollUnit, y: f32, cell_h: f32) -> f32 {
    match unit {
        MouseScrollUnit::Line => y,
        MouseScrollUnit::Pixel => y / cell_h.max(1.0),
    }
}

/// Adds `delta_cells` to the accumulator and returns whole notches to emit
/// (positive = up/older), carrying the remainder. Resets on a sign flip.
pub(crate) fn accumulate_notches(
    acc: &mut WheelAccumulator,
    delta_cells: f32,
    cells_per_notch: f32,
) -> i32 {
    if acc.residual_cells != 0.0 && acc.residual_cells.signum() != delta_cells.signum() {
        acc.residual_cells = 0.0;
    }
    let threshold = cells_per_notch.max(f32::EPSILON);
    acc.residual_cells += delta_cells;
    let notches = (acc.residual_cells / threshold).trunc() as i32;
    if notches != 0 {
        acc.residual_cells -= notches as f32 * threshold;
    }
    notches
}

/// Pure wheel decision. `notches` is in the engine convention (negative =
/// up/older); callers negate the Bevy-derived up-positive value before calling.
pub(crate) fn decide_wheel(
    modes: TermMode,
    notches: i32,
    cell: CellCoord,
    mods: WheelModifiers,
    cfg: &WheelConfig,
) -> Vec<MouseEffect> {
    match WheelAction::route(modes, notches, cell, mods, cfg) {
        WheelAction::Noop => Vec::new(),
        WheelAction::WriteToPty(b) => vec![MouseEffect::Write(b)],
        WheelAction::ScrollViewport(lines) => vec![MouseEffect::Scroll(lines)],
    }
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p ozma_terminal --lib mouse::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/ozma_terminal/src/mouse.rs
git commit -m "feat(ozma_terminal): pure decide_wheel + pixel-notch accumulation

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Hyperlink module — open + hover cursor

**Files:**
- Create: `crates/ozma_terminal/src/hyperlink.rs`
- Modify: `crates/ozma_terminal/src/lib.rs` (`mod hyperlink;`), `crates/ozma_terminal/src/mouse.rs` (uncomment the `use crate::hyperlink::{...}`)

**Interfaces:**
- Consumes: `current_terminal_modifiers`, `ozma_tty_renderer::schema::{is_allowed, HyperlinkHoverState, TerminalGrid}`, `TerminalCellMetricsResource`.
- Produces: `fn link_modifier_held(mods: &ProtocolModifiers) -> bool`; `fn try_open_uri(uri: &str)`; `fn cursor_decision(has_link: bool, modifier_held: bool, over_grid: bool) -> SystemCursorIcon`; `hyperlink_hover_cursor` system; `HyperlinkInputPlugin` (or fold registration into `OzmaMousePlugin`).

- [ ] **Step 1: Write the failing tests**

Create `crates/ozma_terminal/src/hyperlink.rs` with the `use` block + `#[cfg(test)] mod tests`:

```rust
#[test]
fn link_modifier_matches_platform() {
    let mut m = ProtocolModifiers::default();
    assert!(!link_modifier_held(&m));
    if cfg!(target_os = "macos") {
        m.meta = true;
    } else {
        m.ctrl = true;
    }
    assert!(link_modifier_held(&m));
}

#[test]
fn cursor_decision_pointer_only_on_link_with_modifier() {
    assert_eq!(cursor_decision(true, true, true), SystemCursorIcon::Pointer);
    assert_eq!(cursor_decision(true, false, true), SystemCursorIcon::Text);
    assert_eq!(cursor_decision(false, true, true), SystemCursorIcon::Text);
    assert_eq!(cursor_decision(false, false, false), SystemCursorIcon::Default);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p ozma_terminal --lib hyperlink::`
Expected: FAIL — module/functions not found.

- [ ] **Step 3: Implement `hyperlink.rs`**

```rust
//! Cmd/Ctrl-click hyperlink activation and hover-cursor feedback for the Ozma
//! terminal. The click-open path is invoked from the mouse dispatcher; the hover
//! system updates `HyperlinkHoverState` (renderer underline) and the window
//! `CursorIcon`.

use crate::input::{InputDisabled, current_terminal_modifiers};
use crate::mouse::{OzmaTerminalMouseSet, cell_at_cursor};
use crate::spawn::OzmaTerminal;
use bevy::input::keyboard::KeyboardInput;
use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::window::{CursorIcon, PrimaryWindow, SystemCursorIcon, Window};
use ozma_tty_engine::ProtocolModifiers;
use ozma_tty_renderer::TerminalCellMetricsResource;
use ozma_tty_renderer::schema::{HyperlinkHoverState, TerminalGrid, is_allowed};

/// Returns `true` when the platform link-activation modifier is held: Cmd on
/// macOS, Ctrl elsewhere.
pub(crate) fn link_modifier_held(mods: &ProtocolModifiers) -> bool {
    if cfg!(target_os = "macos") {
        mods.meta
    } else {
        mods.ctrl
    }
}

/// Validates `uri` against the shared allowlist and opens it via the OS default
/// handler. Disallowed URIs are dropped with a debug log.
pub(crate) fn try_open_uri(uri: &str) {
    if !is_allowed(uri) {
        debug!("hyperlink: dropping disallowed uri {}", uri);
        return;
    }
    if let Err(e) = open::that_detached(uri) {
        warn!("hyperlink: failed to open {}: {}", uri, e);
    }
}

/// The cursor icon for a hover state: pointer over a link with the modifier
/// held, I-beam over the grid, arrow elsewhere.
pub(crate) fn cursor_decision(
    has_link: bool,
    modifier_held: bool,
    over_grid: bool,
) -> SystemCursorIcon {
    match (over_grid, has_link, modifier_held) {
        (true, true, true) => SystemCursorIcon::Pointer,
        (true, _, _) => SystemCursorIcon::Text,
        _ => SystemCursorIcon::Default,
    }
}

/// Updates `HyperlinkHoverState` and the window cursor as the pointer moves over
/// the terminal grid. Gated to the single enabled `OzmaTerminal`.
fn hyperlink_hover_cursor(
    mut hover: ResMut<HyperlinkHoverState>,
    mut cursor_icons: Query<&mut CursorIcon, With<PrimaryWindow>>,
    terminal: Query<
        (Entity, &ComputedNode, &UiGlobalTransform, &TerminalGrid),
        (With<OzmaTerminal>, Without<InputDisabled>),
    >,
    metrics: Res<TerminalCellMetricsResource>,
    keys: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let modifier_held = link_modifier_held(&{
        let m = current_terminal_modifiers(&keys);
        ProtocolModifiers { shift: m.shift, ctrl: m.ctrl, alt: m.alt, meta: m.meta }
    });
    hover.entity = None;
    hover.hyperlink_id = None;
    hover.modifier_held = modifier_held;

    let decision = resolve_hover(&mut hover, &terminal, &metrics, &windows, modifier_held);
    if let Ok(mut icon) = cursor_icons.single_mut() {
        let desired = CursorIcon::System(decision);
        if *icon != desired {
            *icon = desired;
        }
    }
}

fn resolve_hover(
    hover: &mut HyperlinkHoverState,
    terminal: &Query<
        (Entity, &ComputedNode, &UiGlobalTransform, &TerminalGrid),
        (With<OzmaTerminal>, Without<InputDisabled>),
    >,
    metrics: &TerminalCellMetricsResource,
    windows: &Query<&Window, With<PrimaryWindow>>,
    modifier_held: bool,
) -> SystemCursorIcon {
    let Ok(window) = windows.single() else {
        return SystemCursorIcon::Default;
    };
    let Some(cursor_phys) = window.cursor_position().map(|c| c * window.scale_factor()) else {
        return SystemCursorIcon::Default;
    };
    let Ok((entity, node, transform, grid)) = terminal.single() else {
        return SystemCursorIcon::Default;
    };
    let cell_w = metrics.metrics.advance_phys.floor().max(1.0);
    let cell_h = metrics.metrics.line_height_phys.floor().max(1.0);
    let Some((cell, _side)) = cell_at_cursor(node, transform, cursor_phys, cell_w, cell_h, grid.cols, grid.rows)
    else {
        return SystemCursorIcon::Default;
    };
    let id = grid
        .hyperlink_at((cell.row - 1) as u16, (cell.col - 1) as u16)
        .map(|(id, _uri)| id);
    hover.entity = Some(entity);
    hover.hyperlink_id = id;
    cursor_decision(id.is_some(), modifier_held, true)
}
```

Register the hover system in `OzmaMousePlugin::build` (Task 2) by extending its chain:

```rust
app.init_resource::<OzmaMouseConfig>()
    .init_resource::<OzmaMouseGesture>()
    .init_resource::<crate::mouse::WheelAccumulator>()
    .add_systems(
        Update,
        crate::hyperlink::hyperlink_hover_cursor
            .in_set(OzmaTerminalMouseSet)
            .run_if(on_message::<KeyboardInput>.or(on_message::<CursorMoved>)),
    );
```

(`hyperlink_hover_cursor` must be `pub(crate)` for the plugin to name it; adjust `fn` → `pub(crate) fn`. Add `mod hyperlink;` to `lib.rs`. Add the imports `use bevy::input::keyboard::KeyboardInput;`, `use bevy::window::CursorMoved;` to `mouse.rs` for the run conditions. `HyperlinkHoverState` is a renderer resource — confirm it is registered by the renderer plugin; if not present in a headless test, the system is simply not exercised.)

- [ ] **Step 4: Run the tests**

Run: `cargo test -p ozma_terminal --lib hyperlink:: && cargo build -p ozma_terminal`
Expected: PASS + compiles.

- [ ] **Step 5: Commit**

```bash
git add crates/ozma_terminal/src/hyperlink.rs crates/ozma_terminal/src/lib.rs crates/ozma_terminal/src/mouse.rs
git commit -m "feat(ozma_terminal): hyperlink open + hover cursor

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: Dispatch systems + apply, with gating test

**Files:**
- Modify: `crates/ozma_terminal/src/mouse.rs`

**Interfaces:**
- Consumes: everything from Tasks 2–5; engine `TerminalHandle`/`PtyHandle`/`Coalescer`, `Clipboard`, `TerminalGrid`, `TerminalCellMetricsResource`.
- Produces: `dispatch_mouse_buttons`, `dispatch_mouse_wheel` systems (registered in `OzmaMousePlugin`), `apply_effect`.

- [ ] **Step 1: Write the failing gating test**

Add to `mouse.rs` `mod tests`:

```rust
use crate::spawn::OzmaTerminal;
use crate::input::InputDisabled;
use bevy::input::mouse::{MouseButton, MouseButtonInput};

#[test]
fn input_disabled_terminal_drains_without_arming_a_gesture() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_message::<MouseButtonInput>()
        .init_resource::<OzmaMouseConfig>()
        .init_resource::<OzmaMouseGesture>()
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<Clipboard>()
        .insert_resource(test_metrics())
        .add_systems(Update, dispatch_mouse_buttons);
    app.world_mut().spawn((OzmaTerminal, InputDisabled));
    app.world_mut().spawn((Window { focused: true, ..default() }, PrimaryWindow));
    app.world_mut()
        .resource_mut::<bevy::ecs::message::Messages<MouseButtonInput>>()
        .write(MouseButtonInput { button: MouseButton::Left, state: ButtonState::Pressed, window: Entity::PLACEHOLDER });
    app.update();
    assert!(app.world().resource::<OzmaMouseGesture>().drag.is_none());
}

fn test_metrics() -> TerminalCellMetricsResource {
    use ozma_tty_renderer::CellMetrics;
    TerminalCellMetricsResource {
        metrics: CellMetrics {
            advance_phys: 8.0, line_height_phys: 16.0, ascent_phys: 12.0, descent_phys: 4.0,
            underline_position_phys: -2.0, underline_thickness_phys: 1.0, max_overflow_phys: 0.0,
        },
        phys_font_size: 16,
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p ozma_terminal --lib mouse::input_disabled`
Expected: FAIL — `dispatch_mouse_buttons` not found.

- [ ] **Step 3: Implement the systems + apply**

Add to `mouse.rs` (extend `use` with `Clipboard`, `try_open_uri`, `link_modifier_held`, `MouseButton`, `MouseButtonInput`, `TerminalHandle`, `PtyHandle`, `Coalescer`, `TerminalGrid`, `Time`, `Real`, `ButtonState`):

```rust
/// The crate's mouse-button dispatcher. Resolves the cursor cell, tracks clicks
/// and drag state, drives `decide_button`, and applies the effects. Skips the
/// `OzmaTerminal` while it carries `InputDisabled`.
pub(crate) fn dispatch_mouse_buttons(
    mut gesture: ResMut<OzmaMouseGesture>,
    mut buttons: MessageReader<MouseButtonInput>,
    mut terminal: Query<
        (&mut TerminalHandle, &mut PtyHandle, &mut Coalescer, &ComputedNode, &UiGlobalTransform, &TerminalGrid),
        (With<OzmaTerminal>, Without<InputDisabled>),
    >,
    mut clipboard: ResMut<Clipboard>,
    cfg: Res<OzmaMouseConfig>,
    metrics: Res<TerminalCellMetricsResource>,
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time<Real>>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let Ok((mut handle, mut pty, mut coalescer, node, transform, grid)) = terminal.single_mut() else {
        buttons.clear();
        gesture.drag = None;
        return;
    };
    let Ok(window) = windows.single() else {
        buttons.clear();
        gesture.drag = None;
        return;
    };
    if !window.focused {
        buttons.clear();
        gesture.drag = None;
        return;
    }
    let Some(cursor_phys) = window.cursor_position().map(|c| c * window.scale_factor()) else {
        buttons.clear();
        return;
    };
    let scale = window.scale_factor();
    let cell_w = metrics.metrics.advance_phys.floor().max(1.0);
    let cell_h = metrics.metrics.line_height_phys.floor().max(1.0);
    let modes = handle.current_modes();
    let mods = protocol_mods(&keys);
    let modifier_held = link_modifier_held(&mods);

    for ev in buttons.read() {
        let Some(button) = map_button(ev.button) else { continue };
        let Some((cell, side)) = cell_at_cursor(node, transform, cursor_phys, cell_w, cell_h, grid.cols, grid.rows) else { continue };
        let kind = match ev.state {
            ButtonState::Pressed => ButtonEventKind::Press,
            ButtonState::Released => ButtonEventKind::Release,
        };
        let click_count = if kind == ButtonEventKind::Press {
            gesture.click.register(time.elapsed(), cursor_phys / scale, (cfg.double_click_timeout, cfg.click_drift_px))
        } else {
            1
        };
        let link_at_cell = (kind == ButtonEventKind::Press && button == MouseButtonKind::Left && modifier_held)
            .then(|| grid.hyperlink_at((cell.row - 1) as u16, (cell.col - 1) as u16).map(|(_id, uri)| uri.as_str().to_string()))
            .flatten();
        let evt = ButtonEvent { kind, button, cell, side, click_count };
        let effects = decide_button(&mut gesture, modes, evt, mods, modifier_held, link_at_cell, &cfg.buttons);
        for effect in effects {
            apply_effect(&mut handle, &mut pty, &mut coalescer, &mut clipboard, effect);
        }
    }

    if gesture.drag.is_some()
        && let Some((cell, side)) = cell_at_cursor(node, transform, cursor_phys, cell_w, cell_h, grid.cols, grid.rows)
        && gesture.drag.as_ref().is_some_and(|d| d.last_cell != cell)
    {
        let button = gesture.drag.as_ref().map(|d| d.button).unwrap_or(MouseButtonKind::Left);
        let evt = ButtonEvent { kind: ButtonEventKind::Drag, button, cell, side, click_count: 1 };
        let effects = decide_button(&mut gesture, modes, evt, mods, modifier_held, None, &cfg.buttons);
        for effect in effects {
            apply_effect(&mut handle, &mut pty, &mut coalescer, &mut clipboard, effect);
        }
    }
}

/// The crate's wheel dispatcher: accumulates notches, drives `decide_wheel`,
/// applies the result.
pub(crate) fn dispatch_mouse_wheel(
    mut gesture_acc: ResMut<WheelAccumulator>,
    mut wheel: MessageReader<MouseWheel>,
    mut terminal: Query<
        (&mut TerminalHandle, &mut PtyHandle, &mut Coalescer, &ComputedNode, &UiGlobalTransform, &TerminalGrid),
        (With<OzmaTerminal>, Without<InputDisabled>),
    >,
    mut clipboard: ResMut<Clipboard>,
    cfg: Res<OzmaMouseConfig>,
    metrics: Res<TerminalCellMetricsResource>,
    keys: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let Ok((mut handle, mut pty, mut coalescer, node, transform, grid)) = terminal.single_mut() else {
        wheel.clear();
        return;
    };
    let Ok(window) = windows.single() else { wheel.clear(); return };
    if !window.focused { wheel.clear(); return }
    let cell_h = metrics.metrics.line_height_phys.floor().max(1.0);
    let cell_w = metrics.metrics.advance_phys.floor().max(1.0);

    let mut delta_cells = 0.0f32;
    for ev in wheel.read() {
        delta_cells += wheel_delta_cells(ev.unit, ev.y, cell_h);
    }
    let raw = accumulate_notches(&mut gesture_acc, delta_cells, cfg.cells_per_notch);
    if raw == 0 {
        return;
    }
    // Bevy +y (up/older) → engine convention (negative = up/older).
    let notches = -raw;
    let cell = window
        .cursor_position()
        .map(|c| c * window.scale_factor())
        .and_then(|p| cell_at_cursor(node, transform, p, cell_w, cell_h, grid.cols, grid.rows))
        .map(|(cell, _)| cell)
        .unwrap_or(CellCoord { col: 1, row: 1 });
    let m = current_terminal_modifiers(&keys);
    let mods = WheelModifiers {
        shift: m.shift,
        ctrl: m.ctrl,
        alt: m.alt,
        fine: fine_held(cfg.fine_modifier, &m),
    };
    for effect in decide_wheel(handle.current_modes(), notches, cell, mods, &cfg.wheel) {
        apply_effect(&mut handle, &mut pty, &mut coalescer, &mut clipboard, effect);
    }
}

fn fine_held(modifier: FineModifier, m: &ozma_tty_engine::TerminalModifiers) -> bool {
    match modifier {
        FineModifier::Shift => m.shift,
        FineModifier::Ctrl => m.ctrl,
        FineModifier::Alt => m.alt,
        FineModifier::None => true,
    }
}

fn map_button(b: MouseButton) -> Option<MouseButtonKind> {
    match b {
        MouseButton::Left => Some(MouseButtonKind::Left),
        MouseButton::Middle => Some(MouseButtonKind::Middle),
        MouseButton::Right => Some(MouseButtonKind::Right),
        _ => None,
    }
}

fn apply_effect(
    handle: &mut TerminalHandle,
    pty: &mut PtyHandle,
    coalescer: &mut Coalescer,
    clipboard: &mut Clipboard,
    effect: MouseEffect,
) {
    match effect {
        MouseEffect::Write(b) => {
            if let Err(e) = handle.write(pty, &b) {
                tracing::warn!(?e, "ozma mouse pty write failed");
            }
        }
        MouseEffect::SelStart { point, side, ty } => handle.selection_start_at(coalescer, point, side, ty),
        MouseEffect::SelUpdate { point, side } => handle.selection_update_to(coalescer, point, side),
        MouseEffect::SelClear => handle.selection_clear(coalescer),
        MouseEffect::Copy => {
            if let Some(text) = handle.selection_to_string() {
                clipboard.write(text);
            }
        }
        MouseEffect::Scroll(lines) => handle.scroll(coalescer, lines),
        MouseEffect::OpenUri(uri) => try_open_uri(&uri),
    }
}
```

Add `use ozma_tty_engine::TerminalModifiers;` to the `use` block (or write `current_terminal_modifiers`'s return type via its existing re-export). Register both systems in `OzmaMousePlugin::build`:

```rust
.add_systems(
    Update,
    (dispatch_mouse_buttons, dispatch_mouse_wheel)
        .in_set(OzmaTerminalMouseSet)
        .run_if(on_message::<MouseButtonInput>.or(on_message::<CursorMoved>).or(on_message::<MouseWheel>)),
)
```

(Add `app.add_message::<MouseButtonInput>().add_message::<MouseWheel>();` if not already present from `DefaultPlugins` — in the headless test we add them manually.)

- [ ] **Step 4: Run the gating test + full crate tests**

Run: `cargo test -p ozma_terminal`
Expected: PASS (gating test + all pure tests).

- [ ] **Step 5: Lint + commit**

Run: `cargo clippy -p ozma_terminal --all-targets`
Expected: no warnings.

```bash
git add crates/ozma_terminal/src/mouse.rs
git commit -m "feat(ozma_terminal): mouse-button + wheel dispatch systems

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: Host wiring + manual verification

**Files:**
- Modify: `crates/ozma_terminal/src/input.rs` (promote `current_terminal_modifiers` to `pub(crate)`)
- Modify: `src/input/shortcuts.rs` (add `populate_mouse_config`)
- Modify: `src/input.rs` (register it)
- Modify: `src/ozma_input.rs` (`.before(OzmaTerminalMouseSet)`)

**Interfaces:**
- Consumes: `ozma_terminal::{OzmaMouseConfig, FineModifier, OzmaTerminalMouseSet}`, `OzmuxConfigsResource.mouse`.

- [ ] **Step 1: Promote `current_terminal_modifiers`**

In `crates/ozma_terminal/src/input.rs:128`, change `fn current_terminal_modifiers` to `pub(crate) fn current_terminal_modifiers`.

Run: `cargo build -p ozma_terminal`
Expected: compiles (it is now used by `mouse.rs`/`hyperlink.rs`).

- [ ] **Step 2: Write the failing config-mapping test**

In `src/input/shortcuts.rs` `#[cfg(test)] mod tests`, add (after adding the function in Step 3 you will re-run):

```rust
#[test]
fn mouse_config_maps_from_ozmux_config() {
    use ozmux_configs::mouse::{FineModifier as CfgFine, MouseConfig};
    let mut mc = MouseConfig::default();
    mc.fine_modifier = CfgFine::Ctrl;
    mc.max_protocol_events_per_frame = 5;
    mc.cells_per_notch = 1.0;
    let out = ozma_mouse_config(&mc);
    assert_eq!(out.buttons.max_protocol_events_per_frame, 5);
    assert_eq!(out.wheel.max_protocol_events_per_frame, 5);
    assert_eq!(out.wheel.lines_per_notch, mc.lines_per_notch);
    assert_eq!(out.cells_per_notch, 1.0);
    assert_eq!(out.fine_modifier, ozma_terminal::FineModifier::Ctrl);
    assert_eq!(out.double_click_timeout, std::time::Duration::from_millis(mc.double_click_timeout_ms as u64));
    assert_eq!(out.click_drift_px, mc.click_drift_px);
}
```

- [ ] **Step 3: Implement the mapping + Startup system**

Add to `src/input/shortcuts.rs` (add `use ozma_terminal::{OzmaMouseConfig, FineModifier};` and `use ozmux_configs::mouse::{FineModifier as CfgFineModifier, MouseConfig};` to the top `use` block; `use ozma_tty_engine::{ButtonConfig, WheelConfig};` too):

```rust
/// Maps the resolved `[mouse]` config block to the terminal crate's
/// `OzmaMouseConfig`.
fn ozma_mouse_config(mc: &MouseConfig) -> OzmaMouseConfig {
    OzmaMouseConfig {
        buttons: ButtonConfig { max_protocol_events_per_frame: mc.max_protocol_events_per_frame },
        wheel: WheelConfig {
            lines_per_notch: mc.lines_per_notch,
            fine_lines: mc.fine_lines,
            max_protocol_events_per_frame: mc.max_protocol_events_per_frame,
        },
        cells_per_notch: mc.cells_per_notch,
        double_click_timeout: std::time::Duration::from_millis(mc.double_click_timeout_ms as u64),
        click_drift_px: mc.click_drift_px,
        fine_modifier: match mc.fine_modifier {
            CfgFineModifier::Shift => FineModifier::Shift,
            CfgFineModifier::Ctrl => FineModifier::Ctrl,
            CfgFineModifier::Alt => FineModifier::Alt,
            CfgFineModifier::None => FineModifier::None,
        },
    }
}

/// `Startup` system: inserts `OzmaMouseConfig` from the resolved `[mouse]` block.
pub(crate) fn populate_mouse_config(mut commands: Commands, configs: Res<OzmuxConfigsResource>) {
    commands.insert_resource(ozma_mouse_config(&configs.mouse));
}
```

- [ ] **Step 4: Register the Startup system**

In `src/input.rs`, add `shortcuts::populate_mouse_config` to the same `Startup` tuple that holds `shortcuts::build_resolved_shortcuts` / `shortcuts::populate_input_bindings` (around `src/input.rs:37-38`).

- [ ] **Step 5: Gate the mouse set in the host**

In `src/ozma_input.rs`: add `OzmaTerminalMouseSet` to the import from `ozma_terminal`, and extend the `maintain_input_disabled` registration:

```rust
maintain_input_disabled
    .before(OzmaTerminalInputSet)
    .before(OzmaTerminalMouseSet)
    .run_if(in_state(AppMode::Ozma)),
```

- [ ] **Step 6: Build, test, lint**

Run: `cargo test -p ozma_terminal && cargo test -p ozmux-gui mouse_config && cargo build && cargo clippy --workspace --all-targets && cargo fmt --check`
Expected: PASS / no warnings.

- [ ] **Step 7: Manual smoke test**

Run: `cargo run`
Verify in Ozma mode (no tmux): (1) run `vim`, click to move the cursor and drag to select — vim reacts (app reporting); (2) at the shell, drag across text → it highlights; release → paste into another app confirms the clipboard got it; (3) `seq 200` then wheel up → the viewport scrolls into scrollback, wheel down returns to the tail; (4) print an OSC-8 hyperlink (`printf '\e]8;;https://example.com\e\\link\e]8;;\e\\\n'`), hover with Cmd held → cursor becomes a pointer; Cmd-click → browser opens.

- [ ] **Step 8: Commit**

```bash
git add crates/ozma_terminal/src/input.rs src/input/shortcuts.rs src/input.rs src/ozma_input.rs
git commit -m "feat(ozma): wire host mouse config + InputDisabled gating for the terminal

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review Notes

- **Spec coverage:** app reporting (T3 forward path + T6 apply), selection + copy (T3 + T6 copy-on-release), wheel scrollback + alt-screen (T4 + T6), Cmd-click + hover (T5 + T3 OpenUri), config (T2 + T7), gating (T6 + T7), allowlist sharing (T1). Non-goals (autoscroll, inline-webview, middle-click paste, multi-button, Cmd+C) are not implemented, as intended.
- **Type consistency:** `MouseEffect`, `decide_button`/`decide_wheel`, `cell_at_local`/`cell_at_cursor`/`to_viewport_point`, `OzmaMouseConfig`, `FineModifier`, `OzmaTerminalMouseSet`, `OzmaMouseGesture`/`DragGesture`/`DragPhase`, `ClickTracker`, `WheelAccumulator` are defined in Task 2–4 and consumed by the same names in Tasks 5–7. `ButtonConfig`/`WheelConfig`/`WheelModifiers`/`ProtocolModifiers`/`CellCoord`/`Point`/`Side`/`SelectionType`/`TermMode` are engine re-exports.
- **Known soft spot:** the systems (`dispatch_mouse_buttons`/`dispatch_mouse_wheel`) are covered only by the headless gating test + the pure decision tests; full end-to-end behavior is covered by the Task 7 manual smoke test (a `TerminalHandle` has no public constructor for a unit test).
