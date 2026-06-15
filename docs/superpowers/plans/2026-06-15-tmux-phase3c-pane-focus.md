# tmux Phase 3c — Pane Click-to-Focus + Dim Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Click a tmux pane to focus it (send `select-pane -t %<id>`, command-echo) and dim every pane except the active one with a Bevy UI overlay.

**Architecture:** A new `select_pane_command` builder; a binary-side `OzmuxTmuxPaneFocusPlugin` that (a) augments each rendered `TmuxPane` node once with a `Button` (click target) plus a `FocusPolicy::Pass` dim-overlay child stored on the pane as `PaneDim(Entity)`, (b) on a pane `Interaction::Pressed` sends `select-pane`, (c) on `ProjectionModel` change shows the overlay on every pane except `active_pane` (and dims nothing when `active_pane` is `None`). All state flows command-echo: tmux's `%window-pane-changed` updates `ProjectionModel.active_pane`.

**Tech Stack:** Rust (edition 2024), Bevy 0.18 ECS/UI (`bevy_ui` `Interaction`/`FocusPolicy`/`Visibility`), tmux `-CC`. Crates: `ozmux_tmux` (`crates/tmux_session`), the binary (`src/`).

**Spec:** `docs/superpowers/specs/2026-06-15-tmux-phase3c-pane-focus-design.md`

**Conventions (enforced):** no `mod.rs`; comments only `// TODO:`/`// NOTE:`/`// SAFETY:` (English); `//!` on new module files, `///` on `pub` items; contiguous imports; mutable params first; private items last; whole-system change guards via `run_if`. `docs/` is gitignored — commit with `git add -f`. Commit messages end with `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. The `ozmux-gui` binary test suite segfaults under parallel CEF threads — run gui tests with `-- --test-threads=1` or filtered.

**Verified facts (spec-review, bevy_ui 0.18.1 source):** a `Node` with NO `FocusPolicy` component is treated as `Block` (`focus.rs:324`), so the pane is a click target by default and the overlay MUST carry an explicit `FocusPolicy::Pass`. The repo's interactive nodes use `Button` (which requires `Interaction`). A child overlay's UI stack index is above the parent `MaterialNode`, so a semi-transparent `BackgroundColor` child composites above and dims the terminal. `layout_tmux_panes` only writes `Node` rect fields and never removes components/children.

---

## File Structure

| File | Responsibility | Task |
| --- | --- | --- |
| `crates/tmux_session/src/enumerate.rs` + `lib.rs` | `select_pane_command` builder | T1 |
| `src/theme.rs` | `PANE_DIM_OVERLAY` color constant | T2 |
| `src/ui/tmux_pane_focus.rs` (new) | `PaneDim` component, `augment_tmux_pane`, `sync_pane_dim`, `focus_pane_on_click`, `OzmuxTmuxPaneFocusPlugin` | T2–T4 |
| `src/ui.rs` | `mod tmux_pane_focus;` | T2 |
| `src/main.rs` | register `OzmuxTmuxPaneFocusPlugin` | T2 |
| `crates/tmux_session/tests/real_tmux_pane.rs` (new) | gated integration | T5 |

---

## Task 1: `select_pane_command` builder

**Files:** Modify `crates/tmux_session/src/enumerate.rs`, `crates/tmux_session/src/lib.rs`.

- [ ] **Step 1: Failing test** (in `enumerate.rs` `#[cfg(test)] mod tests`):

```rust
    #[test]
    fn select_pane_command_targets_at_id() {
        assert_eq!(select_pane_command(PaneId(3)), "select-pane -t %3");
    }
```

- [ ] **Step 2: Run, expect FAIL**

Run: `cargo test -p ozmux_tmux select_pane_command_targets_at_id`
Expected: FAIL — function not defined.

- [ ] **Step 3: Implement** — in `enumerate.rs`, next to `select_window_command`, among the `pub fn` builders:

```rust
/// Builds `select-pane -t %<id>` to focus a pane.
pub fn select_pane_command(id: PaneId) -> String {
    format!("select-pane -t %{}", id.0)
}
```

Confirm `PaneId` (`tmux_control_parser::PaneId(pub u32)`) is imported at the top of `enumerate.rs`; if not, add it to the existing `use` block. In `crates/tmux_session/src/lib.rs`, add `select_pane_command` to the `pub use enumerate::{ ... }` re-export (keep contiguous/sorted).

