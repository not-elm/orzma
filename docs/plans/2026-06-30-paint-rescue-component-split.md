# paint_rescue Component Split Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Split the two per-pane state machines in `src/mode/tmux/paint_rescue.rs` out of one combined `ReseedTracker` struct (held in a `HashMap<PaneId, _>` resource) into two independent Bevy Components on the pane entity.

**Architecture:** Define `StructuralReseedState` and `BlankRecoveryState` as `#[derive(Component, Default)]`, attach them once per pane via a dedicated `attach_rescue_state` system (the binary cannot `#[require]` the out-of-crate `TmuxPane`), then rewrite the decision functions and `rescue_unpainted_panes` to read the components by `&mut` directly. Drop the `PaneSeedTrackers` resource and the `prune_tracker_on_pane_removed` observer (Components despawn with the entity).

**Tech Stack:** Rust (edition 2024), Bevy 0.18 ECS.

## Global Constraints

- Pure internal refactor — externally observable behavior MUST NOT change (coverage, debounce timing, copy-mode freeze). Reference: `docs/specs/2026-06-30-paint-rescue-component-split-design.md`.
- Debounce constants unchanged: `RESEED_DEBOUNCE_FRAMES = 3`, `RESEED_INFLIGHT_TIMEOUT = 30`.
- Rust conventions (`.claude/rules/rust.md`): no `mod.rs`; non-doc comments only `// TODO:` / `// NOTE:` / `// SAFETY:`; all `use` at top in one contiguous block, no inline fully-qualified paths; minimize visibility (these items are module-private — no `pub`); `Plugin::build` is a single method chain; mutable params before immutable; private items declared after exported ones.
- Lint gate after each task: `cargo build`, `cargo clippy --workspace` (0 warnings), `cargo fmt --check`.
- Test gate after each task: `cargo test --bin ozmux paint_rescue -- --test-threads=1` (plus `cargo test -p ozma_tty_engine has_visible_content` stays green — unchanged but a smoke check).

---

### Task 1: Add the two state Components and the attach system

Introduce the new components and the system that attaches them, registered in the plugin. The old `ReseedTracker` / `PaneSeedTrackers` and `rescue_unpainted_panes` stay in place and unchanged this task, so the crate compiles and all existing tests stay green; the new components are attached but not yet read.

**Files:**
- Modify: `src/mode/tmux/paint_rescue.rs`

**Interfaces:**
- Produces:
  - `struct StructuralReseedState { unpainted_streak: u8, inflight_age: Option<u16> }` — `#[derive(Component, Default)]`
  - `struct BlankRecoveryState { streak: u8, recovery_seq: Option<u32>, settled: bool }` — `#[derive(Component, Default)]`
  - `fn attach_rescue_state(commands: Commands, panes: Query<Entity, (With<TmuxPane>, Without<StructuralReseedState>)>)` — inserts both components once per pane.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `src/mode/tmux/paint_rescue.rs`:

```rust
#[test]
fn attach_inserts_both_state_components_once() {
    use tmux_control_parser::CellDims;

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_systems(Update, attach_rescue_state);

    let pane = app
        .world_mut()
        .spawn(TmuxPane {
            id: PaneId(1),
            dims: CellDims {
                width: 4,
                height: 2,
                xoff: 0,
                yoff: 0,
            },
        })
        .id();

    app.update();

    assert!(
        app.world().get::<StructuralReseedState>(pane).is_some(),
        "attach inserts StructuralReseedState",
    );
    assert!(
        app.world().get::<BlankRecoveryState>(pane).is_some(),
        "attach inserts BlankRecoveryState",
    );

    // A second pass must not re-insert (Without<StructuralReseedState> filter).
    app.update();
    assert!(app.world().get::<StructuralReseedState>(pane).is_some());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --bin ozmux attach_inserts_both_state_components_once -- --test-threads=1`
Expected: FAIL — `cannot find type StructuralReseedState` / `cannot find function attach_rescue_state` (compile error).

- [ ] **Step 3: Add the two components**

Insert after the existing `RESEED_INFLIGHT_TIMEOUT` const (and before `ReseedTracker`) in `src/mode/tmux/paint_rescue.rs`:

