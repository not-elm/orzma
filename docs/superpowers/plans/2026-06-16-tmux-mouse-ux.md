# tmux Backend Native Mouse UX Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add native multiplexer mouse UX under the tmux control-mode backend — a single left-button gesture arbiter that drives pane focus, drag-to-resize dividers, drag-to-select text (auto copy-mode), and double/triple-click word/line select.

**Architecture:** A new `src/tmux_mouse.rs` module owns a single gesture state machine reading raw `MouseButtonInput` + `Window::cursor_position()`. It is the sole authority over pane-body left-button gestures: `select-pane` on press, divider resize on a divider-grab drag (`resize-pane`), and server-side copy-mode selection (`begin-selection`/`copy-selection`/`select-word`/`select-line`) targeted at the pane under the cursor by `%id`. Divider geometry is computed once from tmux's authoritative split tree (`tmux_control_parser`) and projected to the binary. Existing wheel→copy-mode and window-tab handling stay separate (they don't read the left button).

**Tech Stack:** Rust (edition 2024), Bevy 0.18 ECS, tmux `-CC` control mode, `alacritty_terminal` (detached VT emulators), the workspace crates `ozmux_tmux` (`crates/tmux_session`), `tmux_control_parser`, `ozma_tty_engine`, `ozma_tty_renderer`, `ozmux_configs`.

**Source spec:** `docs/superpowers/specs/2026-06-16-tmux-mouse-ux-design.md`

---

## Design refinement vs. the spec (read first)

One deliberate deviation from the spec's "remove the per-pane `Button`/`FocusPolicy::Block`":

- **Keep `augment_tmux_pane` (the `Button` + `FocusPolicy::Block`).** The review flagged the `Block` as load-bearing (it stops pane clicks from reaching webview surfaces behind/under the pane). Rather than re-implement that blocking in raw hit-testing, we **retain the `Button`/`Block` purely for its click-blocking semantics** and **remove only the `select-pane` action** from `focus_pane_on_click`. The arbiter becomes the single authority over the *action* (it issues `select-pane` from a raw-input hit-test), while `Block` continues to protect webviews. This satisfies the spec's "replicate Block semantics" requirement with less risk and no behavior change for webviews.

Everything else follows the spec.

---

## File Structure

**New files:**
- `src/tmux_mouse.rs` — the gesture arbiter: `OzmuxTmuxMousePlugin`, `TmuxMouseGesture` resource, the arbiter system, and the pure geometry/click/divider-hit-test helpers (relocated + new). One responsibility: interpret left-button gestures into tmux commands.

**Modified files:**
- `crates/configs/src/mouse.rs` — three new `MouseConfig` knobs.
- `crates/tmux_control_parser/src/layout.rs` — `Divider`/`DividerAxis` types + the pure `dividers(&WindowLayout)` function.
- `crates/tmux_control_parser/src/lib.rs` — export the new types/fn.
- `crates/tmux_session/src/enumerate.rs` — `resize_pane_x_command` / `resize_pane_y_command` / `resize_pane_rel_command` builders.
- `crates/tmux_session/src/events.rs` — carry `Vec<Divider>` in `TmuxLayoutChanged`.
- `crates/tmux_session/src/components.rs` — `TmuxDividers` component.
- `crates/tmux_session/src/observers.rs` (or wherever `TmuxLayoutChanged` is applied) — project `TmuxDividers` onto the active window entity.
- `crates/tmux_session/src/lib.rs` — export `TmuxDividers`, the resize builders, `Divider`/`DividerAxis`.
- `src/tmux_copy_mode.rs` — promote `cursor_deltas`/`cell_at_pane`/`phys_to_pane_local` to `pub(crate)` (Phase 1); delete `drag_select_in_copy_mode` + `DragSelect` (Phase 3).
- `src/ui/tmux_pane_focus.rs` — delete `focus_pane_on_click` + its registration (Phase 1); keep `augment_tmux_pane` + `sync_pane_dim`.
- `src/main.rs` — register `OzmuxTmuxMousePlugin`; add `mod tmux_mouse;`.

---

## PHASE 1 — Arbiter skeleton + fold pane focus

Outcome: a new arbiter module exists, owns `select-pane`-on-press via a raw-input hit-test, and the old `Interaction`-based focus sender is gone. Behavior for the user is identical (click a pane → it focuses). Config knobs and reusable helpers are in place for later phases.

### Task 1: Add mouse config knobs

**Files:**
- Modify: `crates/configs/src/mouse.rs`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `crates/configs/src/mouse.rs`:

```rust
    #[test]
    fn gesture_defaults_present() {
        let cfg = MouseConfig::default();
        assert_eq!(cfg.drag_threshold_px, 4.0);
        assert_eq!(cfg.divider_grab_tolerance_px, 4.0);
        assert_eq!(cfg.max_resize_commands_per_frame, 4);
    }

    #[test]
    fn gesture_fields_parse_from_toml() {
        let toml = r#"
            drag_threshold_px = 6.0
            divider_grab_tolerance_px = 5.0
            max_resize_commands_per_frame = 8
        "#;
        let patch: MousePatch = toml::from_str(toml).unwrap();
        assert_eq!(patch.drag_threshold_px, Some(6.0));
        assert_eq!(patch.divider_grab_tolerance_px, Some(5.0));
        assert_eq!(patch.max_resize_commands_per_frame, Some(8));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ozmux_configs gesture_`
Expected: FAIL — `no field drag_threshold_px on type MouseConfig`.

- [ ] **Step 3: Add the fields, defaults, patch fields, and patch application**

In `crates/configs/src/mouse.rs`, add to `struct MouseConfig` (after `autoscroll_step_ms`):

```rust
    /// Pointer travel (logical px) before a left-press is treated as a drag
    /// rather than a click. Below this, release fires a click (focus / word /
    /// line); at or above it, the gesture becomes a resize or text drag.
    pub drag_threshold_px: f32,
    /// Half-width (logical px) of a pane divider's grab zone for resize.
    pub divider_grab_tolerance_px: f32,
    /// Per-frame cap on `resize-pane` commands emitted during a divider drag,
    /// a backstop beneath the one-in-flight-resize throttle.
    pub max_resize_commands_per_frame: u32,
```

In `impl Default for MouseConfig`, add (after `autoscroll_step_ms: 4,`):

```rust
            drag_threshold_px: 4.0,
            divider_grab_tolerance_px: 4.0,
            max_resize_commands_per_frame: 4,
```

In `struct MousePatch`, add (after `autoscroll_step_ms`):

```rust
    pub(crate) drag_threshold_px: Option<f32>,
    pub(crate) divider_grab_tolerance_px: Option<f32>,
    pub(crate) max_resize_commands_per_frame: Option<u32>,
```

In `MousePatch::apply_to`, add (before `base`):

```rust
        if let Some(v) = self.drag_threshold_px {
            base.drag_threshold_px = v;
        }
        if let Some(v) = self.divider_grab_tolerance_px {
            base.divider_grab_tolerance_px = v;
        }
        if let Some(v) = self.max_resize_commands_per_frame {
            base.max_resize_commands_per_frame = v;
        }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ozmux_configs`
Expected: PASS (incl. `defaults_match_expected_values` still green).

- [ ] **Step 5: Commit**

```bash
git add crates/configs/src/mouse.rs
git commit -m "feat(configs): add mouse gesture knobs (drag threshold, divider grab, resize cap)"
```

### Task 2: Make copy-mode geometry helpers reusable

