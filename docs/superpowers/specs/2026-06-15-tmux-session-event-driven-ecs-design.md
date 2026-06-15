# tmux_session: event-driven ECS projection

**Date:** 2026-06-15
**Status:** Design approved, pending spec review
**Crate:** `crates/tmux_session` (+ binary consumers in `src/`)

## Problem

`crates/tmux_session` projects tmux session/window/pane state in two places at
once:

1. A plain-data model — `ProjectionModel` (a `Resource`) holding
   `Vec<WindowModel>`, each with `Vec<PaneModel>`, plus `active_pane`,
   `session`, and `session_name`. The reducer (`apply_event` / `seed_from_rows`)
   mutates it in stream order.
2. ECS entities — `TmuxSession` / `TmuxWindow` / `TmuxPane`, produced by
   `reconcile_projection`, which diffs the model into entities and maintains the
   `TmuxProjection` id→entity index.

This duplicates the state and forces non-idiomatic plumbing:

- `drain_tmux_events` mutates the model through `bypass_change_detection()` and
  then must call `model.set_changed()` by hand so an `%output` flood does not
  trigger a full reconcile every frame.
- `reconcile_projection` is gated on `resource_exists_and_changed::<ProjectionModel>`
  and re-derives the entity tree from scratch each time the model changes.
- The "model" and the entities can drift; consumers read from *both* sources
  (some read the entity components, others read the `ProjectionModel` resource
  directly for `active_pane` / `session_name` / `windows`).

## Goal

Collapse the two representations into one: the ECS entity tree is the single
source of truth. Window and pane state lives on components of dedicated
entities; those same entities also host their UI `Node`s (UI and model unified
on one entity). `drain_tmux_events` becomes a pure translator that fires
**global events** carrying only tmux-side ids; observers resolve ids to entities
and apply all state changes.

Outcomes:

- Delete `ProjectionModel`, `WindowModel`, `PaneModel`, and the reducer.
- Delete the standalone `reconcile_projection` system and its
  `resource_exists_and_changed` gate.
- Remove `bypass_change_detection` / `set_changed` — Bevy change detection fires
  automatically on component insert/mutate.
- Active state becomes marker components, not resource fields.

## Non-goals

- No change to the connection lifecycle (`ConnectionState`), the transport
  drain mechanics, the enumeration command flow (`EnumerationState`,
  `list-windows` / `client-name`), or `%output` routing (`PaneOutput` stays a
  `Message`).
- No change to how panes are rendered (`TerminalHandle` + render bundle) beyond
  the source of the data driving them.
- No new tmux features.

## Architecture

### Event flow

```
tmux -CC transport
   │  TransportEvent
   ▼
drain_tmux_events  (pure translator)
   │  commands.trigger(<global Event carrying tmux ids only>)
   ▼
observers  (On<Event>, ResMut<TmuxProjection> + Commands)
   │  resolve id→entity via index, spawn/despawn/insert components,
   │  maintain index
   ▼
entity tree  (TmuxSession / TmuxWindow / TmuxPane + markers + UI Nodes)
   ▲
consumers  (render, input, window bar, pane focus, status bar)
   read components / markers; react via Added/Removed/Changed queries
```

### Events (global `#[derive(Event)]`, ids only — no `Entity` in payloads)

| Source | Event |
| --- | --- |
| `%session-changed` | `TmuxSessionChanged { session: SessionId, name: String }` |
| `%window-add` / seed row | `TmuxWindowAdded { window: WindowId, index: u32, name: String }` |
| `%window-close` | `TmuxWindowClosed { window: WindowId }` |
| `%window-renamed` | `TmuxWindowRenamed { window: WindowId, name: String }` |
| `%layout-change` / seed row | `TmuxLayoutChanged { window: WindowId, panes: Vec<PaneGeom> }` |
| `%window-pane-changed` | `TmuxActivePaneChanged { window: WindowId, pane: PaneId }` |
| seed row (active flag) | `TmuxActiveWindowChanged { window: WindowId }` |
| seed prune | `TmuxWindowsRetained { windows: Vec<WindowId> }` |
| transport `Closed` | `TmuxConnectionReset` |

`PaneGeom { id: PaneId, dims: CellDims }` is a plain payload value type that
replaces `PaneModel`. `pane_leaves(&WindowLayout) -> Vec<PaneGeom>` survives as
an internal helper.