```rust
/// Per-pane structural-reseed debounce: a streak before the first capture
/// request, then an in-flight age that re-requests on timeout until painted.
#[derive(Component, Default)]
struct StructuralReseedState {
    unpainted_streak: u8,
    inflight_age: Option<u16>,
}

/// Per-pane blank-recovery debounce: a blank-grid-vs-live-mirror episode keyed
/// on the grid seq, repainting from the mirror once the divergence persists.
#[derive(Component, Default)]
struct BlankRecoveryState {
    streak: u8,
    recovery_seq: Option<u32>,
    settled: bool,
}
```

- [ ] **Step 4: Add the attach system**

Add this system (place it near the other systems, after `rescue_unpainted_panes`):

```rust
/// Attaches the per-pane rescue state components once per pane. `TmuxPane` is
/// defined in `ozmux_tmux`, so the binary cannot `#[require]` these onto it; the
/// `Without<StructuralReseedState>` filter makes this run exactly once per pane
/// (both components are always inserted together).
fn attach_rescue_state(
    mut commands: Commands,
    panes: Query<Entity, (With<TmuxPane>, Without<StructuralReseedState>)>,
) {
    for entity in panes.iter() {
        commands
            .entity(entity)
            .insert((StructuralReseedState::default(), BlankRecoveryState::default()));
    }
}
```

- [ ] **Step 5: Register the attach system in the plugin**

In `PaintRescuePlugin::build`, add `attach_rescue_state` to the `Update` systems, chained before `rescue_unpainted_panes`. Replace the current single-system registration:

```rust
impl Plugin for PaintRescuePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PaneSeedTrackers>()
            .add_observer(prune_tracker_on_pane_removed)
            .add_observer(repaint_pane_from_mirror)
            .add_systems(
                Update,
                (attach_rescue_state, rescue_unpainted_panes)
                    .chain()
                    .after(TmuxProjectionSet)
                    .before(TmuxLayoutSet)
                    .in_set(super::TmuxActiveSet),
            );
    }
}
```

- [ ] **Step 6: Run the test to verify it passes**

Run: `cargo test --bin ozmux attach_inserts_both_state_components_once -- --test-threads=1`
Expected: PASS.

- [ ] **Step 7: Lint + full paint_rescue suite**

Run: `cargo clippy --workspace 2>&1 | tail -2` (expect 0 warnings) and `cargo fmt` then `cargo test --bin ozmux paint_rescue -- --test-threads=1`
Expected: clippy clean, all paint_rescue tests PASS (the new components are unused-by-system this task, but attached — no behavior change).

- [ ] **Step 8: Commit**

```bash
git add src/mode/tmux/paint_rescue.rs
git commit -m "refactor(tmux): add per-pane rescue state components + attach system"
```

---

### Task 2: Migrate the rescue path to the components and delete the resource

Rewrite the decision functions and `rescue_unpainted_panes` to read the new components, delete `ReseedTracker` / `PaneSeedTrackers` / `prune_tracker_on_pane_removed`, and update the tests. The crate compiles and all tests pass at the end.

**Files:**
- Modify: `src/mode/tmux/paint_rescue.rs`

**Interfaces:**
- Consumes (from Task 1): `StructuralReseedState`, `BlankRecoveryState`, `attach_rescue_state`.
- Produces:
  - `fn reseed_decision(state: &mut StructuralReseedState, needs_seed: bool) -> bool`
  - `fn evaluate_blank_recovery(state: &mut BlankRecoveryState, grid: &TerminalGrid, handle: &TerminalHandle) -> bool`
  - `fn rescue_unpainted_panes(commands: Commands, reseed: MessageWriter<RequestPaneReseed>, panes: Query<(Entity, &TmuxPane, &TerminalHandle, &TerminalGrid, &mut StructuralReseedState, &mut BlankRecoveryState), Without<CopyModeState>>)`

- [ ] **Step 1: Rewrite `reseed_decision` to take `&mut StructuralReseedState`**

Replace the existing `reseed_decision` fn body's signature and the `!needs_seed` branch (the per-field reset becomes a full reset; delete the `// NOTE:` footgun comment):