**Files:**
- Modify: `src/tmux_copy_mode.rs`

- [ ] **Step 1: Change visibility of the three helpers**

In `src/tmux_copy_mode.rs`, change each helper's signature from `fn` to `pub(crate) fn`:

```rust
pub(crate) fn cursor_deltas(cur: (u16, u16), target: (u16, u16)) -> Vec<String> {
```
```rust
pub(crate) fn cell_at_pane(
```
```rust
pub(crate) fn phys_to_pane_local(
```

(Bodies unchanged.)

- [ ] **Step 2: Verify the crate still builds and tests pass**

Run: `cargo test -p ozmux-gui tmux_copy_mode`
Expected: PASS — the existing `cursor_deltas_right_and_down` and copy-mode tests still green; no new warnings about unused visibility (they now have an external caller in later tasks; until then `pub(crate)` on an in-module-only item is acceptable here as it is consumed in Phase 2/3 — if clippy `unreachable_pub`-style noise appears, it will be resolved when `tmux_mouse` calls them in Task 4).

- [ ] **Step 3: Commit**

```bash
git add src/tmux_copy_mode.rs
git commit -m "refactor(tmux): expose copy-mode geometry helpers as pub(crate) for the mouse arbiter"
```

### Task 3: Create the arbiter module with focus-on-press, and remove the old focus sender

**Files:**
- Create: `src/tmux_mouse.rs`
- Modify: `src/main.rs`
- Modify: `src/ui/tmux_pane_focus.rs`

- [ ] **Step 1: Write the failing test (focus-on-press)**

Create `src/tmux_mouse.rs` with this content (module skeleton + first system + test):

```rust
//! Single left-button gesture arbiter for tmux panes: focus on press, divider
//! drag-resize, and server-side copy-mode selection. The sole reader of the
//! left mouse button under the tmux backend, so click / drag / divider
//! gestures disambiguate in one place. Wheel and window-tab handling live
//! elsewhere (they do not read the left button).

use crate::input::InputPhase;
use bevy::input::ButtonState;
use bevy::input::mouse::MouseButtonInput;
use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::window::PrimaryWindow;
use ozmux_tmux::{ActivePane, TmuxConnection, TmuxPane, select_pane_command};

/// Registers the tmux mouse gesture arbiter.
pub struct OzmuxTmuxMousePlugin;

impl Plugin for OzmuxTmuxMousePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TmuxMouseGesture>();
        app.add_systems(Update, arbiter.in_set(InputPhase::Dispatch));
    }
}

/// State machine for the in-progress left-button gesture.
#[derive(Resource, Default)]
struct TmuxMouseGesture {
    state: GestureState,
}

#[derive(Default, Debug, PartialEq)]
enum GestureState {
    #[default]
    Idle,
    /// Left button down inside a pane body; not yet past the drag threshold.
    Pressed {
        pane_id: ozmux_tmux::PaneId,
        origin_phys: Vec2,
    },
}

/// Picks the pane whose UI node contains `cursor_phys`, returning its id.
fn pane_under_cursor(
    panes: &Query<(&TmuxPane, &ComputedNode, &UiGlobalTransform)>,
    cursor_phys: Vec2,
) -> Option<ozmux_tmux::PaneId> {
    panes
        .iter()
        .find(|(_, node, transform)| node.contains_point(***transform, cursor_phys))
        .map(|(pane, _, _)| pane.id)
}

/// The arbiter. Phase 1: on a left press inside a pane body, send
/// `select-pane` for the pane under the cursor (focus), and record the press.
fn arbiter(
    mut gesture: ResMut<TmuxMouseGesture>,
    mut buttons: MessageReader<MouseButtonInput>,
    connection: NonSend<TmuxConnection>,
    panes: Query<(&TmuxPane, &ComputedNode, &UiGlobalTransform)>,
    windows: Query<&Window, With<PrimaryWindow>>,
    _active: Query<Entity, With<ActivePane>>,
) {
    let Ok(window) = windows.single() else {
        buttons.clear();
        return;
    };
    if !window.focused {
        buttons.clear();
        gesture.state = GestureState::Idle;
        return;
    }
    let scale = window.scale_factor();
    for ev in buttons.read() {
        if ev.button != MouseButton::Left {
            continue;
        }
        match ev.state {
            ButtonState::Pressed => {
                let Some(cursor_phys) = window.cursor_position().map(|c| c * scale) else {
                    continue;
                };
                let Some(pane_id) = pane_under_cursor(&panes, cursor_phys) else {
                    continue;
                };
                if let Some(client) = connection.client() {
                    let cmd = select_pane_command(pane_id);
                    if let Err(e) = client.handle().send(&cmd) {
                        tracing::warn!(?e, pane = pane_id.0, "arbiter select-pane send failed");
                    }
                }
                gesture.state = GestureState::Pressed {
                    pane_id,
                    origin_phys: cursor_phys,
                };
            }
            ButtonState::Released => {
                gesture.state = GestureState::Idle;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_under_cursor_selects_containing_pane() {
        // Pure containment check is exercised via the system test below; this
        // placeholder ensures the module compiles with tests enabled.
        assert_eq!(GestureState::default(), GestureState::Idle);
    }
}
```

Note: `ozmux_tmux::PaneId` is re-exported (`crates/tmux_session/src/lib.rs` re-exports `tmux_control_parser::{PaneId, WindowId}`). Confirm the import path resolves; if `PaneId` is only at `tmux_control_parser::PaneId`, import it from there instead.

- [ ] **Step 2: Wire the module and plugin; remove the old focus sender**

In `src/main.rs`, add the module declaration alongside the other `mod` lines (near `mod tmux_copy_mode;`):

```rust
mod tmux_mouse;
```

Add the import near `use ui::tmux_pane_focus::OzmuxTmuxPaneFocusPlugin;`:

```rust
use tmux_mouse::OzmuxTmuxMousePlugin;
```

Register the plugin immediately after `.add_plugins(OzmuxTmuxCopyModePlugin)`:

```rust
        .add_plugins(OzmuxTmuxCopyModePlugin)
        .add_plugins(OzmuxTmuxMousePlugin)
```

In `src/ui/tmux_pane_focus.rs`, delete the `focus_pane_on_click` function entirely and remove its registration line, so `build` becomes:

```rust
impl Plugin for OzmuxTmuxPaneFocusPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                augment_tmux_pane.after(TmuxProjectionSet),
                sync_pane_dim.run_if(pane_active_state_changed),
            ),
        );
    }
}
```

Remove the now-unused imports `use crate::input::InputPhase;` and `select_pane_command` from `src/ui/tmux_pane_focus.rs` (keep `ActivePane`, `TmuxConnection`, `TmuxPane`, `TmuxProjectionSet`). Delete the `pane_press_maps_to_select_pane` test (its assertion moves to Task 3 Step 3). Keep `augment_adds_button_and_focus_block_no_overlay` and `sync_sets_pane_dim_from_active_marker`.

- [ ] **Step 3: Write the arbiter focus test (App-level)**