- [ ] **Step 4: Run, expect PASS**

Run: `cargo test -p ozmux_tmux select_pane_command_targets_at_id`
Expected: PASS. Then `cargo test -p ozmux_tmux 2>&1 | tail -3` (all pass).

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add crates/tmux_session/src/{enumerate.rs,lib.rs}
git commit -m "$(printf 'feat(tmux): add select_pane_command builder\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Task 2: Pane augmentation (Button + dim overlay) + plugin

**Files:** Create `src/ui/tmux_pane_focus.rs`; modify `src/theme.rs`, `src/ui.rs`, `src/main.rs`.

**Read first:** `src/tmux_render.rs` `attach_tmux_pane_terminal` (how panes get `TerminalHandle` + `Node`); `src/ui/tmux_window_bar.rs` (a plugin that spawns UI + registers systems; `Button` usage); `src/theme.rs` (constant style).

- [ ] **Step 1: Add the dim color** — in `src/theme.rs`, near the other `Color` constants:

```rust
/// Semi-transparent overlay drawn over inactive tmux panes to dim them.
pub const PANE_DIM_OVERLAY: Color = Color::srgba(0.0, 0.0, 0.0, 0.35);
```

- [ ] **Step 2: Create the module with the component, augment system, plugin, and a failing test**

`src/ui/tmux_pane_focus.rs`:

```rust
//! Pane click-to-focus + dim: augments each tmux pane node with a `Button`
//! (click target) and a `FocusPolicy::Pass` dim overlay, sends `select-pane`
//! on click, and shows the overlay on every pane except the active one.

use crate::theme;
use bevy::prelude::*;
use bevy::ui::FocusPolicy;
use ozma_tty_engine::TerminalHandle;
use ozmux_tmux::TmuxPane;

/// Points a pane at its dim-overlay child entity (O(1) lookup in `sync_pane_dim`).
#[derive(Component)]
pub(crate) struct PaneDim(pub(crate) Entity);

/// Registers pane click-to-focus and dim systems.
pub struct OzmuxTmuxPaneFocusPlugin;

impl Plugin for OzmuxTmuxPaneFocusPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, augment_tmux_pane);
    }
}

/// Gives each rendered pane (one that has its `TerminalHandle` but no `Button`
/// yet) a `Button` click target and a hidden `FocusPolicy::Pass` dim overlay
/// child, recorded on the pane as `PaneDim`. The `Without<Button>` filter makes
/// this run exactly once per pane.
fn augment_tmux_pane(
    mut commands: Commands,
    panes: Query<Entity, (With<TmuxPane>, With<TerminalHandle>, Without<Button>)>,
) {
    for pane in panes.iter() {
        let overlay = commands
            .spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    right: Val::Px(0.0),
                    top: Val::Px(0.0),
                    bottom: Val::Px(0.0),
                    ..default()
                },
                BackgroundColor(theme::PANE_DIM_OVERLAY),
                FocusPolicy::Pass,
                Visibility::Hidden,
                ChildOf(pane),
            ))
            .id();
        commands.entity(pane).insert((Button, PaneDim(overlay)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ozmux_tmux::TmuxPane;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use tmux_control_parser::{CellDims, PaneId};

    fn dims() -> CellDims {
        CellDims { width: 10, height: 5, xoff: 0, yoff: 0 }
    }

    #[test]
    fn augment_adds_button_and_hidden_overlay() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, OzmuxTmuxPaneFocusPlugin));
        let pane = app
            .world_mut()
            .spawn((
                TmuxPane { id: PaneId(1), dims: dims() },
                TerminalHandle::detached(10, 5, Arc::new(AtomicBool::new(false))),
            ))
            .id();
        app.update();

        assert!(app.world().get::<Button>(pane).is_some(), "pane gains a Button");
        let pane_dim = app.world().get::<PaneDim>(pane).expect("PaneDim recorded");
        let overlay = pane_dim.0;
        assert_eq!(
            app.world().get::<Visibility>(overlay).copied(),
            Some(Visibility::Hidden),
            "overlay starts hidden",
        );
        assert_eq!(
            app.world().get::<FocusPolicy>(overlay).copied(),
            Some(FocusPolicy::Pass),
            "overlay passes clicks through to the pane",
        );

        // Idempotent: a second update does not add a second overlay.
        app.update();
        let children = app.world().get::<Children>(pane).map(|c| c.len()).unwrap_or(0);
        assert_eq!(children, 1, "augment runs exactly once per pane");
    }
}
```

