# tmux_session event-driven ECS projection — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace `crates/tmux_session`'s duplicated `ProjectionModel` + `reconcile` design with a single ECS source of truth: `drain_tmux_events` fires global events carrying tmux ids, observers resolve ids to entities and mutate components/markers.

**Architecture:** A *strangler-fig* migration. Phase 1 enriches the existing entity tree (the old `reconcile` keeps running but now also sets `ActivePane`/`ActiveWindow` markers and `TmuxSession.name`), then migrates every consumer off the `ProjectionModel` resource onto component/marker queries — each commit stays green and the app keeps working. Phase 2 cuts the internals over: new `events.rs` + `observers.rs` replace `model.rs` + `reconcile.rs`; `drain_tmux_events` triggers events instead of mutating the model; the model is deleted last, once no consumer reads it.

**Tech Stack:** Rust edition 2024, Bevy 0.18 ECS (global `#[derive(Event)]` + `commands.trigger` + `On<E>` observers, `commands.spawn().id()` id reservation, `Added`/`RemovedComponents` change detection), `tmux_control` / `tmux_control_parser`.

**Spec:** `docs/superpowers/specs/2026-06-15-tmux-session-event-driven-ecs-design.md`

**Key Bevy facts this plan relies on:**
- `commands.trigger(E)` is deferred; trigger commands apply FIFO; each applied trigger runs its observer synchronously and that observer's commands flush before the next trigger applies. So an observer that does `let e = commands.spawn(..).id(); index.insert(id, e);` makes `e` resolvable by a later same-batch event via the index — **without** relying on `e`'s components being live yet.
- **Exactly one observer per event type.** Multiple observers of one event have unspecified order and would break the ordering invariant.
- `On<E>` derefs to `&E`; read fields as `ev.field`.
- A bare `Single<…>` system param *skips the whole system* on 0 or ≥2 matches — use `Option<Single<…>>` where a graceful miss is required.
- `commands.entity(e).despawn()` in Bevy 0.18 cascades to `ChildOf` children, so despawning a window despawns its panes — index bookkeeping must be pruned but pane entities must NOT be despawned again.

**Conventions reminder (`.claude/rules/rust.md`):** no `mod.rs`; comments only `// TODO:` / `// NOTE:` (critical caveats only) / `// SAFETY:`; `//!` on every module file; `///` on every `pub` item; all `use` at top, single block; mutable params before immutable; private items last in a block; minimize visibility.

**Commands:**
- Crate tests: `cargo test -p ozmux_tmux`
- Binary tests: `cargo test -p ozmux-gui` (root package)
- Lint/format: `cargo clippy --workspace --fix --allow-dirty --allow-staged && cargo fmt`

> NOTE: The crate is named `ozmux_tmux` (the `tmux_session` directory). External imports use `ozmux_tmux::…`; `cargo test -p ozmux_tmux`.

---

## File map

**Phase 1 (bridge + consumer migration):**
- Modify `crates/tmux_session/src/components.rs` — add `name` to `TmuxSession`; add `ActivePane`, `ActiveWindow` markers.
- Modify `crates/tmux_session/src/reconcile.rs` — set `TmuxSession.name` + the two markers from the model.
- Modify `crates/tmux_session/src/lib.rs` — export the markers.
- Modify `src/tmux_render.rs` — `route_tmux_output` query-based pane lookup; `sync_active_window` via `ActiveWindow`.
- Modify `src/tmux_input.rs` — paste via `Option<Single<&TmuxPane, With<ActivePane>>>`.
- Modify `src/ui/tmux_pane_focus.rs` — `sync_pane_dim` via `Has<ActivePane>` + `Added` gates.
- Modify `src/ui/tmux_window_bar.rs` — rebuild from window/session entities.
- Modify `crates/tmux_session/src/plugin.rs` + `src/ui/status_bar_sync.rs` — `TmuxPresence` resource.

**Phase 2 (internals cutover):**
- Create `crates/tmux_session/src/events.rs` — global events + `PaneGeom` + `pane_geoms`.
- Create `crates/tmux_session/src/observers.rs` — the `TmuxProjection` index (new shape) + observers.
- Delete `crates/tmux_session/src/model.rs`, `crates/tmux_session/src/reconcile.rs`.
- Modify `crates/tmux_session/src/event_pump.rs` — drop `apply_events`/`seed_from_reply`; keep `drain_transport`/`advance_state`/`take_client_name`.
- Modify `crates/tmux_session/src/plugin.rs` — drain triggers events; register observers; drop reconcile.
- Modify `crates/tmux_session/src/components.rs` — remove `TmuxWindow.active`.
- Modify `crates/tmux_session/src/lib.rs` — final exports.
- Modify `crates/tmux_session/tests/real_tmux*.rs` — assert entities/markers.

---

# Phase 1 — Bridge & consumer migration

## Task 1: Markers + `TmuxSession.name`, set from the existing reconcile

**Files:**
- Modify: `crates/tmux_session/src/components.rs`
- Modify: `crates/tmux_session/src/reconcile.rs`
- Modify: `crates/tmux_session/src/lib.rs`

- [ ] **Step 1: Add the markers and the session name field**

In `crates/tmux_session/src/components.rs`, change `TmuxSession` and append the markers:

```rust
/// The projected tmux session entity.
#[derive(Component, Debug, Clone, PartialEq, Eq)]
pub struct TmuxSession {
    /// tmux session id (`$N`).
    pub id: SessionId,
    /// Session name, from `%session-changed`. Empty until first known.
    pub name: String,
}

/// Marker on the single active pane entity (`%window-pane-changed`).
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ActivePane;

/// Marker on the single active window entity.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ActiveWindow;
```

- [ ] **Step 2: Write a failing test that reconcile sets name + markers**

Append to the `tests` module in `crates/tmux_session/src/reconcile.rs`:

```rust
#[test]
fn reconcile_sets_session_name_and_active_markers() {
    use crate::components::{ActivePane, ActiveWindow};
    let mut app = app();
    {
        let mut model = app.world_mut().resource_mut::<ProjectionModel>();
        model.session = Some(SessionId(1));
        model.session_name = Some("main".to_string());
        model.active_pane = Some(PaneId(9));
        model.windows = vec![WindowModel {
            id: WindowId(1),
            active: true,
            index: 0,
            name: "w".to_string(),
            panes: vec![PaneModel { id: PaneId(9), dims: dims() }],
        }];
    }
    app.update();

    let index = app.world().resource::<TmuxProjection>();
    let session_entity = index.session.unwrap();
    let window_entity = index.windows[&WindowId(1)];
    let pane_entity = index.panes[&PaneId(9)];

    assert_eq!(app.world().get::<TmuxSession>(session_entity).unwrap().name, "main");
    assert!(app.world().get::<ActiveWindow>(window_entity).is_some());
    assert!(app.world().get::<ActivePane>(pane_entity).is_some());
}
```

- [ ] **Step 3: Run it — expect failure**

Run: `cargo test -p ozmux_tmux reconcile_sets_session_name_and_active_markers`
Expected: FAIL (compile error: `TmuxSession.name` set nowhere / no markers inserted), then assertion failure.

- [ ] **Step 4: Update reconcile to set name + markers**

In `crates/tmux_session/src/reconcile.rs`, update `reconcile_session` to include `name`:

```rust
fn reconcile_session(commands: &mut Commands, index: &mut TmuxProjection, model: &ProjectionModel) {
    let name = model.session_name.clone().unwrap_or_default();
    match (model.session, index.session) {
        (Some(id), Some(entity)) => {
            commands.entity(entity).insert(TmuxSession { id, name });
        }
        (Some(id), None) => {
            let entity = commands.spawn(TmuxSession { id, name }).id();
            index.session = Some(entity);
        }
        (None, Some(entity)) => {
            commands.entity(entity).despawn();
            index.session = None;
        }
        (None, None) => {}
    }
}
```

Add the marker imports to the top `use` block (merge into the existing `crate::components::{…}` line):

```rust
use crate::components::{ActivePane, ActiveWindow, TmuxPane, TmuxSession, TmuxWindow};
```

Add marker params to `reconcile_projection` and a marker step (mutable params first):

