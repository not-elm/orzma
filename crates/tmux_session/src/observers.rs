//! Observers that apply the global projection events to the ECS world, plus the
//! tmux-id -> entity index they resolve through.

use crate::components::{
    ActivePane, ActiveWindow, TmuxPane, TmuxSession, TmuxWindow, TmuxWindowLayout, WindowFlags,
};
use crate::copy_queries::CopyModeQueries;
use crate::enumerate::EnumerationState;
use crate::events::{
    PaneGeom, TmuxActivePaneChanged, TmuxActiveWindowChanged, TmuxConnectionReset,
    TmuxLayoutChanged, TmuxSessionChanged, TmuxWindowAdded, TmuxWindowClosed,
    TmuxWindowFlagsChanged, TmuxWindowRenamed, TmuxWindowsRetained, pane_geoms,
};
use crate::keybindings::KeyBindings;
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
        .add_observer(on_window_flags_changed)
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

fn on_window_flags_changed(
    ev: On<TmuxWindowFlagsChanged>,
    mut commands: Commands,
    index: Res<TmuxProjection>,
    flags: Query<&WindowFlags>,
) {
    let Some(&e) = index.windows.get(&ev.window) else {
        return;
    };
    // NOTE: only insert when the value actually changes, so Changed<WindowFlags>
    // fires on real changes (not every subscription tick).
    if flags.get(e) == Ok(&ev.flags) {
        return;
    }
    commands.entity(e).insert(ev.flags);
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

    commands
        .entity(window)
        .insert(TmuxWindowLayout(ev.layout.clone()));

    let panes = pane_geoms(&ev.layout);
    let live: HashSet<PaneId> = panes.iter().map(|p| p.id).collect();
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
    for geom in &panes {
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
    // NOTE: tmux broadcasts %window-pane-changed to ALL control clients
    // server-wide (no session guard), so a foreign client's pane focus in
    // another session must not spawn a phantom window here. Only the trusted
    // own-session active-pane query reply (from_notification == false) may seed
    // a window; an unknown window from a live notification is foreign — ignore it.
    if ev.from_notification && !index.windows.contains_key(&ev.window) {
        return;
    }
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
    mut enumeration: ResMut<EnumerationState>,
    mut keybindings: ResMut<KeyBindings>,
    mut copy_queries: ResMut<CopyModeQueries>,
) {
    // NOTE: try_despawn for the window entities only. They are reparented
    // ChildOf the WorkspaceUiRoot subtree, so on a mode exit DespawnOnExit(Tmux)
    // may have already removed them and plain despawn would warn per already-gone
    // entity. The session entity below is NOT under that subtree, so it keeps
    // plain despawn — a warning there would flag a genuine index/world desync.
    for (_, e) in index.windows.drain() {
        commands.entity(e).try_despawn();
    }
    index.panes.clear();
    if let Some(e) = index.session.take() {
        commands.entity(e).despawn();
    }
    index.pending_active_pane = None;
    *enumeration = EnumerationState::default();
    keybindings.clear();
    copy_queries.clear();
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
    // NOTE: re-assert ChildOf(window) on every upsert, not just on spawn — a pane
    // tmux moves between windows (break/join-pane) reappears in the new window's
    // layout, and without re-parenting it would keep a stale parent (rendering
    // under the wrong window or as a UI root). Refresh the stored window id too.
    let entity = match index.panes.get(&geom.id).map(|&(e, _)| e) {
        Some(e) => {
            commands.entity(e).insert((pane, ChildOf(window)));
            e
        }
        None => commands.spawn((pane, ChildOf(window))).id(),
    };
    index.panes.insert(geom.id, (entity, window_id));
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
    use tmux_control_parser::{SessionId, WindowLayout};

    fn app() -> App {
        let mut app = App::new();
        app.init_resource::<TmuxProjection>()
            .init_resource::<EnumerationState>()
            .init_resource::<KeyBindings>()
            .init_resource::<CopyModeQueries>();
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
            layout: layout(b"abcd,80x24,0,0{40x24,0,0,1,39x24,41,0,2}"),
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
        app.world_mut().trigger(TmuxActivePaneChanged {
            window: WindowId(1),
            pane: PaneId(5),
            from_notification: false,
        });
        app.update();
        assert_eq!(
            app.world().resource::<TmuxProjection>().pending_active_pane,
            Some(PaneId(5))
        );

        app.world_mut().trigger(TmuxLayoutChanged {
            window: WindowId(1),
            layout: layout(b"abcd,80x24,0,0,5"),
        });
        app.update();

        let index = app.world().resource::<TmuxProjection>();
        assert_eq!(index.pending_active_pane, None);
        let (pane_e, _) = index.panes[&PaneId(5)];
        assert!(app.world().get::<ActivePane>(pane_e).is_some());
    }

    #[test]
    fn foreign_notification_active_pane_does_not_spawn_window() {
        let mut app = app();
        // A live %window-pane-changed (from_notification: true) for a window not
        // in this session's projection is foreign (tmux broadcasts it
        // server-wide) and must be ignored: no phantom window, no ActiveWindow
        // hijack, no pending active pane.
        app.world_mut().trigger(TmuxActivePaneChanged {
            window: WindowId(7),
            pane: PaneId(3),
            from_notification: true,
        });
        app.update();

        let index = app.world().resource::<TmuxProjection>();
        assert!(
            index.windows.is_empty(),
            "foreign notification must not spawn a window"
        );
        assert_eq!(index.pending_active_pane, None);
        let mut active = app
            .world_mut()
            .query_filtered::<Entity, With<ActiveWindow>>();
        assert_eq!(
            active.iter(app.world()).count(),
            0,
            "foreign notification must not set the ActiveWindow marker"
        );
    }

    #[test]
    fn window_close_despawns_window_and_prunes_panes() {
        let mut app = app();
        app.world_mut().trigger(TmuxWindowAdded {
            window: WindowId(1),
            index: 0,
            name: "w".into(),
        });
        app.world_mut().trigger(TmuxLayoutChanged {
            window: WindowId(1),
            layout: layout(b"abcd,80x24,0,0,9"),
        });
        app.update();
        app.world_mut().trigger(TmuxWindowClosed {
            window: WindowId(1),
        });
        app.update();

        let index = app.world().resource::<TmuxProjection>();
        assert!(index.windows.is_empty());
        assert!(index.panes.is_empty());
    }

    #[test]
    fn windows_retained_prunes_absent_windows() {
        let mut app = app();
        for id in [1u32, 2, 3] {
            app.world_mut().trigger(TmuxWindowAdded {
                window: WindowId(id),
                index: 0,
                name: "w".into(),
            });
        }
        app.update();
        app.world_mut().trigger(TmuxWindowsRetained {
            windows: vec![WindowId(2)],
        });
        app.update();

        let index = app.world().resource::<TmuxProjection>();
        assert_eq!(
            index.windows.keys().copied().collect::<Vec<_>>(),
            vec![WindowId(2)]
        );
    }

    #[test]
    fn active_markers_are_singletons() {
        let mut app = app();
        app.world_mut().trigger(TmuxActiveWindowChanged {
            window: WindowId(1),
        });
        // NOTE: flush is required so the first observer's deferred commands (inserting
        // ActiveWindow on entity 1) are applied before the second trigger runs; without
        // it the second observer's Query<With<ActiveWindow>> sees no holders and both
        // entities end up with the marker.
        app.world_mut().flush();
        app.world_mut().trigger(TmuxActiveWindowChanged {
            window: WindowId(2),
        });
        app.update();

        let mut q = app
            .world_mut()
            .query_filtered::<Entity, With<ActiveWindow>>();
        assert_eq!(q.iter(app.world()).count(), 1);
    }

    #[test]
    fn session_changed_sets_id_and_name() {
        let mut app = app();
        app.world_mut().trigger(TmuxSessionChanged {
            session: SessionId(7),
            name: "main".into(),
        });
        app.update();
        let e = app.world().resource::<TmuxProjection>().session.unwrap();
        let s = app.world().get::<TmuxSession>(e).unwrap();
        assert_eq!((s.id, s.name.as_str()), (SessionId(7), "main"));
    }

    #[test]
    fn window_flags_changed_inserts_flags_on_existing_window() {
        use crate::components::WindowFlags;
        use crate::events::TmuxWindowFlagsChanged;

        let mut app = app();
        app.world_mut().trigger(TmuxWindowAdded {
            window: WindowId(1),
            index: 0,
            name: "w".into(),
        });
        app.world_mut().flush();
        app.world_mut().trigger(TmuxWindowFlagsChanged {
            window: WindowId(1),
            flags: WindowFlags::BELL,
        });
        app.update();

        let index = app.world().resource::<TmuxProjection>();
        let e = index.windows[&WindowId(1)];
        assert_eq!(
            app.world().get::<WindowFlags>(e).copied(),
            Some(WindowFlags::BELL)
        );
    }

    #[test]
    fn window_flags_changed_for_unknown_window_is_noop() {
        use crate::components::WindowFlags;
        use crate::events::TmuxWindowFlagsChanged;

        let mut app = app();
        app.world_mut().trigger(TmuxWindowFlagsChanged {
            window: WindowId(99),
            flags: WindowFlags::ZOOM,
        });
        app.update();
        assert!(app.world().resource::<TmuxProjection>().windows.is_empty());
    }

    #[test]
    fn connection_reset_clears_everything() {
        let mut app = app();
        app.world_mut().trigger(TmuxSessionChanged {
            session: SessionId(1),
            name: "m".into(),
        });
        app.world_mut().trigger(TmuxWindowAdded {
            window: WindowId(1),
            index: 0,
            name: "w".into(),
        });
        app.world_mut().trigger(TmuxLayoutChanged {
            window: WindowId(1),
            layout: layout(b"abcd,80x24,0,0,1"),
        });
        app.update();
        app.world_mut().trigger(TmuxConnectionReset);
        app.update();

        let index = app.world().resource::<TmuxProjection>();
        assert!(index.windows.is_empty() && index.panes.is_empty() && index.session.is_none());
    }

    #[test]
    fn layout_change_attaches_window_layout_component() {
        use crate::components::TmuxWindowLayout;
        let mut app = app();
        app.world_mut().trigger(TmuxWindowAdded {
            window: WindowId(1),
            index: 0,
            name: "w".into(),
        });
        app.world_mut().trigger(TmuxLayoutChanged {
            window: WindowId(1),
            layout: layout(b"abcd,80x24,0,0{40x24,0,0,1,39x24,41,0,2}"),
        });
        app.update();

        let index = app.world().resource::<TmuxProjection>();
        let window_e = index.windows[&WindowId(1)];
        assert!(
            app.world().get::<TmuxWindowLayout>(window_e).is_some(),
            "window carries its layout tree after %layout-change",
        );
    }
}