In `src/ui.rs`, add `mod tmux_pane_focus;` with the other `mod` declarations (private). In `src/main.rs`, add `OzmuxTmuxPaneFocusPlugin` to the plugin list near the other tmux plugins (`OzmuxTmuxRenderPlugin`/`OzmuxTmuxInputPlugin`/`OzmuxTmuxWindowBarPlugin`); import via `use crate::ui::tmux_pane_focus::OzmuxTmuxPaneFocusPlugin;` (or the path your module tree exposes).

- [ ] **Step 3: Run, expect FAIL then PASS**

Run: `cargo test -p ozmux-gui augment_adds_button_and_hidden_overlay -- --test-threads=1`
Iterate until PASS. (If `ChildOf` / `FocusPolicy` / `Visibility` import paths differ in this Bevy version, fix the `use`; `FocusPolicy` is `bevy::ui::FocusPolicy`, `ChildOf`/`Visibility`/`Button` are in `bevy::prelude`.)

- [ ] **Step 4: Build + clippy + commit**

```bash
cargo build 2>&1 | tail -3
cargo clippy -p ozmux-gui --all-targets 2>&1 | tail -4
cargo fmt
git add src/ui/tmux_pane_focus.rs src/theme.rs src/ui.rs src/main.rs
git commit -m "$(printf 'feat(ui): augment tmux panes with a click target + dim overlay\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Task 3: `sync_pane_dim` — dim every pane except the active one

**Files:** Modify `src/ui/tmux_pane_focus.rs`.

**Context:** `ozmux_tmux::ProjectionModel.active_pane: Option<PaneId>` is the active pane. Show each pane's overlay (`Visibility::Visible`) when it is NOT active; hide it on the active pane; when `active_pane` is `None`, hide ALL overlays (dim nothing).

- [ ] **Step 1: Failing test** (add to the `tests` module in `tmux_pane_focus.rs`):

```rust
    #[test]
    fn sync_dims_inactive_and_clears_when_none() {
        use ozmux_tmux::ProjectionModel;

        let mut app = App::new();
        app.add_plugins((MinimalPlugins, OzmuxTmuxPaneFocusPlugin));
        let h = || TerminalHandle::detached(10, 5, Arc::new(AtomicBool::new(false)));
        let p1 = app.world_mut().spawn((TmuxPane { id: PaneId(1), dims: dims() }, h())).id();
        let p2 = app.world_mut().spawn((TmuxPane { id: PaneId(2), dims: dims() }, h())).id();
        app.update(); // augment both panes (spawns overlays)

        let overlay = |app: &App, pane| app.world().get::<PaneDim>(pane).unwrap().0;
        let vis = |app: &App, e| app.world().get::<Visibility>(e).copied().unwrap();

        // active_pane = Some(1): pane 1 overlay hidden, pane 2 overlay visible.
        app.insert_resource(ProjectionModel { active_pane: Some(PaneId(1)), ..default() });
        app.update();
        assert_eq!(vis(&app, overlay(&app, p1)), Visibility::Hidden);
        assert_eq!(vis(&app, overlay(&app, p2)), Visibility::Visible);

        // Flip to 2.
        app.world_mut().resource_mut::<ProjectionModel>().active_pane = Some(PaneId(2));
        app.update();
        assert_eq!(vis(&app, overlay(&app, p1)), Visibility::Visible);
        assert_eq!(vis(&app, overlay(&app, p2)), Visibility::Hidden);

        // None → dim nothing.
        app.world_mut().resource_mut::<ProjectionModel>().active_pane = None;
        app.update();
        assert_eq!(vis(&app, overlay(&app, p1)), Visibility::Hidden);
        assert_eq!(vis(&app, overlay(&app, p2)), Visibility::Hidden);
    }