**Notification → event mapping notes:**

- A `%window-add` notification carries only the id, so it triggers
  `TmuxWindowAdded { window, index: 0, name: String::new() }`. tmux does not
  re-send `WindowAdd` for an existing window, so the defaults never clobber real
  metadata; the seed path supplies the real `index` / `name`.
- The seed (the `list-windows` reply matching `EnumerationState.pending`) is
  decomposed into granular events per row, then a single prune:
  for each row → `TmuxWindowAdded { window, index, name }`,
  `TmuxLayoutChanged { window, panes }`, and (if the row is active)
  `TmuxActiveWindowChanged { window }`; finally
  `TmuxWindowsRetained { windows: <all row ids> }`.
- Events are triggered in transport stream order. Because the seed reply
  occupies its real position in the drained batch, any notification ordered
  after it in the same batch is triggered (and its observer runs) after the seed
  events — preserving the ordering guarantee that `apply_events` documents today.

### Id→Entity index (`TmuxProjection` resource, repurposed)

`TmuxProjection` stays as the id→entity index only — no state data:

```rust
#[derive(Resource, Default)]
struct TmuxProjection {
    windows: HashMap<WindowId, Entity>,
    panes: HashMap<PaneId, Entity>,
    session: Option<Entity>,
}
```

It is now an internal implementation detail of the crate (no longer part of the
state read by consumers); demote its visibility accordingly.

**In-batch ordering guarantee:** an observer that creates an entity reserves the
`Entity` id with `commands.spawn(()).id()` and writes it into
`ResMut<TmuxProjection>` *synchronously* before returning. Observers run in
trigger order on the main thread, so a later same-batch event (e.g. seed →
`TmuxWindowAdded @1` → `TmuxLayoutChanged @1`) resolves `@1` to the reserved
entity even though the spawn command has not yet flushed. All structural
mutation (component insert, child spawn, despawn) flows through `Commands` and
applies in order. This replaces the synchronous-stream-order reducer.

### Observers

All observers take `ResMut<TmuxProjection>` + `Commands`; they resolve ids and
mutate the world.

- `on_session_changed` — ensure the session entity (spawn-reserve + index if
  absent); insert `TmuxSession { id }` and `TmuxSessionName(name)`.
- `on_window_added` — ensure the window entity; insert
  `TmuxWindow { id, index, name }`. Idempotent ensure: spawn-and-set if absent,
  set fields if present.
- `on_window_renamed` — resolve; set `TmuxWindow.name`.
- `on_layout_changed` — per-window pane diff: spawn missing panes as
  `ChildOf(window)` and index them, update `TmuxPane.dims` on existing, despawn
  removed panes and remove them from the index.
- `on_active_pane_changed` — ensure window + pane exist; move the `ActivePane`
  marker (remove from prior holder, insert on the resolved pane) and move
  `ActiveWindow` to the resolved window.
- `on_active_window_changed` — move the `ActiveWindow` marker only (seed's
  per-row active flag).
- `on_windows_retained` — despawn every window not in the set (and its panes via
  cascade), pruning the index. Replaces the model's wholesale window
  replacement.
- `on_connection_reset` — despawn the session and every window/pane; clear the
  index.

The `ActivePane` marker is preserved across a seed unless its pane entity is
despawned by `on_layout_changed` / `on_windows_retained`, matching the current
`prune_active_pane` semantics (active pane cleared only when the pane vanishes).

`on_window_closed` / `on_windows_retained` despawning a window cascades to its
`ChildOf` pane children; the observer must remove those pane ids from the index
to avoid dangling entries (look them up via the window's `Children` or a reverse
scan of `index.panes`).

### `drain_tmux_events` (pure translator)

Retains: drain transport, advance `ConnectionState`, send the enumeration
commands on attach, `take_client_name`, and write `PaneOutput` messages.
Removes: all `ProjectionModel` interaction (`apply_events`,
`bypass_change_detection`, `set_changed`). For each drained event it triggers
the corresponding global event via `commands.trigger`. The seed reply is parsed
and decomposed into the granular events above. On `Closed` it reclaims the dead
client (as today) and triggers `TmuxConnectionReset`.