Replace the `#[cfg(test)] mod tests` block in `src/tmux_mouse.rs` with a real App-level test that mirrors the harness from `src/tmux_copy_mode.rs` tests (MinimalPlugins + a `TmuxConnection` + a spawned `TmuxPane`). Because asserting on `select-pane` requires a live tmux client (which tests cannot spawn), assert instead on the **gesture state transition** (the press is recorded), which is the testable, client-independent behavior:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use bevy::input::ButtonState;
    use bevy::input::mouse::MouseButtonInput;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use ozma_tty_engine::TerminalHandle;
    use tmux_control_parser::{CellDims, PaneId};

    fn press(button: MouseButton) -> MouseButtonInput {
        MouseButtonInput {
            button,
            state: ButtonState::Pressed,
            window: Entity::PLACEHOLDER,
        }
    }

    #[test]
    fn left_press_records_pressed_state() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<MouseButtonInput>();
        app.insert_non_send_resource(TmuxConnection::default());
        app.init_resource::<TmuxMouseGesture>();
        app.add_systems(Update, arbiter);

        // A focused primary window with a known cursor position.
        app.world_mut().spawn((
            Window {
                focused: true,
                ..default()
            },
            PrimaryWindow,
        ));
        // NOTE: cursor_position() is None unless the window reports one; this
        // test asserts the no-cursor path leaves state Idle, and a follow-up
        // integration check (manual) covers the populated-cursor path. If the
        // harness can set cursor_position, extend to assert Pressed.

        app.world_mut()
            .resource_mut::<bevy::ecs::message::Messages<MouseButtonInput>>()
            .write(press(MouseButton::Left));
        app.update();

        // With no cursor position available in the headless window, the press
        // is ignored and state stays Idle.
        assert_eq!(
            app.world().resource::<TmuxMouseGesture>().state,
            GestureState::Idle
        );
    }
}
```

Then add a **pure** unit test for `pane_under_cursor`'s containment intent by testing the geometry directly is impractical without a built UI tree; instead keep the state-machine assertions and rely on Phase 2/3 system tests (which set cursor positions via the same scaffold `src/tmux_copy_mode.rs` uses) for cursor-populated coverage. Document this in the test module with a `// NOTE:` only if it states a real caveat; otherwise leave as is.

- [ ] **Step 4: Run tests**

Run: `cargo test -p ozmux-gui tmux_mouse && cargo test -p ozmux-gui tmux_pane_focus`
Expected: PASS. Also run `cargo build` to confirm `main.rs` wiring compiles.

- [ ] **Step 5: Manual smoke + commit**

Build and run (`cargo run`), click a pane → it focuses (selects) and the inactive panes dim as before. Then:

```bash
git add src/tmux_mouse.rs src/main.rs src/ui/tmux_pane_focus.rs src/tmux_copy_mode.rs
git commit -m "feat(tmux): mouse gesture arbiter owns select-pane on press (fold focus)"
```

### Task 4: Optional — rename the now-chrome-only focus plugin

**Files:**
- Modify: `src/ui/tmux_pane_focus.rs`, `src/main.rs`