```

- [ ] **Step 2: Run, expect FAIL**

Run: `cargo test -p ozmux-gui sync_dims_inactive_and_clears_when_none -- --test-threads=1`
Expected: FAIL — `sync_pane_dim` not registered.

- [ ] **Step 3: Implement** — add the system and register it (gated on projection change). In `tmux_pane_focus.rs`:

```rust
fn sync_pane_dim(
    mut overlays: Query<&mut Visibility>,
    panes: Query<(&TmuxPane, &PaneDim)>,
    model: Res<ozmux_tmux::ProjectionModel>,
) {
    for (pane, dim) in panes.iter() {
        let active = model.active_pane == Some(pane.id) || model.active_pane.is_none();
        let want = if active { Visibility::Hidden } else { Visibility::Visible };
        // NOTE: the overlay may not be spawned yet on the frame a pane appears;
        // a `get_mut` miss is a no-op, never a panic.
        if let Ok(mut vis) = overlays.get_mut(dim.0) {
            vis.set_if_neq(want);
        }
    }
}
```

Register it in `OzmuxTmuxPaneFocusPlugin::build`, gated:

```rust
        app.add_systems(
            Update,
            (
                augment_tmux_pane,
                sync_pane_dim.run_if(resource_exists_and_changed::<ozmux_tmux::ProjectionModel>),
            ),
        );
```

Import `resource_exists_and_changed` from `bevy::prelude` (already covered by `use bevy::prelude::*;`). `set_if_neq` is a `DetectChangesMut` method on `Mut<Visibility>` (`Visibility` derives `PartialEq`); if it's not in scope, add `use bevy::prelude::DetectChangesMut;` or use `if *vis != want { *vis = want; }`.

NOTE on ordering: `sync_pane_dim` running the same frame `augment_tmux_pane` spawns an overlay is fine — the `get_mut` miss is tolerated and the next projection change re-syncs. To make the first sync deterministic in the test, the test calls `app.update()` once (augment) before inserting the `ProjectionModel`.

- [ ] **Step 4: Run, expect PASS**

Run: `cargo test -p ozmux-gui sync_dims_inactive_and_clears_when_none -- --test-threads=1`
Expected: PASS.

- [ ] **Step 5: Build + clippy + commit**

```bash
cargo build 2>&1 | tail -3
cargo clippy -p ozmux-gui --all-targets 2>&1 | tail -3
cargo fmt
git add src/ui/tmux_pane_focus.rs
git commit -m "$(printf 'feat(ui): dim inactive tmux panes from the active-pane projection\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Task 4: `focus_pane_on_click` — click a pane to `select-pane`

**Files:** Modify `src/ui/tmux_pane_focus.rs`.

**Read first:** `src/ui/tmux_window_bar_input.rs` `switch_window_on_click` (the click→command pattern: `Changed<Interaction>`, `Interaction::Pressed`, `connection.client()`, `handle().send`, `tracing::warn!`); `crate::input::InputPhase`.