```rust
pub(crate) fn reconcile_projection(
    mut commands: Commands,
    mut index: ResMut<TmuxProjection>,
    prev_active_window: Query<Entity, With<ActiveWindow>>,
    prev_active_pane: Query<Entity, With<ActivePane>>,
    model: Res<ProjectionModel>,
) {
    reconcile_windows(&mut commands, &mut index, &model);
    reconcile_session(&mut commands, &mut index, &model);
    reconcile_markers(&mut commands, &index, &prev_active_window, &prev_active_pane, &model);
}

fn reconcile_markers(
    commands: &mut Commands,
    index: &TmuxProjection,
    prev_active_window: &Query<Entity, With<ActiveWindow>>,
    prev_active_pane: &Query<Entity, With<ActivePane>>,
    model: &ProjectionModel,
) {
    for e in prev_active_window.iter() {
        commands.entity(e).remove::<ActiveWindow>();
    }
    if let Some(active) = model.windows.iter().find(|w| w.active)
        && let Some(&entity) = index.windows.get(&active.id)
    {
        commands.entity(entity).insert(ActiveWindow);
    }
    for e in prev_active_pane.iter() {
        commands.entity(e).remove::<ActivePane>();
    }
    if let Some(pane) = model.active_pane
        && let Some(&entity) = index.panes.get(&pane)
    {
        commands.entity(entity).insert(ActivePane);
    }
}
```

- [ ] **Step 5: Run the crate tests**

Run: `cargo test -p ozmux_tmux`
Expected: PASS (the new test + all existing reconcile tests; the existing `spawns_session_entity_from_model_session` test still passes because it asserts only `.id`).

- [ ] **Step 6: Export the markers**

In `crates/tmux_session/src/lib.rs`, change the components re-export:

```rust
pub use components::{ActivePane, ActiveWindow, TmuxPane, TmuxSession, TmuxWindow};
```

- [ ] **Step 7: Build the workspace**

Run: `cargo build`
Expected: success (no consumer broke — `TmuxSession.name` is additive; `TmuxSession` construction sites are all inside this crate and updated).

- [ ] **Step 8: Commit**

```bash
git add crates/tmux_session/src/components.rs crates/tmux_session/src/reconcile.rs crates/tmux_session/src/lib.rs
git commit -m "feat(tmux): bridge ActivePane/ActiveWindow markers + TmuxSession.name from reconcile"
```

---

## Task 2: `route_tmux_output` — pane lookup via query, not the index

This removes the binary's last use of `TmuxProjection`, so Phase 2 can make it crate-private.

**Files:**
- Modify: `src/tmux_render.rs:103-142` (`route_tmux_output`) and its test (`src/tmux_render.rs:322-380`)

- [ ] **Step 1: Replace the index dependency with a `TmuxPane` query**

In `src/tmux_render.rs`, change the `route_tmux_output` signature and the lookup. Replace `index: Res<TmuxProjection>` with a pane query, and build a `PaneId -> Entity` map at the top:

```rust
fn route_tmux_output(
    mut commands: Commands,
    mut reader: MessageReader<PaneOutput>,
    mut handles: Query<&mut TerminalHandle>,
    panes: Query<(Entity, &TmuxPane)>,
    connection: NonSend<TmuxConnection>,
) {
    let mut by_pane: HashMap<_, Vec<u8>> = HashMap::new();
    for msg in reader.read() {
        by_pane
            .entry(msg.pane)
            .or_default()
            .extend_from_slice(&msg.data);
    }
    let entity_of: HashMap<_, _> = panes.iter().map(|(e, p)| (p.id, e)).collect();
    for (pane, data) in by_pane {
        let Some(&entity) = entity_of.get(&pane) else {
            continue;
        };
        let Ok(mut handle) = handles.get_mut(entity) else {
            continue;
        };
        handle.advance(&data);
        handle.flush_emit(&mut commands, entity);
        let replies = handle.take_replies();
        if replies.is_empty() {
            continue;
        }
        let Some(client) = connection.client() else {
            continue;
        };
        let target = format!("%{}", pane.0);
        for chunk in replies.chunks(REPLY_CHUNK_BYTES) {
            let cmd = send_bytes_command(&target, chunk);
            if let Err(e) = client.handle().send(&cmd) {
                tracing::warn!(?e, pane = pane.0, "reply forward send failed");
                break;
            }
        }
    }
}
```

Remove `TmuxProjection` from the `ozmux_tmux::{…}` import in `src/tmux_render.rs` (line 14-17).

- [ ] **Step 2: Update the test to not use the index**

In `src/tmux_render.rs` `output_routed_into_pane_grid_renders_text` (around line 322): remove `app.init_resource::<TmuxProjection>();` and the `index.panes.insert(...)` block; the pane entity already carries `TmuxPane`, which the query now finds. The test body after spawning the pane becomes:

```rust
        // A projected pane entity (the query finds it by its TmuxPane).
        let pane_id = PaneId(1);
        let pane_entity = app
            .world_mut()
            .spawn(TmuxPane {
                id: pane_id,
                dims: dims(),
            })
            .id();
```

Remove the now-unused `TmuxProjection` import in the test module if present.

- [ ] **Step 3: Run binary tests**

Run: `cargo test -p ozmux-gui output_routed_into_pane_grid_renders_text`
Expected: PASS.

- [ ] **Step 4: Build**

Run: `cargo build`
Expected: success.

- [ ] **Step 5: Commit**

```bash
git add src/tmux_render.rs
git commit -m "refactor(tmux): route pane output via TmuxPane query, drop index dependency"
```

---

## Task 3: `sync_pane_dim` — markers instead of `ProjectionModel`

**Files:**
- Modify: `src/ui/tmux_pane_focus.rs` (plugin registration, `sync_pane_dim`, test)

- [ ] **Step 1: Replace the system and its gate**

In `src/ui/tmux_pane_focus.rs`, change the import line to drop `ProjectionModel` usage and update the plugin + system. Registration:

```rust
impl Plugin for OzmuxTmuxPaneFocusPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                augment_tmux_pane.after(TmuxProjectionSet),
                focus_pane_on_click.in_set(InputPhase::Dispatch),
                sync_pane_dim.run_if(pane_active_state_changed),
            ),
        );
    }
}
```

New gate + system (replacing the old `sync_pane_dim`):

```rust
/// True when a pane's active state may have changed this frame: a new pane
/// appeared, or the `ActivePane` marker was inserted/removed.
fn pane_active_state_changed(
    mut removed_active: RemovedComponents<ActivePane>,
    added_panes: Query<(), Added<TmuxPane>>,
    added_active: Query<(), Added<ActivePane>>,
) -> bool {
    added_panes.iter().next().is_some()
        || added_active.iter().next().is_some()
        || removed_active.read().next().is_some()
}

/// Sets each pane entity's [`PaneDim`] brightness: `1.0` for the pane carrying
/// `ActivePane` (or for all panes when no pane is active), the configured dim
/// factor otherwise. Only inserts when the value changes.
fn sync_pane_dim(
    mut commands: Commands,
    panes: Query<(Entity, Has<ActivePane>, Option<&PaneDim>), With<TmuxPane>>,
    configs: Option<Res<OzmuxConfigsResource>>,
) {
    let dim_factor = inactive_dim_factor(configs.as_deref());
    let any_active = panes.iter().any(|(_, active, _)| active);
    for (entity, active, current) in panes.iter() {
        let want = if active || !any_active { 1.0 } else { dim_factor };
        if current.map(|d| d.0) != Some(want) {
            commands.entity(entity).insert(PaneDim(want));
        }
    }
}
```

Update the top `use` block: replace `use ozmux_tmux::{TmuxConnection, TmuxPane, TmuxProjectionSet, select_pane_command};` with:

```rust
use ozmux_tmux::{ActivePane, TmuxConnection, TmuxPane, TmuxProjectionSet, select_pane_command};
```

- [ ] **Step 2: Rewrite the test to drive state via markers**

Replace `sync_sets_pane_dim_from_active_pane` (in the `tests` module) with a marker-driven version:

```rust
#[test]
fn sync_sets_pane_dim_from_active_marker() {
    use ozmux_tmux::ActivePane;

    let mut app = App::new();
    app.add_plugins((MinimalPlugins, OzmuxTmuxPaneFocusPlugin));
    app.insert_non_send_resource(ozmux_tmux::TmuxConnection::default());
    app.insert_resource(OzmuxConfigsResource::default());
    let h = || TerminalHandle::detached(10, 5, Arc::new(AtomicBool::new(false)));
    let p1 = app.world_mut().spawn((TmuxPane { id: PaneId(1), dims: dims() }, h(), ActivePane)).id();
    let p2 = app.world_mut().spawn((TmuxPane { id: PaneId(2), dims: dims() }, h())).id();
    let dim = |app: &App, e| app.world().get::<PaneDim>(e).map(|d| d.0);

    app.update();
    assert_eq!(dim(&app, p1), Some(1.0), "active pane full-bright");
    assert_eq!(dim(&app, p2), Some(0.5), "inactive pane dimmed");

    // Move ActivePane to p2.
    app.world_mut().entity_mut(p1).remove::<ActivePane>();
    app.world_mut().entity_mut(p2).insert(ActivePane);
    app.update();
    assert_eq!(dim(&app, p1), Some(0.5));
    assert_eq!(dim(&app, p2), Some(1.0));

    // No active pane: both full-bright.
    app.world_mut().entity_mut(p2).remove::<ActivePane>();
    app.update();
    assert_eq!(dim(&app, p1), Some(1.0));
    assert_eq!(dim(&app, p2), Some(1.0));
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p ozmux-gui -- tmux_pane_focus`
Expected: PASS (`sync_sets_pane_dim_from_active_marker` + `augment_adds_button_and_focus_block_no_overlay`).

- [ ] **Step 4: Commit**

```bash
git add src/ui/tmux_pane_focus.rs
git commit -m "refactor(tmux): drive pane dim from ActivePane marker, gated on marker/pane changes"
```

---

## Task 4: `tmux_input` paste — `Option<Single<&TmuxPane, With<ActivePane>>>`

**Files:**
- Modify: `src/tmux_input.rs` (`forward_keys_to_tmux` signature + paste branch)

- [ ] **Step 1: Swap the active-pane source**

In `src/tmux_input.rs`, change the import to drop `ProjectionModel`:

```rust
use ozmux_tmux::{
    ActivePane, KeyMods, TmuxConnection, TmuxPane, bevy_key_to_tmux_name, send_bytes_command,
    send_keys_command,
};
```

Replace the `model: Res<ProjectionModel>` param with:

```rust
    active_pane: Option<Single<&TmuxPane, With<ActivePane>>>,
```

(place it among the immutable params, after the other read-only params). In the `GuiChord::Paste` branch, replace `let Some(pane) = model.active_pane else { continue; };` with:

```rust
                    let Some(active) = active_pane.as_deref() else {
                        continue;
                    };
                    let pane = active.id;
```

The rest of the paste branch (`format!("%{}", pane.0)`) is unchanged.

- [ ] **Step 2: Build (no unit test exists for the paste path; the chord classifier tests still apply)**

Run: `cargo test -p ozmux-gui -- tmux_input`
Expected: PASS (existing `gui_chord` tests unaffected).

- [ ] **Step 3: Build the workspace**

Run: `cargo build`
Expected: success.

- [ ] **Step 4: Commit**

```bash
git add src/tmux_input.rs
git commit -m "refactor(tmux): paste targets the ActivePane entity via Option<Single>"
```

---

## Task 5: `sync_active_window` — `With<ActiveWindow>` instead of `TmuxWindow.active`

**Files:**
- Modify: `src/tmux_render.rs:264-275` (`sync_active_window`)

- [ ] **Step 1: Query the marker**

In `src/tmux_render.rs`, replace `sync_active_window`:

```rust
fn sync_active_window(mut windows: Query<(&mut Node, Has<ActiveWindow>), With<TmuxWindow>>) {
    for (mut node, active) in windows.iter_mut() {
        let want = if active { Display::Flex } else { Display::None };
        if node.display != want {
            node.display = want;
        }
    }
}
```

Add `ActiveWindow` to the `ozmux_tmux::{…}` import in `src/tmux_render.rs`.

- [ ] **Step 2: Build**

Run: `cargo build`
Expected: success (`TmuxWindow.active` is still present from Task 1; this just stops reading it).

- [ ] **Step 3: Run binary tests**

Run: `cargo test -p ozmux-gui -- tmux_render`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src/tmux_render.rs
git commit -m "refactor(tmux): show active window via ActiveWindow marker"
```

---

## Task 6: `tmux_window_bar` — rebuild from window/session entities

**Files:**
- Modify: `src/ui/tmux_window_bar.rs` (imports, plugin gate, `rebuild_window_bar`, test)

- [ ] **Step 1: Replace the model reads with entity queries**

In `src/ui/tmux_window_bar.rs`, change the import:

```rust
use ozmux_tmux::{ActiveWindow, TmuxSession, TmuxWindow, WindowId};
```

Change the plugin registration gate:

```rust
        app.add_systems(
            Update,
            rebuild_window_bar.run_if(window_bar_dirty),
        );
```

Add the gate function and rewrite `rebuild_window_bar`:

```rust
/// True when the window set, any window's metadata, the active window, or the
/// session name may have changed this frame.
fn window_bar_dirty(
    mut removed_windows: RemovedComponents<TmuxWindow>,
    mut removed_active: RemovedComponents<ActiveWindow>,
    added_windows: Query<(), Added<TmuxWindow>>,
    changed_windows: Query<(), Changed<TmuxWindow>>,
    added_active: Query<(), Added<ActiveWindow>>,
    changed_session: Query<(), Changed<TmuxSession>>,
) -> bool {
    added_windows.iter().next().is_some()
        || changed_windows.iter().next().is_some()
        || added_active.iter().next().is_some()
        || changed_session.iter().next().is_some()
        || removed_windows.read().next().is_some()
        || removed_active.read().next().is_some()
}

/// Despawns the window bar's children and rebuilds them from the window
/// entities + the session entity: a `[session]` label followed by one
/// clickable entry per window (ascending `index`), with the `ActiveWindow`
/// highlighted. Gated by `window_bar_dirty` at registration.
fn rebuild_window_bar(
    mut commands: Commands,
    bar: Query<Entity, With<WindowBarRoot>>,
    windows: Query<(&TmuxWindow, Has<ActiveWindow>)>,
    session: Query<&TmuxSession>,
    ui_font: Option<Res<TerminalUiFont>>,
) {
    let Ok(bar) = bar.single() else {
        return;
    };
    commands.entity(bar).despawn_related::<Children>();

    let font = ui_font.as_deref().map(|f| f.0.clone()).unwrap_or_default();

    let session_name = session.iter().next().map(|s| s.name.as_str()).unwrap_or("");
    commands.spawn((
        SessionLabel,
        Text::new(format!("[{session_name}]")),
        TextColor(palette::ACCENT),
        TextFont {
            font: font.clone(),
            font_size: theme::UI_FONT_SIZE,
            ..default()
        },
        ChildOf(bar),
    ));

    let mut entries: Vec<(u32, WindowId, String, bool)> = windows
        .iter()
        .map(|(w, active)| (w.index, w.id, w.name.clone(), active))
        .collect();
    entries.sort_by_key(|(index, id, _, _)| (*index, id.0));

    for (index, id, name, active) in entries {
        let (bg, fg) = if active {
            (palette::TAB_ACTIVE_BG, palette::FOREGROUND)
        } else {
            (palette::PANEL, palette::MUTED)
        };
        let entry = commands
            .spawn((
                Button,
                Node {
                    align_items: AlignItems::Center,
                    padding: UiRect::axes(Val::Px(theme::TAB_PADDING_X_PX), Val::Px(0.0)),
                    ..default()
                },
                BackgroundColor(bg),
                WindowEntry { index, window: id },
                WindowEntryActive(active),
                ChildOf(bar),
            ))
            .id();
        commands.spawn((
            Text::new(window_label(index, &name)),
            TextColor(fg),
            TextFont {
                font: font.clone(),
                font_size: theme::UI_FONT_SIZE,
                ..default()
            },
            ChildOf(entry),
        ));
    }
}
```

> NOTE: sort by `(index, id)` so the bar order is stable and deterministic — entity iteration order is not.

- [ ] **Step 2: Rewrite the test to spawn entities**

Replace `rebuild_renders_window_entries_with_active_highlight` with an entity-driven version:

```rust
#[test]
fn rebuild_renders_window_entries_with_active_highlight() {
    use ozmux_tmux::{ActiveWindow, TmuxWindow};

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(OzmuxTmuxWindowBarPlugin);
    app.insert_resource(metrics_fixture());
    app.insert_non_send_resource(ozmux_tmux::TmuxConnection::default());
    app.world_mut().spawn((Node::default(), UiRoot));

    app.world_mut().spawn(TmuxWindow { id: WindowId(1), index: 0, name: "zsh".into() });
    app.world_mut().spawn((TmuxWindow { id: WindowId(2), index: 1, name: "vim".into() }, ActiveWindow));

    app.update();
    app.update();

    let world = app.world_mut();
    let mut q = world.query::<(&WindowEntry, &WindowEntryActive)>();
    let mut entries: Vec<(u32, u32, bool)> = q
        .iter(world)
        .map(|(e, a)| (e.index, e.window.0, a.0))
        .collect();
    entries.sort();
    assert_eq!(entries, vec![(0, 1, false), (1, 2, true)]);
}
```

> NOTE: this test no longer references `TmuxWindow.active` (removed in Phase 2). After Task 1, `TmuxWindow` still HAS `active`; do not set it here — the highlight comes from the `ActiveWindow` marker.

- [ ] **Step 3: Run tests**

Run: `cargo test -p ozmux-gui -- tmux_window_bar`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src/ui/tmux_window_bar.rs
git commit -m "refactor(tmux): rebuild window bar from window/session entities + ActiveWindow"
```