```rust
/// Advances a pane's structural-reseed debounce one frame and returns whether to
/// emit a reseed request now. A painted grid (`!needs_seed`) resets the state.
/// Otherwise it debounces `RESEED_DEBOUNCE_FRAMES` consecutive unpainted frames
/// before the first request, then suppresses while a request is in flight,
/// re-requesting every `RESEED_INFLIGHT_TIMEOUT` frames until the grid paints.
fn reseed_decision(state: &mut StructuralReseedState, needs_seed: bool) -> bool {
    if !needs_seed {
        *state = StructuralReseedState::default();
        return false;
    }
    match &mut state.inflight_age {
        Some(age) => {
            *age = age.saturating_add(1);
            if *age >= RESEED_INFLIGHT_TIMEOUT {
                *age = 0;
                true
            } else {
                false
            }
        }
        None => {
            state.unpainted_streak = state.unpainted_streak.saturating_add(1);
            if state.unpainted_streak >= RESEED_DEBOUNCE_FRAMES {
                state.inflight_age = Some(0);
                true
            } else {
                false
            }
        }
    }
}
```

- [ ] **Step 2: Rewrite `evaluate_blank_recovery` to take `&mut BlankRecoveryState`**

Replace its signature and field names (`blank_recovery_seq` → `recovery_seq`, `blank_streak` → `streak`, `blank_recovery_settled` → `settled`):

```rust
/// Advances a pane's blank-recovery state machine one frame and returns whether
/// to repaint it from the live mirror now.
///
/// Fires once the grid has been blank while the mirror still holds content for
/// [`RESEED_DEBOUNCE_FRAMES`] consecutive frames. The episode is keyed on the
/// grid `last_seq`: a seq change reopens evaluation, and once an episode is
/// `settled` (repainted, mirror also blank, or grid painted) the per-frame
/// mirror scan is skipped until the grid changes again.
fn evaluate_blank_recovery(
    state: &mut BlankRecoveryState,
    grid: &TerminalGrid,
    handle: &TerminalHandle,
) -> bool {
    if state.recovery_seq != Some(grid.last_seq) {
        state.recovery_seq = Some(grid.last_seq);
        state.streak = 0;
        state.settled = false;
    }
    if state.settled {
        return false;
    }
    if !grid_visibly_blank(grid) {
        state.streak = 0;
        state.settled = true;
        return false;
    }
    if !handle.has_visible_content() {
        // NOTE: grid and mirror both blank — a genuinely empty pane with nothing
        // to restore. Settling here (not just returning) is load-bearing: it
        // stops the per-frame mirror scan until the grid's seq changes.
        state.settled = true;
        return false;
    }
    state.streak = state.streak.saturating_add(1);
    if state.streak >= RESEED_DEBOUNCE_FRAMES {
        state.settled = true;
        true
    } else {
        false
    }
}
```

- [ ] **Step 3: Rewrite `rescue_unpainted_panes` to query the components**

Replace the whole function:

```rust
/// Requests a tmux re-seed for each non-copy-mode pane whose grid is
/// structurally unpainted (see [`grid_needs_full_seed`]) once the state has
/// held for [`RESEED_DEBOUNCE_FRAMES`], then re-requests every
/// [`RESEED_INFLIGHT_TIMEOUT`] frames until the grid paints. Copy-mode panes
/// are skipped — they paint via the separate `CopyRenderHandle`.
///
/// Separately, recovers a grid that went *blank* (structurally fine) while its
/// live mirror still holds content: it triggers [`RepaintLiveMirror`], whose
/// observer repaints from the authoritative mirror. The gather query stays
/// read-only on the handle; the `&mut TerminalHandle` write lives in the observer.
fn rescue_unpainted_panes(
    mut commands: Commands,
    mut reseed: MessageWriter<RequestPaneReseed>,
    mut panes: Query<
        (
            Entity,
            &TmuxPane,
            &TerminalHandle,
            &TerminalGrid,
            &mut StructuralReseedState,
            &mut BlankRecoveryState,
        ),
        Without<CopyModeState>,
    >,
) {
    for (entity, pane, handle, grid, mut reseed_state, mut blank_state) in panes.iter_mut() {
        let (h_cols, h_rows, _) = handle.read_geometry();
        let needs = grid_needs_full_seed(grid.cols, grid.rows, grid.cells.len(), h_cols, h_rows);
        if reseed_decision(&mut reseed_state, needs) {
            reseed.write(RequestPaneReseed { pane: pane.id });
        }
        if needs {
            // NOTE: structural reseed owns this pane; reopen blank-recovery so it
            // re-evaluates once the grid is structurally repainted.
            *blank_state = BlankRecoveryState::default();
            continue;
        }
        if evaluate_blank_recovery(&mut blank_state, grid, handle) {
            commands.trigger(RepaintLiveMirror { entity });
        }
    }
}
```

- [ ] **Step 4: Delete `ReseedTracker`, `PaneSeedTrackers`, the prune observer, and the resource init**