- [ ] **Step 1: Failing test** (a pure mapping, since a live `TmuxConnection` can't be faked headlessly — the click wiring is covered by the gated test in T5). Add to the `tests` module:

```rust
    #[test]
    fn pane_press_maps_to_select_pane() {
        use ozmux_tmux::select_pane_command;
        assert_eq!(select_pane_command(PaneId(2)), "select-pane -t %2");
    }
```

- [ ] **Step 2: Run, expect PASS (mapping) / then implement the system**

Run: `cargo test -p ozmux-gui pane_press_maps_to_select_pane -- --test-threads=1` (passes once `select_pane_command` is imported).

- [ ] **Step 3: Implement** — add the system, importing what it needs at the top of `tmux_pane_focus.rs` (`use crate::input::InputPhase;`, `use ozmux_tmux::{TmuxConnection, select_pane_command};` — fold into the existing `ozmux_tmux` import):

```rust
fn focus_pane_on_click(
    panes: Query<(&Interaction, &TmuxPane), Changed<Interaction>>,
    connection: NonSend<TmuxConnection>,
) {
    let Some(client) = connection.client() else {
        return;
    };
    for (interaction, pane) in panes.iter() {
        if *interaction != Interaction::Pressed {
            continue;
        }
        let cmd = select_pane_command(pane.id);
        if let Err(e) = client.handle().send(&cmd) {
            tracing::warn!(?e, pane = pane.id.0, "select-pane send failed");
        }
    }
}
```

Register it in `InputPhase::Dispatch` (so the focus change is same-frame, mirroring `switch_window_on_click`):

```rust
        app.add_systems(
            Update,
            (
                augment_tmux_pane,
                focus_pane_on_click.in_set(crate::input::InputPhase::Dispatch),
                sync_pane_dim.run_if(resource_exists_and_changed::<ozmux_tmux::ProjectionModel>),
            ),
        );
```

The headless tests insert no `TmuxConnection`; `focus_pane_on_click` reads `NonSend<TmuxConnection>`, so the test App needs it. Add `app.insert_non_send_resource(ozmux_tmux::TmuxConnection::default());` to BOTH existing tests' setup (the augment + sync tests) so the system can be scheduled. Confirm `TmuxConnection` derives `Default` (it does — used the same way in the window-bar tests).

- [ ] **Step 4: Run all module tests**

Run: `cargo test -p ozmux-gui tmux_pane_focus -- --test-threads=1`
Expected: all pass (`augment_*`, `sync_*`, `pane_press_maps_*`).

- [ ] **Step 5: Build + clippy + commit**

```bash
cargo build 2>&1 | tail -3
cargo clippy -p ozmux-gui --all-targets 2>&1 | tail -3
cargo fmt
git add src/ui/tmux_pane_focus.rs
git commit -m "$(printf 'feat(ui): click a tmux pane to select-pane\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Task 5: Gated integration test + final verification

**Files:** Create `crates/tmux_session/tests/real_tmux_pane.rs`.

**Read first:** `crates/tmux_session/tests/real_tmux_window.rs` (the exact harness: `#[ignore = "requires a real tmux binary and a controlling PTY"]`, socket naming, `TmuxSessionPlugin`, `.set(client)`, deadline pump loops, the departure-then-round-trip pattern, `kill-server` teardown).

- [ ] **Step 1: Create `crates/tmux_session/tests/real_tmux_pane.rs`**

Mirror `real_tmux_window.rs`, but for panes. The test (`#[ignore = "requires a real tmux binary and a controlling PTY"]`):
1. Spawn `tmux -CC` (unique socket via `std::process::id()`), `App` + `TmuxSessionPlugin`, `.set(client)`.
2. Pump (5s deadline) until `ConnectionState::Attached` and `ProjectionModel.active_pane.is_some()`. Capture `first_active: PaneId` (it's `Copy`).
3. Send `split-window` via `connection.client().unwrap().handle().send("split-window")`. Pump until the active window has ≥2 panes — i.e. the projection's active window's `panes.len() >= 2` (find the active window in `ProjectionModel.windows`). tmux auto-focuses the new pane.
4. Pump (5s) until `ProjectionModel.active_pane != Some(first_active)` (the split moved focus); assert it departed.
5. Send `ozmux_tmux::select_pane_command(first_active)`; pump (5s) until `ProjectionModel.active_pane == Some(first_active)`; assert it flipped back (verify-live gate for `select-pane` + command-echo).
6. Teardown: `connection.take()` then `client.handle().send("kill-server").ok()`.
Module `//!` doc with the run command `cargo test -p ozmux_tmux --test real_tmux_pane -- --ignored`. Capture owned `Copy` values (not borrows) across `app.update()`.

- [ ] **Step 2: Full check**

```bash
cd /Users/taiga/workspace/ozmux/wt/tmux-phase3
cargo build 2>&1 | tail -3
cargo clippy --workspace --all-targets 2>&1 | tail -4
cargo fmt --check 2>&1 | tail -2
cargo test -p ozma_tty_engine -p ozmux_tmux -p ozmux_configs 2>&1 | tail -8
cargo test -p ozmux-gui -- --test-threads=1 2>&1 | grep "test result" | tail -2
cargo test -p ozmux_tmux --test real_tmux_pane 2>&1 | tail -5   # collected as 1 ignored
```
Expected: all green; the new gated test shows `1 ignored`.

- [ ] **Step 3: Manual GUI verification** (desktop; run from OUTSIDE the attached tmux session)

`cargo run`, pick a session, split a window (tmux `C-b %` / `C-b "`). Confirm: inactive panes are dimmed and the active one is not; clicking an inactive (dimmed) pane focuses it (it un-dims, the previously-active pane dims); the overlay tracks pane resizes (drag a tmux pane border via `C-b` resize and confirm the dim still fills each pane). Note any click that fails to focus (would indicate the overlay is stealing the click — the `FocusPolicy::Pass` guard).

- [ ] **Step 4: Commit any fixes**

```bash
git add -A && git commit -m "$(printf 'test(tmux): gated real-tmux pane-focus integration test\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')" || echo "nothing to commit"
```

---

## Out of scope (later phases)

- Mouse wheel/scroll forwarding and text selection into tmux panes.
- Active-pane border/highlight (chose dim-only).
- Removal of the old `ozmux_multiplexer` mouse path (`mouse_buttons.rs`) — Phase 5.