---

## Task 7: `TmuxPresence` resource for the status-bar gate

**Files:**
- Modify: `crates/tmux_session/src/plugin.rs` (insert resource), `crates/tmux_session/src/lib.rs` (export), `src/ui/status_bar_sync.rs` (use it)

- [ ] **Step 1: Add and insert the presence resource**

In `crates/tmux_session/src/plugin.rs`, define the resource near the plugin:

```rust
/// Present (inserted at plugin build) whenever the tmux backend is active, so
/// consumers can gate "tmux mode" from frame 0 — before any `%session-changed`.
#[derive(Resource, Default)]
pub struct TmuxPresence;
```

Insert it in `build` alongside the other resources:

```rust
            .init_resource::<EnumerationState>()
            .insert_resource(TmuxPresence)
```

Export it from `crates/tmux_session/src/lib.rs`:

```rust
pub use plugin::{TmuxPresence, TmuxProjectionSet, TmuxSessionPlugin};
```

- [ ] **Step 2: Use it in the status-bar gate**

In `src/ui/status_bar_sync.rs`, change the import `use ozmux_tmux::ProjectionModel;` to `use ozmux_tmux::TmuxPresence;` and the run condition:

```rust
/// Run condition: true once the tmux backend is active (the `TmuxPresence`
/// resource exists, inserted at plugin build). The old multiplexer status bar
/// is gated off in tmux mode so only the tmux window bar renders.
pub(crate) fn tmux_projection_present(presence: Option<Res<TmuxPresence>>) -> bool {
    presence.is_some()
}
```

- [ ] **Step 3: Build + test**

Run: `cargo build && cargo test -p ozmux-gui -- status_bar`
Expected: success / PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/tmux_session/src/plugin.rs crates/tmux_session/src/lib.rs src/ui/status_bar_sync.rs
git commit -m "refactor(tmux): gate status bar on TmuxPresence (frame-0), not ProjectionModel"
```

**Checkpoint:** No binary code now reads `ProjectionModel` or `TmuxProjection`. Confirm:

Run: `grep -rn "ProjectionModel\|TmuxProjection\b" src/`
Expected: no matches (only `TmuxProjectionSet` may remain — that is the system set, allowed).

---

# Phase 2 — Internals cutover

## Task 8: `events.rs` — global events + `PaneGeom` + `pane_geoms`

**Files:**
- Create: `crates/tmux_session/src/events.rs`
- Modify: `crates/tmux_session/src/lib.rs` (add `mod events;`)

- [ ] **Step 1: Create the events module**

Create `crates/tmux_session/src/events.rs`:

```rust
//! Global events fired by the drain system and applied by the observers. Each
//! payload carries only tmux-side ids (never an `Entity`); observers resolve
//! ids to entities via the `TmuxProjection` index.

use bevy::prelude::Event;
use tmux_control_parser::{Cell, CellDims, PaneId, SessionId, WindowId, WindowLayout};

/// A pane's tmux id plus its cell geometry, carried in `TmuxLayoutChanged`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PaneGeom {
    /// tmux pane id (`%N`).
    pub(crate) id: PaneId,
    /// Cell geometry from the window layout.
    pub(crate) dims: CellDims,
}

/// `%session-changed`: the attached session and its name.
#[derive(Event, Debug, Clone)]
pub(crate) struct TmuxSessionChanged {
    pub(crate) session: SessionId,
    pub(crate) name: String,
}

/// `%window-add` (defaults) or a seed row (real `index`/`name`).
#[derive(Event, Debug, Clone)]
pub(crate) struct TmuxWindowAdded {
    pub(crate) window: WindowId,
    pub(crate) index: u32,
    pub(crate) name: String,
}

/// `%window-close`.
#[derive(Event, Debug, Clone)]
pub(crate) struct TmuxWindowClosed {
    pub(crate) window: WindowId,
}

/// `%window-renamed`.
#[derive(Event, Debug, Clone)]
pub(crate) struct TmuxWindowRenamed {
    pub(crate) window: WindowId,
    pub(crate) name: String,
}

/// `%layout-change` or a seed row: the window's full pane set.
#[derive(Event, Debug, Clone)]
pub(crate) struct TmuxLayoutChanged {
    pub(crate) window: WindowId,
    pub(crate) panes: Vec<PaneGeom>,
}

/// `%window-pane-changed`: the active pane (and its window).
#[derive(Event, Debug, Clone)]
pub(crate) struct TmuxActivePaneChanged {
    pub(crate) window: WindowId,
    pub(crate) pane: PaneId,
}

/// A seed row's active flag: this window is the active one.
#[derive(Event, Debug, Clone)]
pub(crate) struct TmuxActiveWindowChanged {
    pub(crate) window: WindowId,
}

/// Seed prune: despawn every window not in this set.
#[derive(Event, Debug, Clone)]
pub(crate) struct TmuxWindowsRetained {
    pub(crate) windows: Vec<WindowId>,
}

/// Transport `Closed`: tear the whole projection down.
#[derive(Event, Debug, Clone)]
pub(crate) struct TmuxConnectionReset;

/// Flattens a window layout tree into its panes, in layout order. Leaves with
/// no id (a layout-grammar artifact) are skipped.
pub(crate) fn pane_geoms(layout: &WindowLayout) -> Vec<PaneGeom> {
    let mut out = Vec::new();
    collect_leaves(&layout.root, &mut out);
    out
}

