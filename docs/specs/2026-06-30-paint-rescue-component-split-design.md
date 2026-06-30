# paint_rescue: split the two state machines into separate Components

## Problem

`src/mode/tmux/paint_rescue.rs` holds two unrelated per-pane debounce state
machines inside a single `ReseedTracker` struct, stored in a
`PaneSeedTrackers(HashMap<PaneId, ReseedTracker>)` resource:

1. **Structural reseed** — `unpainted_streak` + `inflight_age`, driven by
   `reseed_decision`. Requests a tmux `capture-pane` re-seed when a pane's grid
   is structurally unpainted (`grid_needs_full_seed`).
2. **Blank recovery** — `blank_streak` + `blank_recovery_seq` +
   `blank_recovery_settled`, driven by `evaluate_blank_recovery`. Repaints from
   the live mirror when the grid went blank but the mirror still holds content.

Cramming both into one struct serviced by one function created a footgun: the
`!needs_seed` branch of `reseed_decision` must reset *only* its own fields, with
a `// NOTE:` warning that resetting the whole tracker (`*tracker = Default`)
would silently wipe the blank-recovery debounce every frame. The two machines
are logically independent but structurally coupled.

This is also the outlier in the codebase: peer per-pane state
(`PaneRecaptureState`, `crates/tmux_session/src/components.rs:93`) lives as a
**Component on the pane entity**, auto-attached via `#[require(PaneRecaptureState)]`
on `TmuxPane`, not as a side `HashMap` keyed by `PaneId`.

## Goal

Split the two state machines into two independent Bevy Components on the pane
entity and delete the `HashMap` resource. Pure internal refactor — externally
observable behavior (coverage, debounce timing, copy-mode freeze) is unchanged.

## Solution

### Components (defined in `paint_rescue.rs`)

Replace `ReseedTracker` with two components. The component name carries the
context, so the `blank_` field prefixes are dropped.

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

### Attaching the components

`TmuxPane` is defined in the `ozmux_tmux` crate, so the binary cannot add
`#[require(...)]` to it. Instead, a dedicated attach system inserts both
components once per pane, mirroring the existing `augment_tmux_pane`
(`src/mode/tmux/pane_focus.rs`) and `attach_tmux_pane_terminal`
(`src/mode/tmux/render.rs`) patterns:

```rust
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

A single `Without<StructuralReseedState>` filter suffices because both
components are always inserted together.

### Plugin wiring

```rust
fn build(&self, app: &mut App) {
    app.add_observer(repaint_pane_from_mirror)
        .add_systems(
            Update,
            (attach_rescue_state, rescue_unpainted_panes)
                .chain()
                .after(TmuxProjectionSet)
                .before(TmuxLayoutSet)
                .in_set(super::TmuxActiveSet),
        );
}
```

`.chain()` orders attach before rescue; Bevy's auto-inserted sync point (the
same mechanism `attach_tmux_pane_terminal` → `layout_tmux_panes` relies on)
makes the freshly-inserted components visible to `rescue_unpainted_panes` in the
same frame. Bevy 0.18's `ScheduleBuildSettings::auto_insert_apply_deferred`
defaults to `true`, and the `.chain()` ordering edge combined with the earlier
system's `Commands` param is what triggers the inserted `ApplyDeferred`.

Correctness does not *depend* on that same-frame guarantee, though: a 1-frame
attach lag would only defer a pane's first evaluation by one tick, well within
`RESEED_DEBOUNCE_FRAMES` (3), and `rescue_unpainted_panes` only matches panes
that also carry `TerminalHandle` + `TerminalGrid` (attached by a separate chain
in `render.rs`). The design therefore tolerates lag rather than relying on the
sync point.

### Decision functions and the rescue system

`reseed_decision` and `evaluate_blank_recovery` take their own component
instead of the shared `&mut ReseedTracker`:

```rust
fn reseed_decision(state: &mut StructuralReseedState, needs_seed: bool) -> bool;
fn evaluate_blank_recovery(
    state: &mut BlankRecoveryState,
    grid: &TerminalGrid,
    handle: &TerminalHandle,
) -> bool;
```

Because the blank-recovery state is now a separate component, the
`!needs_seed` branch of `reseed_decision` can reset its whole component
(`*state = StructuralReseedState::default()`) with no cross-machine footgun —
the `// NOTE:` warning is deleted.

The rescue system reads both components by `&mut` directly, dropping the
`trackers.0.entry(pane.id).or_default()` lookup:

```rust
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
            // Structural reseed owns this pane; reopen blank-recovery so it
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

### Removed

- `ReseedTracker` struct.
- `PaneSeedTrackers(HashMap<PaneId, ReseedTracker>)` resource and its
  `init_resource`.
- `prune_tracker_on_pane_removed` observer (`On<Remove, TmuxPane>`) and its
  `add_observer` — Components are dropped automatically when the entity
  despawns, so the manual prune is no longer needed. This is a strict
  improvement: panes are despawn-only (no `remove::<TmuxPane>` anywhere — stale
  panes and window-close cascades despawn the whole entity), and a `PaneId`
  reused across a reconnect spawns a *fresh* entity, so it starts from default
  component state. This matches the guarantee the in-crate `PaneRecaptureState`
  already documents and removes the class of "`HashMap` entry outlives or
  precedes the pane" bugs the prune observer existed to cover.
- `use std::collections::HashMap;`.

### Tests

- Unit tests: replace `ReseedTracker::default()` with the relevant component
  (`StructuralReseedState::default()` / `BlankRecoveryState::default()`); update
  field references (`t.blank_streak` → `t.streak`,
  `t.blank_recovery_settled` → `t.settled`).
- Integration tests (`blank_grid_with_live_content_repaints_from_mirror`,
  `blank_grid_with_blank_mirror_is_not_repainted`): spawn the pane entity with
  the two state components (or register `attach_rescue_state` chained ahead of
  `rescue_unpainted_panes`); drop the `PaneSeedTrackers` resource init.

## Behavior invariance

- **Coverage**: every `TmuxPane` gets both components via the attach system,
  matching the previous `or_default()` lazy creation for every queried pane.
- **Debounce timing**: `RESEED_DEBOUNCE_FRAMES` / `RESEED_INFLIGHT_TIMEOUT` and
  both decision functions' logic are unchanged.
- **Copy-mode freeze**: the rescue query keeps `Without<CopyModeState>`, so a
  copy-mode pane's component state is left untouched (frozen) and resumes on
  exit — identical to the previous behavior where the `HashMap` entry persisted
  untouched.

## Considered alternatives

These were evaluated during spec review and deliberately not chosen; recorded so
the reasoning is not relitigated.

- **One component with two sub-struct fields** (`PaneRescueState { structural,
  blank }`) instead of two top-level components. Achieves the same decoupling and
  kills the same footgun, with marginally fewer moving parts (one query param,
  one `insert`). Rejected in favor of "one concept = one component"; the two
  decision functions still take `&mut StructuralReseedState` /
  `&mut BlankRecoveryState` either way. A taste call, not a defect.
- **`Option<&mut StructuralReseedState>` lazy-insert** inside the rescue system
  instead of a dedicated attach system. Rejected as strictly worse: a query
  cannot materialize a missing component, so it forces a `commands.insert` whose
  effect is invisible until the next flush plus a local-default-then-write-back
  dance. The dedicated attach system matches the `augment_tmux_pane` /
  `attach_tmux_pane_terminal` idiom.
- **`On<Add, TmuxPane>` observer** to insert the components once per spawn,
  rather than a per-frame `Without<…>` polling system. Viable and idiomatic
  (mirrors `On<Add, OzmaTerminal>`); roughly a wash. Not chosen because the
  polling system stays gated inside `TmuxActiveSet` (`in_state(AppMode::Tmux)`),
  whereas an observer fires regardless of `AppMode` (harmless but a minor
  widening), and the insert is command-deferred either way.
- **Runtime `register_required_components::<TmuxPane, StructuralReseedState>()`**
  in `PaintRescuePlugin::build` (the runtime equivalent of `#[require]`, which
  the binary cannot put on the out-of-crate `TmuxPane`). Rejected due to upstream
  bug [bevyengine/bevy#16406]: runtime requirements interact incorrectly with
  components already added via `#[require(...)]`, and `TmuxPane` carries
  `#[require(PaneRecaptureState)]` — exactly the case that triggers the bug. The
  repo also uses `register_required_components` nowhere.

## Out of scope

- No change to `reseed_decision` / `evaluate_blank_recovery` logic beyond the
  parameter type and the now-safe full reset.
- No change to the `RepaintLiveMirror` event or `repaint_pane_from_mirror`
  observer.
- No change to the structural-reseed or blank-recovery *semantics*.