In `src/mode/tmux/paint_rescue.rs`:
- Delete the `ReseedTracker` struct definition.
- Delete the `PaneSeedTrackers` struct definition.
- Delete the `prune_tracker_on_pane_removed` function.
- In `PaintRescuePlugin::build`, remove `.init_resource::<PaneSeedTrackers>()` and `.add_observer(prune_tracker_on_pane_removed)`. Resulting body:

```rust
impl Plugin for PaintRescuePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(repaint_pane_from_mirror).add_systems(
            Update,
            (attach_rescue_state, rescue_unpainted_panes)
                .chain()
                .after(TmuxProjectionSet)
                .before(TmuxLayoutSet)
                .in_set(super::TmuxActiveSet),
        );
    }
}
```

- [ ] **Step 5: Remove the now-unused `HashMap` import**

In the top `use` block, delete `use std::collections::HashMap;`. (Leave `PaneId` in the `ozmux_tmux` import — it is still used by `RequestPaneReseed { pane: pane.id }` only via `pane.id`; if `cargo build` reports `PaneId` unused, remove it too.)

- [ ] **Step 6: Update the unit tests for the new types**

In the `tests` module, replace every `ReseedTracker::default()` with the matching component, and rename field accesses. Concretely:
- Tests calling `reseed_decision(&mut t, ...)`: change `let mut t = ReseedTracker::default();` → `let mut t = StructuralReseedState::default();`. Field assertions `t.inflight_age` / `t.unpainted_streak` are unchanged (same field names).
- Tests calling `evaluate_blank_recovery(&mut t, ...)`: change `let mut t = ReseedTracker::default();` → `let mut t = BlankRecoveryState::default();`, and rename `t.blank_streak` → `t.streak`, `t.blank_recovery_settled` → `t.settled`.

- [ ] **Step 7: Update the two integration tests to provide the components**

The integration tests `blank_grid_with_live_content_repaints_from_mirror` and `blank_grid_with_blank_mirror_is_not_repainted` currently `app.init_resource::<PaneSeedTrackers>();` and spawn a pane without the state components. For each:
- Delete the `app.init_resource::<PaneSeedTrackers>();` line.
- Register the attach system ahead of the rescue system: change `app.add_systems(Update, rescue_unpainted_panes);` → `app.add_systems(Update, (attach_rescue_state, rescue_unpainted_panes).chain());`

This attaches the components on the first `app.update()`, exactly as in production. (The existing loops already run ≥ `RESEED_DEBOUNCE_FRAMES + 1` updates, so the one-frame attach does not eat the debounce budget.)

- [ ] **Step 8: Run the full paint_rescue suite**

Run: `cargo test --bin ozmux paint_rescue -- --test-threads=1`
Expected: PASS — all structural-reseed unit tests, blank-recovery unit tests, `attach_inserts_both_state_components_once`, and both integration tests green.

- [ ] **Step 9: Lint gate**

Run: `cargo build` then `cargo clippy --workspace 2>&1 | tail -2` (expect 0 warnings) then `cargo fmt --check` (expect clean; run `cargo fmt` if not).
Expected: build clean, 0 clippy warnings, fmt clean. (No `ReseedTracker` / `PaneSeedTrackers` / `prune_tracker_on_pane_removed` / `HashMap` references remain — confirm with `grep -n 'ReseedTracker\|PaneSeedTrackers\|prune_tracker\|HashMap' src/mode/tmux/paint_rescue.rs` → no output.)

- [ ] **Step 10: Commit**

```bash
git add src/mode/tmux/paint_rescue.rs
git commit -m "refactor(tmux): split ReseedTracker into per-pane components; drop HashMap resource + prune observer"
```

---

## Notes for the implementer

- The two integration tests spawn the pane with `TmuxPane` + `TerminalHandle` + `TerminalGrid`. After Step 7 they also rely on `attach_rescue_state` to add the state components — the `.chain()` is what guarantees the components exist before `rescue_unpainted_panes` reads them in the same `app.update()`.
- Do not change `grid_needs_full_seed`, `grid_visibly_blank`, `RepaintLiveMirror`, or `repaint_pane_from_mirror` — they are untouched by this refactor.
- The `On<Remove, TmuxPane>` cleanup is intentionally gone: components are dropped when the pane entity despawns (panes are despawn-only; a reused `PaneId` on reconnect spawns a fresh entity with default components).