fn collect_leaves(cell: &Cell, out: &mut Vec<PaneGeom>) {
    match cell {
        Cell::Leaf { dims, pane_id } => {
            if let Some(id) = pane_id {
                out.push(PaneGeom {
                    id: PaneId(*id),
                    dims: *dims,
                });
            }
        }
        Cell::Split { children, .. } => {
            for child in children {
                collect_leaves(child, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dims(width: u32, height: u32, xoff: i32, yoff: i32) -> CellDims {
        CellDims { width, height, xoff, yoff }
    }

    #[test]
    fn single_pane_layout_yields_one_pane() {
        let layout = WindowLayout::parse(b"b25f,80x24,0,0,0").unwrap();
        assert_eq!(
            pane_geoms(&layout),
            vec![PaneGeom { id: PaneId(0), dims: dims(80, 24, 0, 0) }]
        );
    }

    #[test]
    fn horizontal_split_yields_two_panes_in_order() {
        let layout = WindowLayout::parse(b"abcd,80x24,0,0{40x24,0,0,1,39x24,41,0,2}").unwrap();
        let panes = pane_geoms(&layout);
        assert_eq!(panes.len(), 2);
        assert_eq!((panes[0].id, panes[1].id), (PaneId(1), PaneId(2)));
        assert_eq!(panes[0].dims, dims(40, 24, 0, 0));
        assert_eq!(panes[1].dims, dims(39, 24, 41, 0));
    }
}
```

> NOTE: `pane_geoms` is the renamed successor of `model::pane_leaves`. The old `pane_leaves` stays until Task 10 deletes `model.rs`.

- [ ] **Step 2: Declare the module**

In `crates/tmux_session/src/lib.rs`, add `mod events;` to the module list (keep alphabetical-ish ordering: after `mod enumerate;`).

- [ ] **Step 3: Run tests**

Run: `cargo test -p ozmux_tmux events`
Expected: PASS (`single_pane_layout_yields_one_pane`, `horizontal_split_yields_two_panes_in_order`).

- [ ] **Step 4: Commit**

```bash
git add crates/tmux_session/src/events.rs crates/tmux_session/src/lib.rs
git commit -m "feat(tmux): add global projection events + PaneGeom/pane_geoms"
```

---

## Task 9: `observers.rs` — index (new shape) + observers, unit-tested, not yet wired

**Files:**
- Create: `crates/tmux_session/src/observers.rs`
- Modify: `crates/tmux_session/src/reconcile.rs` (move `TmuxProjection` out / adopt new shape)
- Modify: `crates/tmux_session/src/lib.rs` (add `mod observers;`)

> The `TmuxProjection` index moves to `observers.rs` with the new shape. `reconcile.rs` keeps using it (updated to the tuple shape) until Task 10 deletes reconcile. Since Task 2 already removed the binary's use of the index, only this crate references it.

- [ ] **Step 1: Create `observers.rs` with the index + observers**

Create `crates/tmux_session/src/observers.rs`:

```rust
//! Observers that apply the global projection events to the ECS world, plus the
//! tmux-id -> entity index they resolve through.

use crate::components::{ActivePane, ActiveWindow, TmuxPane, TmuxSession, TmuxWindow};
use crate::events::{
    PaneGeom, TmuxActivePaneChanged, TmuxActiveWindowChanged, TmuxConnectionReset,
    TmuxLayoutChanged, TmuxSessionChanged, TmuxWindowAdded, TmuxWindowClosed, TmuxWindowRenamed,
    TmuxWindowsRetained,
};
use bevy::prelude::*;
use std::collections::{HashMap, HashSet};
use tmux_control_parser::{PaneId, WindowId};

/// Maps tmux ids to their projected entities. Internal routing state only.
#[derive(Resource, Default)]
pub(crate) struct TmuxProjection {
    pub(crate) windows: HashMap<WindowId, Entity>,
    pub(crate) panes: HashMap<PaneId, (Entity, WindowId)>,
    pub(crate) session: Option<Entity>,
    pub(crate) pending_active_pane: Option<PaneId>,
}

/// Registers every projection observer. Exactly one observer per event type.
pub(crate) fn register_observers(app: &mut App) {
    app.add_observer(on_session_changed)
        .add_observer(on_window_added)
        .add_observer(on_window_renamed)
        .add_observer(on_window_closed)
        .add_observer(on_layout_changed)
        .add_observer(on_active_pane_changed)
        .add_observer(on_active_window_changed)
        .add_observer(on_windows_retained)
        .add_observer(on_connection_reset);
}

fn on_session_changed(
    ev: On<TmuxSessionChanged>,
    mut commands: Commands,
    mut index: ResMut<TmuxProjection>,
) {
    let session = TmuxSession {
        id: ev.session,
        name: ev.name.clone(),
    };
    match index.session {
        Some(e) => {
            commands.entity(e).insert(session);
        }
        None => {
            let e = commands.spawn(session).id();
            index.session = Some(e);
        }
    }
}

fn on_window_added(
    ev: On<TmuxWindowAdded>,
    mut commands: Commands,
    mut index: ResMut<TmuxProjection>,
) {
    match index.windows.get(&ev.window) {
        Some(&e) => {
            if !(ev.index == 0 && ev.name.is_empty()) {
                commands.entity(e).insert(TmuxWindow {
                    id: ev.window,
                    index: ev.index,
                    name: ev.name.clone(),
                });
            }
        }
        None => {
            let e = commands
                .spawn(TmuxWindow {
                    id: ev.window,
                    index: ev.index,
                    name: ev.name.clone(),
                })
                .id();
            index.windows.insert(ev.window, e);
        }
    }
}

fn on_window_renamed(
    ev: On<TmuxWindowRenamed>,
    mut commands: Commands,
    index: Res<TmuxProjection>,
    windows: Query<&TmuxWindow>,
) {
    let Some(&e) = index.windows.get(&ev.window) else {
        return;
    };
    let Ok(w) = windows.get(e) else {
        return;
    };
    commands.entity(e).insert(TmuxWindow {
        id: w.id,
        index: w.index,
        name: ev.name.clone(),
    });
}

fn on_window_closed(
    ev: On<TmuxWindowClosed>,
    mut commands: Commands,
    mut index: ResMut<TmuxProjection>,
) {
    despawn_window(&mut commands, &mut index, ev.window);
}

fn on_layout_changed(
    ev: On<TmuxLayoutChanged>,
    mut commands: Commands,
    mut index: ResMut<TmuxProjection>,
    active_panes: Query<Entity, With<ActivePane>>,
) {
    let window = ensure_window(&mut commands, &mut index, ev.window);

    let live: HashSet<PaneId> = ev.panes.iter().map(|p| p.id).collect();
    let stale: Vec<PaneId> = index
        .panes
        .iter()
        .filter(|(id, (_, w))| *w == ev.window && !live.contains(id))
        .map(|(id, _)| *id)
        .collect();
    for id in stale {
        if let Some((e, _)) = index.panes.remove(&id) {
            commands.entity(e).despawn();
        }
    }

    for geom in &ev.panes {
        upsert_pane(&mut commands, &mut index, window, ev.window, geom);
    }

    apply_pending_active_pane(&mut commands, &mut index, &active_panes);
}

fn on_active_pane_changed(
    ev: On<TmuxActivePaneChanged>,
    mut commands: Commands,
    mut index: ResMut<TmuxProjection>,
    active_windows: Query<Entity, With<ActiveWindow>>,
    active_panes: Query<Entity, With<ActivePane>>,
) {
    let window = ensure_window(&mut commands, &mut index, ev.window);
    set_marker::<ActiveWindow>(&mut commands, &active_windows, window);

    match index.panes.get(&ev.pane) {
        Some(&(e, _)) => {
            set_marker::<ActivePane>(&mut commands, &active_panes, e);
            index.pending_active_pane = None;
        }
        None => {
            index.pending_active_pane = Some(ev.pane);
        }
    }
}

fn on_active_window_changed(
    ev: On<TmuxActiveWindowChanged>,
    mut commands: Commands,
    mut index: ResMut<TmuxProjection>,
    active_windows: Query<Entity, With<ActiveWindow>>,
) {
    let window = ensure_window(&mut commands, &mut index, ev.window);
    set_marker::<ActiveWindow>(&mut commands, &active_windows, window);
}

fn on_windows_retained(
    ev: On<TmuxWindowsRetained>,
    mut commands: Commands,
    mut index: ResMut<TmuxProjection>,
) {
    let keep: HashSet<WindowId> = ev.windows.iter().copied().collect();
    let drop_ids: Vec<WindowId> = index
        .windows
        .keys()
        .copied()
        .filter(|id| !keep.contains(id))
        .collect();
    for id in drop_ids {
        despawn_window(&mut commands, &mut index, id);
    }
}

fn on_connection_reset(
    _ev: On<TmuxConnectionReset>,
    mut commands: Commands,
    mut index: ResMut<TmuxProjection>,
) {
    for (_, e) in index.windows.drain() {
        commands.entity(e).despawn();
    }
    index.panes.clear();
    if let Some(e) = index.session.take() {
        commands.entity(e).despawn();
    }
    index.pending_active_pane = None;
}

fn ensure_window(commands: &mut Commands, index: &mut TmuxProjection, id: WindowId) -> Entity {
    if let Some(&e) = index.windows.get(&id) {
        return e;
    }
    let e = commands
        .spawn(TmuxWindow {
            id,
            index: 0,
            name: String::new(),
        })
        .id();
    index.windows.insert(id, e);
    e
}

fn upsert_pane(
    commands: &mut Commands,
    index: &mut TmuxProjection,
    window: Entity,
    window_id: WindowId,
    geom: &PaneGeom,
) {
    let pane = TmuxPane {
        id: geom.id,
        dims: geom.dims,
    };
    match index.panes.get(&geom.id) {
        Some(&(e, _)) => {
            commands.entity(e).insert(pane);
        }
        None => {
            let e = commands.spawn((pane, ChildOf(window))).id();
            index.panes.insert(geom.id, (e, window_id));
        }
    }
}

fn apply_pending_active_pane(
    commands: &mut Commands,
    index: &mut TmuxProjection,
    active_panes: &Query<Entity, With<ActivePane>>,
) {
    let Some(pending) = index.pending_active_pane else {
        return;
    };
    if let Some(&(e, _)) = index.panes.get(&pending) {
        set_marker::<ActivePane>(commands, active_panes, e);
        index.pending_active_pane = None;
    }
}

// NOTE: prune the index for the window's panes here; the window despawn cascades
// to its ChildOf pane entities, so the pane entities must NOT be despawned again.
fn despawn_window(commands: &mut Commands, index: &mut TmuxProjection, id: WindowId) {
    let Some(e) = index.windows.remove(&id) else {
        return;
    };
    index.panes.retain(|_, (_, w)| *w != id);
    commands.entity(e).despawn();
}

fn set_marker<T: Component + Default>(
    commands: &mut Commands,
    holders: &Query<Entity, With<T>>,
    target: Entity,
) {
    for e in holders.iter() {
        commands.entity(e).remove::<T>();
    }
    commands.entity(target).insert(T::default());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::pane_geoms;
    use tmux_control_parser::{SessionId, WindowLayout};

    fn app() -> App {
        let mut app = App::new();
        app.init_resource::<TmuxProjection>();
        register_observers(&mut app);
        app
    }

    fn layout(spec: &[u8]) -> WindowLayout {
        WindowLayout::parse(spec).unwrap()
    }

    #[test]
    fn window_added_then_layout_spawns_window_and_panes() {
        let mut app = app();
        app.world_mut().trigger(TmuxWindowAdded {
            window: WindowId(1),
            index: 0,
            name: "w".into(),
        });
        app.world_mut().trigger(TmuxLayoutChanged {
            window: WindowId(1),
            panes: pane_geoms(&layout(b"abcd,80x24,0,0{40x24,0,0,1,39x24,41,0,2}")),
        });
        app.update();

        let index = app.world().resource::<TmuxProjection>();
        assert_eq!(index.windows.len(), 1);
        assert_eq!(index.panes.len(), 2);
        let (pane_e, w) = index.panes[&PaneId(1)];
        assert_eq!(w, WindowId(1));
        assert_eq!(app.world().get::<TmuxPane>(pane_e).unwrap().id, PaneId(1));
    }

    #[test]
    fn active_pane_before_layout_is_applied_when_pane_appears() {
        let mut app = app();
        // Active pane arrives before the layout that creates it.
        app.world_mut().trigger(TmuxActivePaneChanged {
            window: WindowId(1),
            pane: PaneId(5),
        });
        app.update();
        assert_eq!(
            app.world().resource::<TmuxProjection>().pending_active_pane,
            Some(PaneId(5))
        );

        app.world_mut().trigger(TmuxLayoutChanged {
            window: WindowId(1),
            panes: pane_geoms(&layout(b"abcd,80x24,0,0,5")),
        });
        app.update();

        let index = app.world().resource::<TmuxProjection>();
        assert_eq!(index.pending_active_pane, None);
        let (pane_e, _) = index.panes[&PaneId(5)];
        assert!(app.world().get::<ActivePane>(pane_e).is_some());
    }

    #[test]
    fn window_close_despawns_window_and_prunes_panes() {
        let mut app = app();
        app.world_mut().trigger(TmuxWindowAdded { window: WindowId(1), index: 0, name: "w".into() });
        app.world_mut().trigger(TmuxLayoutChanged {
            window: WindowId(1),
            panes: pane_geoms(&layout(b"abcd,80x24,0,0,9")),
        });
        app.update();
        app.world_mut().trigger(TmuxWindowClosed { window: WindowId(1) });
        app.update();

        let index = app.world().resource::<TmuxProjection>();
        assert!(index.windows.is_empty());
        assert!(index.panes.is_empty());
    }

    #[test]
    fn windows_retained_prunes_absent_windows() {
        let mut app = app();
        for id in [1u16, 2, 3] {
            app.world_mut().trigger(TmuxWindowAdded { window: WindowId(id), index: 0, name: "w".into() });
        }
        app.update();
        app.world_mut().trigger(TmuxWindowsRetained { windows: vec![WindowId(2)] });
        app.update();

        let index = app.world().resource::<TmuxProjection>();
        assert_eq!(index.windows.keys().copied().collect::<Vec<_>>(), vec![WindowId(2)]);
    }

    #[test]
    fn active_markers_are_singletons() {
        let mut app = app();
        app.world_mut().trigger(TmuxActiveWindowChanged { window: WindowId(1) });
        app.world_mut().trigger(TmuxActiveWindowChanged { window: WindowId(2) });
        app.update();

        let mut q = app.world_mut().query_filtered::<Entity, With<ActiveWindow>>();
        assert_eq!(q.iter(app.world()).count(), 1);
    }

    #[test]
    fn session_changed_sets_id_and_name() {
        let mut app = app();
        app.world_mut().trigger(TmuxSessionChanged { session: SessionId(7), name: "main".into() });
        app.update();
        let e = app.world().resource::<TmuxProjection>().session.unwrap();
        let s = app.world().get::<TmuxSession>(e).unwrap();
        assert_eq!((s.id, s.name.as_str()), (SessionId(7), "main"));
    }

    #[test]
    fn connection_reset_clears_everything() {
        let mut app = app();
        app.world_mut().trigger(TmuxSessionChanged { session: SessionId(1), name: "m".into() });
        app.world_mut().trigger(TmuxWindowAdded { window: WindowId(1), index: 0, name: "w".into() });
        app.world_mut().trigger(TmuxLayoutChanged {
            window: WindowId(1),
            panes: pane_geoms(&layout(b"abcd,80x24,0,0,1")),
        });
        app.update();
        app.world_mut().trigger(TmuxConnectionReset);
        app.update();

        let index = app.world().resource::<TmuxProjection>();
        assert!(index.windows.is_empty() && index.panes.is_empty() && index.session.is_none());
    }
}
```

- [ ] **Step 2: Move the index out of `reconcile.rs` and adopt the new shape**

In `crates/tmux_session/src/reconcile.rs`:
- Delete the `TmuxProjection` struct definition (it now lives in `observers.rs`).
- Add `use crate::observers::TmuxProjection;` to the top `use` block.
- Update every `index.panes` access for the new `(Entity, WindowId)` value:
  - In `reconcile_windows`, the pane retain closure:
    ```rust
    index.panes.retain(|id, (entity, _)| {
        let keep = live_panes.contains(id);
        if !keep {
            commands.entity(*entity).despawn();
        }
        keep
    });
    ```
  - The pane upsert match arms:
    ```rust
    match index.panes.get(&pane.id) {
        Some(&(entity, _)) => {
            commands.entity(entity).insert((
                TmuxPane { id: pane.id, dims: pane.dims },
                ChildOf(window_entity),
            ));
        }
        None => {
            let entity = commands
                .spawn((
                    TmuxPane { id: pane.id, dims: pane.dims },
                    ChildOf(window_entity),
                ))
                .id();
            index.panes.insert(pane.id, (entity, window.id));
        }
    }
    ```
- In the reconcile `tests` module, update assertions that read `index.panes[&PaneId(..)]` to destructure the tuple: e.g. `let (pane_entity, _) = index.panes[&PaneId(9)];`. Apply to `spawns_window_and_pane_entities` and `pane_is_child_of_its_window`.

- [ ] **Step 3: Declare the module**

In `crates/tmux_session/src/lib.rs`, add `mod observers;` (after `mod model;`). Do NOT export `TmuxProjection` (it stays crate-private). Remove the `pub use reconcile::TmuxProjection;` line.

> NOTE: `src/tmux_render.rs` no longer imports `TmuxProjection` (Task 2), so dropping the export does not break the binary.

- [ ] **Step 4: Run the crate tests**

Run: `cargo test -p ozmux_tmux`
Expected: PASS (observer tests + updated reconcile tests + everything else). Then build the binary:

Run: `cargo build`
Expected: success.

- [ ] **Step 5: Commit**

```bash
git add crates/tmux_session/src/observers.rs crates/tmux_session/src/reconcile.rs crates/tmux_session/src/lib.rs
git commit -m "feat(tmux): add projection observers + relocate index (new shape), unit-tested"
```

---

## Task 10: Cutover — drain triggers events; delete model + reconcile

**Files:**
- Modify: `crates/tmux_session/src/plugin.rs` (drain triggers; register observers; drop reconcile)
- Modify: `crates/tmux_session/src/event_pump.rs` (drop `apply_events`/`seed_from_reply` + tests; add `trigger_events`)
- Modify: `crates/tmux_session/src/components.rs` (remove `TmuxWindow.active`)
- Delete: `crates/tmux_session/src/model.rs`, `crates/tmux_session/src/reconcile.rs`
- Modify: `crates/tmux_session/src/lib.rs` (remove model/reconcile exports + modules)

- [ ] **Step 1: Add `trigger_events` to `event_pump.rs`**

In `crates/tmux_session/src/event_pump.rs`, add the translator (keep `drain_transport`, `advance_state`, `take_client_name`, `log_transport_event`; delete `apply_events` and `seed_from_reply` and their tests). Add imports for the events + `pane_geoms` + `parse_window_rows` and `Commands`:

```rust
use crate::enumerate::parse_window_rows;
use crate::events::{
    pane_geoms, TmuxActivePaneChanged, TmuxActiveWindowChanged, TmuxConnectionReset,
    TmuxLayoutChanged, TmuxSessionChanged, TmuxWindowAdded, TmuxWindowClosed, TmuxWindowRenamed,
    TmuxWindowsRetained,
};
use bevy::prelude::Commands;
use crate::state::{ConnectionState, next_state};
use crossbeam_channel::Receiver;
use tmux_control::{ClientEvent, CommandId, ControlEvent, TransportEvent};
```

(Merge with the existing `use` block; keep it a single contiguous block.)

Add the function:

```rust
/// Translates a drained transport batch into global projection events, in
/// stream order, triggering each via `commands`. The enumeration reply (the
/// `CommandComplete` whose id matches `pending`) is decomposed into per-row
/// `TmuxWindowAdded` + `TmuxLayoutChanged` (+ `TmuxActiveWindowChanged` for the
/// active row), followed by one `TmuxWindowsRetained` prune. Untracked events
/// (e.g. `%output`) are ignored here (routed separately as `PaneOutput`).
pub(crate) fn trigger_events(
    commands: &mut Commands,
    pending: &mut Option<CommandId>,
    events: &[TransportEvent],
) {
    for event in events {
        match event {
            TransportEvent::Protocol(ClientEvent::Notification(notification)) => {
                trigger_notification(commands, notification);
            }
            TransportEvent::Protocol(ClientEvent::CommandComplete { id, ok, output, .. })
                if *pending == Some(*id) =>
            {
                *pending = None;
                if *ok {
                    trigger_seed(commands, output);
                } else {
                    tracing::warn!("list-windows enumeration command failed");
                }
            }
            _ => {}
        }
    }
}

fn trigger_notification(commands: &mut Commands, event: &ControlEvent) {
    match event {
        ControlEvent::SessionChanged { session, name } => {
            commands.trigger(TmuxSessionChanged { session: *session, name: name.clone() });
        }
        ControlEvent::WindowAdd { window } => {
            commands.trigger(TmuxWindowAdded { window: *window, index: 0, name: String::new() });
        }
        ControlEvent::WindowClose { window } => {
            commands.trigger(TmuxWindowClosed { window: *window });
        }
        ControlEvent::WindowRenamed { window, name } => {
            commands.trigger(TmuxWindowRenamed { window: *window, name: name.clone() });
        }
        ControlEvent::LayoutChange { window, layout, .. } => {
            commands.trigger(TmuxLayoutChanged { window: *window, panes: pane_geoms(layout) });
        }
        ControlEvent::WindowPaneChanged { window, pane } => {
            commands.trigger(TmuxActivePaneChanged { window: *window, pane: *pane });
        }
        _ => {}
    }
}

fn trigger_seed(commands: &mut Commands, output: &[String]) {
    let rows = match parse_window_rows(output) {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!(error = %error, "failed to parse list-windows reply");
            return;
        }
    };
    let mut ids = Vec::with_capacity(rows.len());
    for row in &rows {
        commands.trigger(TmuxWindowAdded {
            window: row.id,
            index: row.index,
            name: row.name.clone(),
        });
        commands.trigger(TmuxLayoutChanged {
            window: row.id,
            panes: pane_geoms(&row.layout),
        });
        if row.active {
            commands.trigger(TmuxActiveWindowChanged { window: row.id });
        }
        ids.push(row.id);
    }
    commands.trigger(TmuxWindowsRetained { windows: ids });
}
```

Delete the now-obsolete tests in `event_pump.rs` (`apply_events_*`, `seed_from_reply_*`, `apply_events_applies_notifications`, `output_only_batch_reports_no_model_change`) and the `ProjectionModel` import. Keep `drain_then_advance_state_attaches`, `take_client_name_*`. Add a translator test:

```rust
#[test]
fn seed_reply_triggers_per_row_events_then_retain() {
    use crate::events::{TmuxLayoutChanged, TmuxWindowAdded, TmuxWindowsRetained};
    use std::sync::{Arc, Mutex};

    #[derive(Resource, Default, Clone)]
    struct Log(Arc<Mutex<Vec<String>>>);

    #[derive(Resource)]
    struct Batch(Vec<TransportEvent>);

    fn run(mut commands: Commands, mut enumeration: ResMut<EnumerationState>, batch: Res<Batch>) {
        trigger_events(&mut commands, &mut enumeration.pending, &batch.0);
    }

    let mut app = App::new();
    app.init_resource::<Log>();
    app.init_resource::<EnumerationState>();
    app.world_mut().resource_mut::<EnumerationState>().pending = Some(CommandId(1));
    app.insert_resource(Batch(vec![TransportEvent::Protocol(
        ClientEvent::CommandComplete {
            id: CommandId(1),
            number: 0,
            ok: true,
            output: vec!["1\t@1\t0\tabcd,80x24,0,0,5\tx\tmain".to_string()],
        },
    )]));
    app.add_observer(|ev: On<TmuxWindowAdded>, log: Res<Log>| {
        log.0.lock().unwrap().push(format!("add@{}", ev.window.0));
    });
    app.add_observer(|ev: On<TmuxLayoutChanged>, log: Res<Log>| {
        log.0.lock().unwrap().push(format!("layout@{}", ev.window.0));
    });
    app.add_observer(|ev: On<TmuxWindowsRetained>, log: Res<Log>| {
        log.0.lock().unwrap().push(format!("retain{}", ev.windows.len()));
    });
    app.add_systems(Update, run);

    let log = app.world().resource::<Log>().clone();
    app.update();

    assert_eq!(*log.0.lock().unwrap(), vec!["add@1", "layout@1", "retain1"]);
    assert_eq!(app.world().resource::<EnumerationState>().pending, None);
}
```

> NOTE: the contract is the trigger ORDER (`add → layout → retain`) and that `pending` is cleared once the matching reply is consumed. Driving `trigger_events` from a one-frame system keeps the `Commands`/observer FIFO semantics identical to production.

- [ ] **Step 2: Run the event_pump tests**

Run: `cargo test -p ozmux_tmux event_pump`
Expected: PASS (translator order test + retained kept tests).

- [ ] **Step 3: Rewrite `plugin.rs` drain to trigger + register observers**

In `crates/tmux_session/src/plugin.rs`:
- Imports: drop `ProjectionModel`, `apply_events`, `reconcile`. Add `use crate::event_pump::{advance_state, drain_transport, take_client_name, trigger_events};`, `use crate::observers::{register_observers, TmuxProjection};`.
- In `build`: replace `.init_resource::<ProjectionModel>()` with nothing (the model is gone), keep `.init_resource::<TmuxProjection>()`, keep `TmuxPresence`. Register observers and the single drain system:

```rust
        register_observers(app);
        app.init_resource::<ConnectionState>()
            .init_resource::<TmuxProjection>()
            .init_resource::<EnumerationState>()
            .insert_resource(TmuxPresence)
            .insert_non_send_resource(TmuxConnection::default())
            .add_message::<PaneOutput>()
            .add_systems(Update, drain_tmux_events.in_set(TmuxProjectionSet));
```

- Rewrite `drain_tmux_events` to trigger events (no model). Replace the `model`-related body. The function takes `Commands` (mutable, first), drops the `model` param:

```rust
fn drain_tmux_events(
    mut commands: Commands,
    mut state: ResMut<ConnectionState>,
    mut enumeration: ResMut<EnumerationState>,
    mut connection: NonSendMut<TmuxConnection>,
    mut pane_output: MessageWriter<PaneOutput>,
) {
    let events = match connection.client() {
        Some(client) => drain_transport(client.events()),
        None => return,
    };
    if events.is_empty() {
        return;
    }
    for output in collect_pane_outputs(&events) {
        pane_output.write(output);
    }
    if advance_state(&mut state, &events)
        && matches!(*state, ConnectionState::Attached)
        && let Some(client) = connection.client()
    {
        match client.handle().send(&list_windows_command()) {
            Ok(id) => enumeration.pending = Some(id),
            Err(error) => tracing::warn!(?error, "failed to send list-windows enumeration"),
        }
        match client.handle().send(&client_name_command()) {
            Ok(id) => enumeration.client_name_pending = Some(id),
            Err(error) => tracing::warn!(?error, "failed to send client-name query"),
        }
    }
    if events
        .iter()
        .any(|event| matches!(event, TransportEvent::Closed { .. }))
    {
        connection.take();
        enumeration.pending = None;
        enumeration.client_name_pending = None;
        commands.trigger(TmuxConnectionReset);
    } else {
        if let Some(name) = take_client_name(&mut enumeration.client_name_pending, &events) {
            connection.set_client_name(name);
        }
        trigger_events(&mut commands, &mut enumeration.pending, &events);
    }
    // NOTE: runs after the Closed branch took the connection, so `client()` is
    // None there and this is a no-op — safe to re-arm only while still attached.
    if matches!(*state, ConnectionState::Attached)
        && connection.client_name().is_none()
        && enumeration.client_name_pending.is_none()
        && let Some(client) = connection.client()
    {
        match client.handle().send(&client_name_command()) {
            Ok(id) => enumeration.client_name_pending = Some(id),
            Err(error) => tracing::warn!(?error, "failed to re-send client-name query"),
        }
    }
}
```

Add `use crate::events::TmuxConnectionReset;` and ensure `collect_pane_outputs`, `client_name_command`, `list_windows_command`, `TransportEvent` are imported. Update the plugin doc comment to drop "projection model".

> NOTE: `advance_state` now takes `&mut state` directly (change detection on `ConnectionState` no longer needs suppression — it changes rarely). Verify `advance_state(state: &mut ConnectionState, ...)` signature is unchanged; pass `&mut state`.

- [ ] **Step 4: Remove `TmuxWindow.active` and delete model + reconcile**

In `crates/tmux_session/src/components.rs`, remove the `active` field from `TmuxWindow`:

```rust
/// A projected tmux window entity.
#[derive(Component, Debug, Clone, PartialEq, Eq)]
pub struct TmuxWindow {
    /// tmux window id (`@N`).
    pub id: WindowId,
    /// tmux display index (#{window_index}).
    pub index: u32,
    /// Window name.
    pub name: String,
}
```

Delete the files:

```bash
git rm crates/tmux_session/src/model.rs crates/tmux_session/src/reconcile.rs
```

In `crates/tmux_session/src/lib.rs`:
- Remove `mod model;` and `mod reconcile;`.
- Remove `pub use model::{PaneModel, ProjectionModel, WindowModel, pane_leaves};`.
- Confirm there is no `pub use reconcile::…;` line (removed in Task 9).
- Update the crate `//!` doc comment to describe the event/observer design instead of `ProjectionModel`.

- [ ] **Step 5: Build the workspace**

Run: `cargo build`
Expected: success. If the compiler flags a leftover reference to `ProjectionModel`/`pane_leaves`/`reconcile`/`TmuxWindow.active`, fix it at the cited site (there should be none — all consumers migrated in Phase 1).

- [ ] **Step 6: Run all crate + binary tests**

Run: `cargo test -p ozmux_tmux && cargo test -p ozmux-gui`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add -A crates/tmux_session/src
git commit -m "refactor(tmux): cut over drain to event triggers; delete ProjectionModel + reconcile"
```

---

## Task 11: Update the tmux-gated integration tests

**Files:**
- Modify: `crates/tmux_session/tests/real_tmux*.rs`

- [ ] **Step 1: Find references to deleted APIs**

Run: `grep -rln "ProjectionModel\|WindowModel\|PaneModel\|pane_leaves\|\.active\b\|TmuxProjection" crates/tmux_session/tests`
Expected: a list of integration tests asserting against the old model.

- [ ] **Step 2: Rewrite assertions to use entities + markers**

For each match, replace model/index assertions with entity queries. Patterns:
- "session present" → `app.world_mut().query::<&TmuxSession>()` finds one with the expected `id`/`name`.
- "windows enumerated" → `query::<&TmuxWindow>()` count + ids.
- "active window/pane" → `query_filtered::<&TmuxWindow, With<ActiveWindow>>()` / `query_filtered::<&TmuxPane, With<ActivePane>>()`.
- "pane geometry" → `query::<&TmuxPane>()` dims.

These tests are gated behind a real `tmux` binary (see each file's `#[ignore]` / cfg gate); keep that gating. Use a polling helper if the file already has one (drain runs over frames).

- [ ] **Step 3: Run the gated tests if `tmux` is available**

Run: `cargo test -p ozmux_tmux --test real_tmux -- --ignored` (and the other `real_tmux_*` test binaries) when `tmux` is installed; otherwise confirm they compile:
Run: `cargo test -p ozmux_tmux --no-run`
Expected: compiles; gated tests pass where `tmux` is present.

- [ ] **Step 4: Commit**

```bash
git add crates/tmux_session/tests
git commit -m "test(tmux): assert real-tmux integration against entities + markers"
```

---

## Task 12: Final lint, format, and full-suite verification

- [ ] **Step 1: Auto-fix lints and format**

Run: `cargo clippy --workspace --fix --allow-dirty --allow-staged && cargo fmt`
Expected: clean (resolve any remaining warnings — e.g. unused imports left by the migration).

- [ ] **Step 2: Manual rule review (not lint-enforced)**

Confirm by inspection of the touched files: every module file starts with `//!`; every `pub` item has `///`; no `mod.rs`; comments are only `// TODO:`/`// NOTE:`/`// SAFETY:`; `use` blocks are single contiguous blocks; mutable params precede immutable in new signatures; private fns are last in their blocks; `TmuxProjection` is crate-private; events are `pub(crate)`.

- [ ] **Step 3: Full workspace test**

Run: `cargo test`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "chore(tmux): clippy/fmt + final review for event-driven projection"
```

---

## Done criteria

- `ProjectionModel`, `WindowModel`, `PaneModel`, `pane_leaves`, `reconcile_projection`, and `model.rs`/`reconcile.rs` no longer exist.
- `drain_tmux_events` only drains, advances `ConnectionState`, writes `PaneOutput`, sends enumeration commands, and `commands.trigger`s events — no `bypass_change_detection`/`set_changed`.
- Observers are the sole writers of `TmuxSession`/`TmuxWindow`/`TmuxPane`/`ActivePane`/`ActiveWindow`; `TmuxProjection` is crate-private with the `(Entity, WindowId)` pane map + `pending_active_pane`.
- Every consumer reads components/markers; `grep -rn "ProjectionModel" src/ crates/` returns nothing.
- `cargo test` passes; `cargo clippy --workspace` is clean.