The `reconcile_projection` system and the
`resource_exists_and_changed::<ProjectionModel>` registration are removed.
`drain_tmux_events` stays in `TmuxProjectionSet`; the binary's render systems
continue to run `.after(TmuxProjectionSet)`.

### Components

- `TmuxSession { id: SessionId }` — unchanged.
- `TmuxWindow { id, index, name }` — **`active: bool` removed**.
- `TmuxPane { id, dims }` — unchanged.
- New: `TmuxSessionName(String)`, `ActivePane` (ZST marker), `ActiveWindow`
  (ZST marker).

Hierarchy is unchanged: a window entity's ECS parent is `WorkspaceUiRoot`
(attached by the render layer), a pane is `ChildOf(window)`, and the session
entity stands alone. Teardown despawns via the index.

## Consumer changes

| File | Before | After |
| --- | --- | --- |
| `src/tmux_render.rs` `sync_active_window` | reads `TmuxWindow.active` | query `With<ActiveWindow>` to pick the shown window; hide others |
| `src/tmux_render.rs` (rest) | reads `TmuxPane` / `TmuxWindow` / `TmuxProjection` | unchanged (output routing keeps using the index) |
| `src/tmux_input.rs` paste | `model.active_pane` | `Single<&TmuxPane, With<ActivePane>>` |
| `src/ui/tmux_pane_focus.rs` `sync_pane_dim` | `model.active_pane` + `run_if(resource_changed)` | `Has<ActivePane>` query, gated by `Added<ActivePane>` / `RemovedComponents<ActivePane>` |
| `src/ui/tmux_window_bar.rs` `rebuild_window_bar` | rebuild from `model.windows` / `model.session_name` + `run_if(resource_changed::<ProjectionModel>)` | rebuild from window entities + the session entity's `TmuxSessionName`; gate on window-set / metadata / active-marker changes |
| `src/ui/status_bar_sync.rs` `tmux_projection_present` | `Option<Res<ProjectionModel>>` | based on the session entity existing (e.g. a `Single<(), With<TmuxSession>>` check or a lightweight presence resource) |

`TmuxProjection` becomes crate-private, so any consumer reference to it must
route through the new component/marker queries instead. Per-entity
change-detection queries (`Added` / `Changed` / `RemovedComponents`) are the
gating mechanism for consumers; these are per-entity, not whole-system guards,
so they comply with the repo's `run_if` rule.

## lib.rs surface

- Remove exports: `PaneModel`, `ProjectionModel`, `WindowModel`, `pane_leaves`.
- Remove `TmuxProjection` from the public surface (now crate-private).
- Add exports: the event types consumers need to observe (if any consumer
  observes them directly), `ActivePane`, `ActiveWindow`, `TmuxSessionName`.
  `TmuxSession` / `TmuxWindow` / `TmuxPane` stay exported.

## Testing

- `model.rs` reducer tests → observer behavior tests over entities (assert the
  spawned/updated/despawned entities and markers after triggering each event).
- `event_pump.rs` `apply_events` tests → `drain_tmux_events` trigger tests
  (assert the right events fire in the right order for a given batch, including
  the seed decomposition + prune).
- `reconcile.rs` tests → observer reconcile tests (layout diff: spawn/update/
  despawn panes; window retain/prune; session spawn/teardown).
- Consumer tests (`tmux_window_bar`, `tmux_pane_focus`, `tmux_render`) updated to
  drive state by triggering events / inserting markers instead of inserting a
  `ProjectionModel`.
- `tests/real_tmux*.rs` (tmux-gated integration tests) updated to assert against
  entities + markers.

## Risks / edge cases

- **In-batch creation ordering** — covered by the spawn-reserve-then-index rule
  above; must be exercised by a test that triggers `WindowAdded @1` and
  `LayoutChanged @1` in one batch with no flush between.
- **Despawn cascade vs. index** — despawning a window cascades to pane children;
  the index must be pruned for those panes or stale ids leak. Tested by the
  window-close / retain tests.
- **Active markers are singletons** — at most one `ActivePane` and one
  `ActiveWindow` should exist. Observers must remove the marker from the prior
  holder before inserting on the new one; consumers using `Single` would panic
  on a duplicate, so tests assert singleton-ness.
- **Window `index` provenance** — `index` only ever arrives via the seed (full
  enumeration); notifications leave it at its last-seen value. This matches
  current behavior (the model only set `index` from `seed_from_rows`).
```