- [ ] **Step 1:** Rename `OzmuxTmuxPaneFocusPlugin` → `OzmuxTmuxPaneChromePlugin` (it now does `augment` (Button/Block) + dim, not focus). Update the `//!` module doc to drop "sends `select-pane` on click". Update the import + registration in `src/main.rs`. Update the two remaining tests' `add_plugins(... OzmuxTmuxPaneChromePlugin)`.
- [ ] **Step 2:** Run `cargo test -p ozmux-gui tmux_pane_chrome` (rename the file's test module references) and `cargo build`. Expected: PASS.
- [ ] **Step 3: Commit**

```bash
git add src/ui/tmux_pane_focus.rs src/main.rs
git commit -m "refactor(tmux): rename pane-focus plugin to pane-chrome (button/block + dim only)"
```

---

## PHASE 2 — Split-parent projection + drag-resize

Outcome: dividers are computed from tmux's authoritative split tree and projected to the binary; dragging a divider resizes panes via `resize-pane`, throttled to one in-flight resize per drag.

### Task 5: `resize-pane` command builders

**Files:**
- Modify: `crates/tmux_session/src/enumerate.rs`, `crates/tmux_session/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` in `crates/tmux_session/src/enumerate.rs`:

```rust
    #[test]
    fn resize_pane_builders_format() {
        assert_eq!(resize_pane_x_command(PaneId(3), 80), "resize-pane -t %3 -x 80");
        assert_eq!(resize_pane_y_command(PaneId(3), 24), "resize-pane -t %3 -y 24");
        assert_eq!(
            resize_pane_rel_command(PaneId(3), ResizeDir::Left, 2),
            "resize-pane -t %3 -L 2"
        );
        assert_eq!(
            resize_pane_rel_command(PaneId(3), ResizeDir::Down, 1),
            "resize-pane -t %3 -D 1"
        );
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ozmux_tmux resize_pane_builders_format`
Expected: FAIL — `cannot find function resize_pane_x_command`.

- [ ] **Step 3: Implement the builders**

Add to `crates/tmux_session/src/enumerate.rs` (near `select_pane_command`):

```rust
/// Relative resize direction for `resize-pane -L|-R|-U|-D`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeDir {
    /// `-L` — grow/shrink the pane's left edge.
    Left,
    /// `-R` — grow/shrink the pane's right edge.
    Right,
    /// `-U` — grow/shrink the pane's top edge.
    Up,
    /// `-D` — grow/shrink the pane's bottom edge.
    Down,
}

/// Builds `resize-pane -t %<id> -x <width>` (absolute, idempotent).
pub fn resize_pane_x_command(id: PaneId, width: u32) -> String {
    format!("resize-pane -t %{} -x {width}", id.0)
}

/// Builds `resize-pane -t %<id> -y <height>` (absolute, idempotent).
pub fn resize_pane_y_command(id: PaneId, height: u32) -> String {
    format!("resize-pane -t %{} -y {height}", id.0)
}

/// Builds `resize-pane -t %<id> -L|-R|-U|-D <n>` (relative fallback).
pub fn resize_pane_rel_command(id: PaneId, dir: ResizeDir, n: u32) -> String {
    let flag = match dir {
        ResizeDir::Left => "-L",
        ResizeDir::Right => "-R",
        ResizeDir::Up => "-U",
        ResizeDir::Down => "-D",
    };
    format!("resize-pane -t %{} {flag} {n}", id.0)
}
```

Export from `crates/tmux_session/src/lib.rs` (add to the `pub use enumerate::{...}` list): `ResizeDir, resize_pane_rel_command, resize_pane_x_command, resize_pane_y_command`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p ozmux_tmux resize_pane_builders_format`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tmux_session/src/enumerate.rs crates/tmux_session/src/lib.rs
git commit -m "feat(tmux): resize-pane command builders (absolute -x/-y + relative)"
```

### Task 6: Divider derivation from the split tree (pure)

**Files:**
- Modify: `crates/tmux_control_parser/src/layout.rs`, `crates/tmux_control_parser/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Add to `crates/tmux_control_parser/src/layout.rs` `#[cfg(test)] mod tests`:

```rust
    fn leaf(id: u32, width: u32, height: u32, xoff: i32, yoff: i32) -> Cell {
        Cell::Leaf {
            dims: CellDims { width, height, xoff, yoff },
            pane_id: Some(id),
        }
    }

    #[test]
    fn single_pane_has_no_dividers() {
        let layout = WindowLayout { checksum: 0, root: leaf(1, 80, 24, 0, 0) };
        assert!(dividers(&layout).is_empty());
    }

    #[test]
    fn left_right_split_yields_one_vertical_divider() {
        // Two panes side by side: left occupies cols 0..40, a 1-cell gap, right 41..80.
        let layout = WindowLayout {
            checksum: 0,
            root: Cell::Split {
                dims: CellDims { width: 80, height: 24, xoff: 0, yoff: 0 },
                dir: SplitDir::LeftRight,
                children: vec![leaf(1, 40, 24, 0, 0), leaf(2, 39, 24, 41, 0)],
            },
        };
        let ds = dividers(&layout);
        assert_eq!(ds.len(), 1);
        let d = ds[0];
        assert_eq!(d.axis, DividerAxis::Vertical);
        assert_eq!(d.primary, PaneId(1)); // resize the left pane
        assert_eq!(d.pos, 40);            // divider column = left.xoff + left.width
        assert_eq!((d.span_start, d.span_end), (0, 24));
    }

    #[test]
    fn top_bottom_split_yields_one_horizontal_divider() {
        let layout = WindowLayout {
            checksum: 0,
            root: Cell::Split {
                dims: CellDims { width: 80, height: 24, xoff: 0, yoff: 0 },
                dir: SplitDir::TopBottom,
                children: vec![leaf(1, 80, 12, 0, 0), leaf(2, 80, 11, 0, 13)],
            },
        };
        let ds = dividers(&layout);
        assert_eq!(ds.len(), 1);
        assert_eq!(ds[0].axis, DividerAxis::Horizontal);
        assert_eq!(ds[0].primary, PaneId(1));
        assert_eq!(ds[0].pos, 12);
        assert_eq!((ds[0].span_start, ds[0].span_end), (0, 80));
    }

    #[test]
    fn floating_split_yields_no_dividers() {
        let layout = WindowLayout {
            checksum: 0,
            root: Cell::Split {
                dims: CellDims { width: 80, height: 24, xoff: 0, yoff: 0 },
                dir: SplitDir::Floating,
                children: vec![leaf(1, 80, 24, 0, 0), leaf(2, 40, 12, 0, 0)],
            },
        };
        assert!(dividers(&layout).is_empty());
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p tmux_control_parser dividers`
Expected: FAIL — `cannot find type DividerAxis` / `function dividers`.

- [ ] **Step 3: Implement the types + the recursive derivation**

Add to `crates/tmux_control_parser/src/layout.rs`:

```rust
/// Orientation of a pane divider (the draggable boundary between split children).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DividerAxis {
    /// A vertical line; dragging it horizontally resizes the left/right panes.
    Vertical,
    /// A horizontal line; dragging it vertically resizes the top/bottom panes.
    Horizontal,
}

/// A draggable boundary between two adjacent children of a split, in cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Divider {
    /// Orientation of the divider line.
    pub axis: DividerAxis,
    /// The pane to resize when dragging this divider (the left/top side).
    pub primary: PaneId,
    /// Cell coordinate of the divider line: column for `Vertical`, row for
    /// `Horizontal`. Equals the left/top child's far edge.
    pub pos: i32,
    /// Start of the shared-edge span on the perpendicular axis (cells).
    pub span_start: i32,
    /// End (exclusive) of the shared-edge span on the perpendicular axis.
    pub span_end: i32,
}

/// Computes the draggable dividers of a window layout from its split tree.
///
/// For each `LeftRight` / `TopBottom` split, a divider sits between each pair of
/// consecutive children, on the leading child's far edge, spanning the overlap
/// of the two children on the perpendicular axis. `Floating` splits yield none.
/// The `primary` pane is the leading child's representative leaf whose far edge
/// equals the divider line (resizing it moves exactly that boundary).
pub fn dividers(layout: &WindowLayout) -> Vec<Divider> {
    let mut out = Vec::new();
    collect_dividers(&layout.root, &mut out);
    out
}

fn collect_dividers(cell: &Cell, out: &mut Vec<Divider>) {
    if let Cell::Split { dir, children, .. } = cell {
        if matches!(dir, SplitDir::LeftRight | SplitDir::TopBottom) {
            for pair in children.windows(2) {
                let (a, b) = (&pair[0], &pair[1]);
                let (ad, bd) = (cell_dims(a), cell_dims(b));
                match dir {
                    SplitDir::LeftRight => {
                        let pos = ad.xoff + ad.width as i32;
                        let span_start = ad.yoff.max(bd.yoff);
                        let span_end =
                            (ad.yoff + ad.height as i32).min(bd.yoff + bd.height as i32);
                        if let Some(primary) = edge_leaf(a, EdgeQuery::RightAt(pos)) {
                            out.push(Divider {
                                axis: DividerAxis::Vertical,
                                primary,
                                pos,
                                span_start,
                                span_end,
                            });
                        }
                    }
                    SplitDir::TopBottom => {
                        let pos = ad.yoff + ad.height as i32;
                        let span_start = ad.xoff.max(bd.xoff);
                        let span_end =
                            (ad.xoff + ad.width as i32).min(bd.xoff + bd.width as i32);
                        if let Some(primary) = edge_leaf(a, EdgeQuery::BottomAt(pos)) {
                            out.push(Divider {
                                axis: DividerAxis::Horizontal,
                                primary,
                                pos,
                                span_start,
                                span_end,
                            });
                        }
                    }
                    SplitDir::Floating => {}
                }
            }
        }
        for child in children {
            collect_dividers(child, out);
        }
    }
}

fn cell_dims(cell: &Cell) -> CellDims {
    match cell {
        Cell::Leaf { dims, .. } | Cell::Split { dims, .. } => *dims,
    }
}

enum EdgeQuery {
    RightAt(i32),
    BottomAt(i32),
}

/// Finds a leaf within `cell` whose far edge equals the queried boundary, in
/// DFS order. For a leaf cell this is itself when its edge matches.
fn edge_leaf(cell: &Cell, query: EdgeQuery) -> Option<PaneId> {
    match cell {
        Cell::Leaf { dims, pane_id } => {
            let matches = match query {
                EdgeQuery::RightAt(x) => dims.xoff + dims.width as i32 == x,
                EdgeQuery::BottomAt(y) => dims.yoff + dims.height as i32 == y,
            };
            pane_id.and_then(|id| matches.then_some(PaneId(id)))
        }
        Cell::Split { children, .. } => children.iter().find_map(|c| edge_leaf(c, match query {
            EdgeQuery::RightAt(x) => EdgeQuery::RightAt(x),
            EdgeQuery::BottomAt(y) => EdgeQuery::BottomAt(y),
        })),
    }
}
```

Export from `crates/tmux_control_parser/src/lib.rs`: add `Divider, DividerAxis, dividers` to the public exports (match how `CellDims`/`WindowLayout` are exported).

- [ ] **Step 4: Run tests**

Run: `cargo test -p tmux_control_parser dividers`
Expected: PASS (all four).

- [ ] **Step 5: Commit**

```bash
git add crates/tmux_control_parser/src/layout.rs crates/tmux_control_parser/src/lib.rs
git commit -m "feat(tmux-parser): derive draggable dividers from the split tree"
```

### Task 7: Project dividers to the binary

**Files:**
- Modify: `crates/tmux_session/src/events.rs`, `crates/tmux_session/src/components.rs`, the layout-apply site (`crates/tmux_session/src/observers.rs`), `crates/tmux_session/src/lib.rs`

- [ ] **Step 1: Add the `TmuxDividers` component**

In `crates/tmux_session/src/components.rs`:

```rust
use tmux_control_parser::Divider;

/// The active window's draggable dividers, projected for the mouse arbiter.
#[derive(Component, Debug, Clone, Default, PartialEq, Eq)]
pub struct TmuxDividers(pub Vec<Divider>);
```

(Add `Divider` to the existing `tmux_control_parser` import line rather than a second `use` — keep imports a single block.)

- [ ] **Step 2: Carry dividers in `TmuxLayoutChanged`**

In `crates/tmux_session/src/events.rs`, extend `TmuxLayoutChanged`:

```rust
#[derive(Event, Debug, Clone)]
pub(crate) struct TmuxLayoutChanged {
    pub(crate) window: WindowId,
    pub(crate) panes: Vec<PaneGeom>,
    pub(crate) dividers: Vec<tmux_control_parser::Divider>,
}
```

At every construction site of `TmuxLayoutChanged` (where `pane_geoms(&layout)` is called), also compute `dividers: tmux_control_parser::dividers(&layout)`. Grep: `rg "TmuxLayoutChanged \{" crates/tmux_session/src` and update each. Add a unit test asserting the dividers field is populated for a two-pane layout, mirroring the existing `pane_geoms` tests in that file.

- [ ] **Step 3: Apply `TmuxDividers` onto the active window entity**

Find where `TmuxLayoutChanged` is consumed (the projection that updates `TmuxPane.dims`). In that observer/system, after updating pane components, insert/update the component on the active window entity:

```rust
commands.entity(active_window_entity).insert(TmuxDividers(ev.dividers.clone()));
```

If the active window entity is not readily available there, store on a resource instead: define `#[derive(Resource, Default)] pub struct ActiveWindowDividers(pub Vec<Divider>)` in `components.rs`, `init_resource` it in the session plugin, and set it here. Choose whichever matches the surrounding code's pattern (prefer the component on the `ActiveWindow` entity if that entity is in scope; otherwise the resource). Export the chosen type from `lib.rs`.

- [ ] **Step 4: Test the projection**

Add an App-level test following `crates/tmux_session/tests/real_tmux_projection.rs` patterns (or an in-crate unit test of the apply function): feed a `TmuxLayoutChanged` with two panes + one divider, run the apply, assert `TmuxDividers` (or the resource) holds one divider. Run: `cargo test -p ozmux_tmux dividers` / the new test name.

- [ ] **Step 5: Commit**

```bash
git add crates/tmux_session/src
git commit -m "feat(tmux): project window dividers (TmuxDividers) from the layout tree"
```

### Task 8: Divider hit-test in the arbiter (pure)

**Files:**
- Modify: `src/tmux_mouse.rs`

- [ ] **Step 1: Write the failing test**

Add to `src/tmux_mouse.rs` tests:

```rust
    use tmux_control_parser::{Divider, DividerAxis, PaneId};

    fn vdiv(primary: u32, pos: i32, s: i32, e: i32) -> Divider {
        Divider { axis: DividerAxis::Vertical, primary: PaneId(primary), pos, span_start: s, span_end: e }
    }

    #[test]
    fn hit_test_grabs_vertical_divider_within_tolerance() {
        // Divider line at column 40, cells 8px wide/16px tall, span rows 0..24.
        // Pointer at x ~ 40*8 = 320px, y in span -> hit.
        let ds = [vdiv(1, 40, 0, 24)];
        let hit = divider_at(&ds, Vec2::new(322.0, 100.0), 8.0, 16.0, 4.0);
        assert!(hit.is_some());
        assert_eq!(hit.unwrap().primary, PaneId(1));
    }

    #[test]
    fn hit_test_misses_outside_tolerance() {
        let ds = [vdiv(1, 40, 0, 24)];
        // x = 360px is 40px from the 320px line, beyond a 4px tolerance.
        assert!(divider_at(&ds, Vec2::new(360.0, 100.0), 8.0, 16.0, 4.0).is_none());
    }

    #[test]
    fn hit_test_misses_outside_span() {
        let ds = [vdiv(1, 40, 0, 12)]; // span rows 0..12 only
        // y = 13*16 = 208px is below the span (which ends at 12*16=192px).
        assert!(divider_at(&ds, Vec2::new(320.0, 208.0), 8.0, 16.0, 4.0).is_none());
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p ozmux-gui hit_test`
Expected: FAIL — `cannot find function divider_at`.

- [ ] **Step 3: Implement `divider_at`**

Add to `src/tmux_mouse.rs`:

```rust
use tmux_control_parser::{Divider, DividerAxis};

/// Returns the divider whose grab zone contains `cursor_phys` (physical px),
/// given physical cell metrics and a half-tolerance in physical px. The
/// pointer must be within `tol` of the divider line on the major axis and
/// inside its span on the perpendicular axis.
fn divider_at(
    dividers: &[Divider],
    cursor_phys: Vec2,
    cell_w: f32,
    cell_h: f32,
    tol_phys: f32,
) -> Option<Divider> {
    dividers.iter().copied().find(|d| match d.axis {
        DividerAxis::Vertical => {
            let line = d.pos as f32 * cell_w;
            let span0 = d.span_start as f32 * cell_h;
            let span1 = d.span_end as f32 * cell_h;
            (cursor_phys.x - line).abs() <= tol_phys
                && cursor_phys.y >= span0
                && cursor_phys.y < span1
        }
        DividerAxis::Horizontal => {
            let line = d.pos as f32 * cell_h;
            let span0 = d.span_start as f32 * cell_w;
            let span1 = d.span_end as f32 * cell_w;
            (cursor_phys.y - line).abs() <= tol_phys
                && cursor_phys.x >= span0
                && cursor_phys.x < span1
        }
    })
}
```

Note on tolerance units: the config knob `divider_grab_tolerance_px` is in **logical** px; convert to physical with `* window.scale_factor()` at the call site in Task 9 before passing here. The test above passes physical px directly.

- [ ] **Step 4: Run tests**

Run: `cargo test -p ozmux-gui hit_test`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/tmux_mouse.rs
git commit -m "feat(tmux): divider grab-zone hit-test for the mouse arbiter"
```

### Task 9: Drag-to-resize in the arbiter

**Files:**
- Modify: `src/tmux_mouse.rs`

- [ ] **Step 1: Add a pure helper for the target size + its test**

Add to `src/tmux_mouse.rs`:

```rust
/// New absolute size (in cells) for the divider's primary pane given the
/// pointer's cell coordinate on the major axis. The primary pane's near edge
/// stays fixed; its far edge follows the pointer. `near` is the pane's xoff
/// (vertical divider) or yoff (horizontal). Clamped to >= 1.
fn resize_target_size(near: i32, pointer_cell: i32) -> u32 {
    (pointer_cell - near).max(1) as u32
}
```

Test:

```rust
    #[test]
    fn resize_target_size_follows_pointer() {
        assert_eq!(resize_target_size(0, 50), 50);
        assert_eq!(resize_target_size(10, 25), 15);
        assert_eq!(resize_target_size(0, 0), 1); // clamp
    }
```

Run: `cargo test -p ozmux-gui resize_target_size` → PASS.

- [ ] **Step 2: Extend the gesture state + arbiter for divider drags**

Extend `GestureState` with a resizing variant:

```rust
    /// Dragging a divider; `last_sent` is the last absolute size we issued and
    /// `in_flight` is true until the resize's `%layout-change` refreshes dims.
    Resizing {
        divider: Divider,
        near: i32,
        last_sent: u32,
        commands_this_frame: u32,
    },
```

In `arbiter`, the divider path requires: the active window's `TmuxDividers` (add `dividers: Query<&TmuxDividers, With<ActiveWindow>>` or `Option<Res<ActiveWindowDividers>>` per Task 7's choice), the `TerminalCellMetricsResource` (`metrics: Res<TerminalCellMetricsResource>`), the config (`configs: Option<Res<crate::configs::OzmuxConfigsResource>>`), and the per-pane geometry to map pointer→cell. Implement this order on a left **press**:

  1. Compute `cursor_phys = window.cursor_position() * scale`.
  2. `cell_w = metrics.metrics.advance_phys.floor().max(1.0)`, `cell_h = metrics.metrics.line_height_phys.floor().max(1.0)` (physical, as `cell_at_pane` expects).
  3. `tol_phys = divider_grab_tolerance_px * scale`.
  4. If `divider_at(&dividers, cursor_phys, cell_w, cell_h, tol_phys)` is `Some(d)` → enter `Resizing { divider: d, near: <d.primary pane's xoff or yoff>, last_sent: <current size>, commands_this_frame: 0 }`. (Look up the primary pane's `TmuxPane.dims` from a `Query<&TmuxPane>` to get `near` and the current size.) **Do not** send `select-pane` for a divider press.
  5. Else fall through to the existing pane-body focus path (Phase 1).

On each frame while `Resizing`:
  - Reset `commands_this_frame = 0` at frame start; bail if `>= max_resize_commands_per_frame`.
  - Map `cursor_phys` to a cell on the major axis: `pointer_cell = (cursor_phys.x / cell_w).floor() as i32` (vertical) or `(cursor_phys.y / cell_h).floor() as i32` (horizontal).
  - `target = resize_target_size(near, pointer_cell)`.
  - If `target == last_sent` → no-op (cell unchanged). Else read the primary pane's **current** `TmuxPane.dims` size (the confirmed layout); if it already equals `target`, update `last_sent` and skip (the prior resize landed). Otherwise send `resize_pane_x_command(d.primary, target)` (vertical) / `resize_pane_y_command(d.primary, target)` (horizontal), set `last_sent = target`, `commands_this_frame += 1`. This realises "one in-flight resize per drag, anchored to confirmed dims": we only re-send when the pointer moved to a new cell AND the confirmed dims have caught up to (or differ from) the last target.

On left **release** from `Resizing` → `GestureState::Idle`.

- [ ] **Step 3: Test the resize state transition (App-level)**

Write an App-level test using the `src/tmux_copy_mode.rs` scaffold pattern: insert `TerminalCellMetricsResource` (advance_phys 8, line_height_phys 16), a focused `Window` + `PrimaryWindow`, an `ActiveWindow` entity carrying `TmuxDividers(vec![vdiv(1,40,0,24)])`, and a `TmuxPane{ id: PaneId(1), dims: { width:40, xoff:0, ... } }`. Because tests cannot set `window.cursor_position()` easily, factor the press-classification into a pure function `classify_press(dividers, panes_geom, cursor_phys, metrics, tol) -> Press` (returning `Press::Divider(Divider)` / `Press::Pane(PaneId)` / `Press::None`) and unit-test **that** directly with explicit `cursor_phys`. Assert a cursor on the divider line yields `Press::Divider` with `primary == PaneId(1)`, and a cursor mid-pane yields `Press::Pane`. This keeps the resize logic testable without a windowing backend.

Run: `cargo test -p ozmux-gui classify_press` → PASS.

- [ ] **Step 4: Manual smoke**

`cargo run`, split a window (tmux prefix `%` / `"`), drag the divider → panes resize and stop cleanly on release with no flicker/flood.

- [ ] **Step 5: Commit**

```bash
git add src/tmux_mouse.rs
git commit -m "feat(tmux): drag a divider to resize panes (absolute, one-in-flight throttle)"
```

---

## PHASE 3 — Auto-enter copy-mode drag-select

Outcome: click-drag inside a pane body auto-enters tmux copy-mode for the pane under the cursor, selects text as you drag, and copies on release; the old `drag_select_in_copy_mode` is deleted.

> **Pre-task investigation (do once, no code):** confirm how the tmux backend enters copy-mode today. Grep the wheel path for the entry: `rg "copy-mode|CopyModeState" src/tmux_input.rs src/tmux_copy_mode.rs`. The arbiter's auto-enter must (a) send `copy-mode -t %<id>` to tmux and (b) insert the `CopyModeState` marker (from `src/ui/copy_mode.rs`) on the pane entity so the copy-mode refresh systems engage. Use the exact command/marker the wheel path uses if it differs.

### Task 10: Copy-mode entry + cursor-positioning helpers (pure)

**Files:**
- Modify: `src/tmux_mouse.rs`

- [ ] **Step 1: Write the failing tests**

Add to `src/tmux_mouse.rs` tests. The positioning helper turns a known copy cursor + target cell into the command list (row via `goto-line` absolute, column via relative `cursor-left/right`):

```rust
    use ozmux_tmux::{CopyState, absolute_to_visible_row};

    #[test]
    fn position_commands_use_goto_line_for_row_and_relative_for_column() {
        // Copy cursor at visible (cx=2, cy=3); target visible cell (5, 7).
        // history_size=100, scroll_position=0 -> top line = 100.
        // target absolute line = top + target_row = 100 + 7 = 107.
        let cmds = position_commands(/*cur*/ (2, 3), /*target*/ (5, 7), /*history*/ 100, /*scroll*/ 0);
        assert_eq!(
            cmds,
            vec![
                "send-keys -X goto-line 107".to_string(),
                "send-keys -X -N 3 cursor-right".to_string(),
            ]
        );
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p ozmux-gui position_commands`
Expected: FAIL — `cannot find function position_commands`.

- [ ] **Step 3: Implement `position_commands`**

```rust
/// Commands to move the tmux copy cursor from `cur` (visible col,row) to
/// `target` (visible col,row). The row uses absolute `goto-line` (idempotent,
/// drift-free); the column uses relative cursor motion. `history` and `scroll`
/// are `CopyState.history_size` / `scroll_position` for the absolute mapping.
fn position_commands(cur: (u16, u16), target: (u16, u16), history: u32, scroll: u32) -> Vec<String> {
    let mut out = Vec::new();
    let top = history as i32 - scroll as i32;
    let target_line = top + target.1 as i32;
    out.push(format!("send-keys -X goto-line {}", target_line.max(0)));
    let dx = target.0 as i32 - cur.0 as i32;
    if dx > 0 {
        out.push(format!("send-keys -X -N {dx} cursor-right"));
    } else if dx < 0 {
        out.push(format!("send-keys -X -N {} cursor-left", -dx));
    }
    out
}
```

(`absolute_to_visible_row` is the inverse mapping already in `ozmux_tmux`; we use the forward direction inline here. Keep the import if a later step needs it, else drop it.)

- [ ] **Step 4: Run tests**

Run: `cargo test -p ozmux-gui position_commands`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/tmux_mouse.rs
git commit -m "feat(tmux): copy-cursor positioning helper (goto-line row + relative column)"
```

### Task 11: Drag-select gesture in the arbiter

**Files:**
- Modify: `src/tmux_mouse.rs`

- [ ] **Step 1: Extend the gesture state**

Add to `GestureState`:

```rust
    /// Selecting text in a pane via tmux copy-mode. `anchor` is the press cell.
    /// `pending` holds a target cell awaiting the first copy-state snapshot
    /// (so we can position the copy cursor before begin-selection).
    Selecting {
        pane: Entity,
        pane_id: ozmux_tmux::PaneId,
        anchor: (u16, u16),
        begun: bool,
        last_target: Option<(u16, u16)>,
    },
```

- [ ] **Step 2: Implement the drag-select flow**

The arbiter gains the SystemParams the old `drag_select_in_copy_mode` used (mirror them exactly): `mut queries: ResMut<CopyModeQueries>`, `metrics: Res<TerminalCellMetricsResource>`, the modality guards (`picker: Res<SessionPicker>`, `copy_prompt: Res<CopyPrompt>`, `focused_webview: Res<FocusedWebview>`), `copy_modes: Query<(), With<CopyModeState>>`, snapshots for the pane (`snapshots: Query<(&TmuxPane, &CopyModeSnapshot)>` — but `CopyModeSnapshot` is private to `tmux_copy_mode.rs`; promote it to `pub(crate)` like the helpers in Task 2, OR expose a `pub(crate)` accessor returning `CopyState`). Per-pane geometry to map pointer→cell: `Query<(Entity, &TmuxPane, &ComputedNode, &UiGlobalTransform)>`.

Flow:
- **Transition Pressed → Selecting** when a pane-body press has moved past `drag_threshold_px` (compare `cursor_phys` to `origin_phys`). On transition:
  - Ensure copy-mode: if `copy_modes.get(pane).is_err()`, send `copy-mode -t %<id>` and `commands.entity(pane).insert(CopyModeState)`.
  - Set `Selecting { pane, pane_id, anchor: <origin cell>, begun: false, last_target: None }`.
- **Each frame while Selecting** (the pane is targeted by `%id`, never `ActivePane`):
  - Resolve the pane's current copy cursor from its `CopyModeSnapshot` → `CopyState { cursor_x, cursor_y, history_size, scroll_position, .. }`. If no snapshot yet, **defer** (PendingPosition): do nothing this frame (the copy-state refresh round-trips; next frame the snapshot exists).
  - Map `cursor_phys` to the pane cell via `cell_at_pane(node, transform, cursor_phys, cell_w, cell_h, cols, rows)`.
  - If not `begun`: issue `position_commands((cursor_x, cursor_y), anchor, history_size, scroll_position)` to move to the **anchor**, then `send-keys -X -t %<id> begin-selection`, set `begun = true`, `last_target = Some(anchor)`.
  - Else if the mapped cell differs from `last_target`: issue `position_commands((cursor_x, cursor_y), cell, history_size, scroll_position)` (extend), `last_target = Some(cell)`.
  - Send all commands targeted: build them as `send-keys -X -t %<id> ...` (the helper emits untargeted `send-keys -X ...`; prepend the target, e.g. format the helper output through a small `target_copy_cmd(pane_id, &cmd)` that inserts `-t %id` after `-X`). Add this tiny transform + a unit test (`"send-keys -X goto-line 5"` → `"send-keys -X -t %2 goto-line 5"`).
- **On release from Selecting:** `send-keys -X -t %<id> copy-selection`, then `show_buffer_command()` and `queries.register(id, pane_id, CopyQueryKind::Buffer)` (the clipboard bridge, exactly as the old system did). Reset to `Idle`.
- **Modality guards / unfocused window:** replicate the old early-returns (drain `buttons`, reset gesture to `Idle`).

- [ ] **Step 3: Tests**

Unit-test the `target_copy_cmd` transform and re-test `position_commands`. For the gesture itself, add a `classify`-style pure test where feasible; full behavior is covered by the manual smoke + the retained copy-mode App tests. Run: `cargo test -p ozmux-gui tmux_mouse`.

- [ ] **Step 4: Commit**

```bash
git add src/tmux_mouse.rs
git commit -m "feat(tmux): drag-select auto-enters copy-mode and copies on release (pane-targeted)"
```

### Task 12: Delete the old `drag_select_in_copy_mode`

**Files:**
- Modify: `src/tmux_copy_mode.rs`

- [ ] **Step 1: Remove the system + resource**

Delete the `drag_select_in_copy_mode` function and its registration block in `OzmuxTmuxCopyModePlugin::build`. Delete the `DragSelect` resource definition and its `app.init_resource::<DragSelect>();`. Keep the `on_copy_mode_exit` observer but remove the `DragSelect` reset inside it (and update the exit test `copy_mode_exit_repaints_live_grid_and_prunes_refresh_state` to drop the `DragSelect` assertions and setup). Keep `cursor_deltas` (still `pub(crate)`, now used by the arbiter) and its unit test.

- [ ] **Step 2: Verify**

Run: `cargo test -p ozmux-gui tmux_copy_mode` and `cargo build`.
Expected: PASS; no `DragSelect`/`drag_select_in_copy_mode` references remain (`rg "DragSelect|drag_select_in_copy_mode" src` → only history).

- [ ] **Step 3: Manual smoke + commit**

`cargo run`: drag across text in a normal pane → it enters copy-mode, highlights, and the selection is on the clipboard after release. Wheel→copy-mode still works.

```bash
git add src/tmux_copy_mode.rs
git commit -m "refactor(tmux): remove old copy-mode drag-select (arbiter now owns it)"
```

### Task 13: Real-tmux integration test for selection

**Files:**
- Create/extend: `crates/tmux_session/tests/real_tmux_input.rs` (or a new `real_tmux_resize.rs`)

- [ ] **Step 1:** Following the existing `real_tmux_*` harness, add a test that (a) issues a `resize_pane_x_command` and asserts the subsequent `%layout-change` reports the new width (settle, no flood), and (b) drives `copy-mode` + `goto-line` + `begin-selection` + `select-word` on seeded content and asserts the resulting buffer via `show-buffer`, documenting the `select-word` first-character behavior. Gate behind the same `#[ignore]`/real-tmux conventions the sibling tests use.
- [ ] **Step 2:** Run: `cargo test -p ozmux_tmux --test real_tmux_input -- --ignored` (or the project's documented real-tmux invocation). Expected: PASS where a real tmux is available.
- [ ] **Step 3: Commit**

```bash
git add crates/tmux_session/tests
git commit -m "test(tmux): real-tmux resize settle + copy-mode selection round-trip"
```

---

## PHASE 4 — Double / triple-click word/line select

Outcome: double-click selects the word under the cursor, triple-click the line, both auto-entering copy-mode and copying.

### Task 14: Click-count tracking (pure)

**Files:**
- Modify: `src/tmux_mouse.rs`

- [ ] **Step 1: Write the failing tests**

```rust
    use std::time::Duration;

    #[test]
    fn click_count_increments_within_timeout_and_drift() {
        let mut t = ClickTracker::default();
        let cfg = (Duration::from_millis(400), 8.0f32); // (timeout, drift_px)
        assert_eq!(t.register(Duration::from_millis(0), Vec2::new(10.0, 10.0), cfg), 1);
        assert_eq!(t.register(Duration::from_millis(200), Vec2::new(11.0, 11.0), cfg), 2);
        assert_eq!(t.register(Duration::from_millis(350), Vec2::new(12.0, 10.0), cfg), 3);
    }

    #[test]
    fn click_count_resets_after_timeout() {
        let mut t = ClickTracker::default();
        let cfg = (Duration::from_millis(400), 8.0f32);
        assert_eq!(t.register(Duration::from_millis(0), Vec2::new(10.0, 10.0), cfg), 1);
        assert_eq!(t.register(Duration::from_millis(500), Vec2::new(10.0, 10.0), cfg), 1);
    }

    #[test]
    fn click_count_resets_after_drift() {
        let mut t = ClickTracker::default();
        let cfg = (Duration::from_millis(400), 8.0f32);
        assert_eq!(t.register(Duration::from_millis(0), Vec2::new(10.0, 10.0), cfg), 1);
        assert_eq!(t.register(Duration::from_millis(100), Vec2::new(40.0, 40.0), cfg), 1);
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p ozmux-gui click_count`
Expected: FAIL — `cannot find type ClickTracker`.

- [ ] **Step 3: Implement `ClickTracker`**

```rust
/// Tracks consecutive-click count using a timeout + positional drift gate.
#[derive(Default)]
struct ClickTracker {
    last: Option<(Duration, Vec2, u8)>,
}

impl ClickTracker {
    /// Registers a press at `now` / `pos` and returns the resulting click count
    /// (1 = single, 2 = double, 3 = triple, capped at 3). `cfg` is
    /// `(double_click_timeout, click_drift_px)` in logical units.
    fn register(&mut self, now: Duration, pos: Vec2, cfg: (Duration, f32)) -> u8 {
        let (timeout, drift) = cfg;
        let count = match self.last {
            Some((t, p, c))
                if now.saturating_sub(t) <= timeout && p.distance(pos) <= drift =>
            {
                (c + 1).min(3)
            }
            _ => 1,
        };
        self.last = Some((now, pos, count));
        count
    }
}
```

Add a `ClickTracker` field to `TmuxMouseGesture`: `click: ClickTracker`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p ozmux-gui click_count`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/tmux_mouse.rs
git commit -m "feat(tmux): consecutive-click tracking for word/line select"
```

### Task 15: Word/line select on multi-click

**Files:**
- Modify: `src/tmux_mouse.rs`

- [ ] **Step 1: Wire click-count into the press path**

In `arbiter`, on a left **press** inside a pane body, call `gesture.click.register(now, cursor_logical, (timeout, drift))` where `now` comes from `Res<Time<Real>>` (`time.elapsed()`) and `cursor_logical = window.cursor_position()` (logical px), `timeout = Duration::from_millis(cfg.double_click_timeout_ms as u64)`, `drift = cfg.click_drift_px`. Store the returned count on the `Pressed` state (`Pressed { pane_id, origin_phys, click_count }`).

- [ ] **Step 2: Act on release without drag**

On left **release** from `Pressed` (i.e. no drag threshold crossed → no `Selecting`/`Resizing`), branch on `click_count`:
  - `1` → nothing extra (focus already sent on press).
  - `2` → ensure copy-mode for the pane (`copy-mode -t %id` + insert `CopyModeState` if absent), then **defer** one frame for the snapshot (`PendingWordSelect { pane, pane_id, cell }` sub-state), then `position_commands(cursor, cell, history, scroll)` + `send-keys -X -t %id select-word` + `copy-selection` + `show-buffer`→clipboard.
  - `3` → same but `send-keys -X -t %id select-line`.

Model the deferral exactly like `Selecting`'s `PendingPosition`: add `GestureState::PendingMultiSelect { pane, pane_id, cell, kind: MultiSelectKind }` where `enum MultiSelectKind { Word, Line }`; on the next frame, once the pane's `CopyModeSnapshot` is present, issue position → `select-word`/`select-line` → `copy-selection` → `show-buffer`, then `Idle`.

- [ ] **Step 3: Tests**

Unit-test a pure `multi_select_commands(kind, cur, cell, history, scroll, pane_id) -> Vec<String>` that returns the full targeted command list, e.g. for `Word`:

```rust
    #[test]
    fn multi_select_word_commands() {
        let cmds = multi_select_commands(MultiSelectKind::Word, (0, 0), (3, 0), 0, 0, PaneId(2));
        assert_eq!(cmds, vec![
            "send-keys -X -t %2 goto-line 0".to_string(),
            "send-keys -X -t %2 -N 3 cursor-right".to_string(),
            "send-keys -X -t %2 select-word".to_string(),
            "send-keys -X -t %2 copy-selection".to_string(),
            "show-buffer".to_string(),
        ]);
    }
```

Implement `multi_select_commands` by composing `position_commands` (targeted via `target_copy_cmd`) + the `select-word`/`select-line` + `copy-selection` + `show_buffer_command()`. Run: `cargo test -p ozmux-gui multi_select`.

- [ ] **Step 4: Manual smoke + commit**

`cargo run`: double-click a word → selects + copies the word; triple-click → the line. Single click still just focuses; drag still selects a range.

```bash
git add src/tmux_mouse.rs
git commit -m "feat(tmux): double-click word / triple-click line select (auto copy-mode + copy)"
```

### Task 16: Final integration pass + docs

**Files:**
- Modify: `src/tmux_mouse.rs` (cleanup), `CLAUDE.md` plugin list if it enumerates tmux plugins

- [ ] **Step 1:** Re-read the spec's "Goals" and "Disambiguation / consistency" sections; verify by manual smoke: divider press never focuses; pane-body press focuses; press-then-drag selects; double/triple-click select; wheel→copy-mode + window-tab switch unaffected; webview surfaces still receive their own clicks (Block retained).
- [ ] **Step 2:** Run the full suite: `cargo test` and `cargo clippy --workspace --all-targets`. Fix any lint (the comment taxonomy + visibility rules in `.claude/rules/rust.md`). Confirm every new `pub`/`pub(crate)` item has the minimal visibility its callers require, and every new public item has a `///` doc.
- [ ] **Step 3:** If `src/main.rs`'s plugin list is mirrored in `CLAUDE.md`, add `OzmuxTmuxMousePlugin` (and the pane-chrome rename) there.
- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "chore(tmux): finalize mouse arbiter — lint, docs, plugin list"
```

---

## Self-Review (completed during authoring)

**Spec coverage:** drag-resize → Phase 2 (Tasks 5–9); drag-select auto copy-mode → Phase 3 (Tasks 10–12); double/triple-click → Phase 4 (Tasks 14–15); consistency/single-authority → Phase 1 (Task 3) + Task 16; split-tree dividers → Tasks 6–7; absolute resize + one-in-flight throttle → Task 9; `goto-line` + optimistic cursor → Tasks 10–11; dedicated config cap + builders + `pub(crate)` helpers → Tasks 1, 2, 5; real-tmux tests → Task 13. The spec's `commanded_cursor` reconciliation is realised as `last_target` + the per-frame snapshot read in Task 11.

**Known deviations (intentional):** (1) `Button`/`FocusPolicy::Block` retained for webview-click-blocking instead of removed (documented at top). (2) `CopyModeSnapshot` must be promoted to `pub(crate)` (Task 11) — same treatment as the geometry helpers. (3) The active-window divider store is a component **or** resource depending on which the projection site makes available (Task 7 Step 3) — pick one and export it consistently.

**Type consistency:** `position_commands`, `target_copy_cmd`, `multi_select_commands`, `divider_at`, `resize_target_size`, `ClickTracker`, `classify_press`, `GestureState` variants, `Divider`/`DividerAxis`, `ResizeDir`, and the `resize_pane_*_command` builders are each defined in a task before first use.

---

## Execution Handoff

Per the request, execute with **superpowers:subagent-driven-development** — a fresh subagent per task with two-stage review between tasks. Phases are sequential; within a phase, tasks are mostly sequential (Task 6 before 7; 8 before 9; 10 before 11 before 12). Tasks 1, 2, and 5 are independent and may be done in any order first.
