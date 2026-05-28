//! Lifecycle observers for the multiplexer. Both fire on Bevy's
//! `On<Remove, _>` lifecycle event, which runs *before* the component is
//! actually removed — so the removed component's value is still readable
//! on the triggering entity.
//!
//! Observer registration happens in `MultiplexerPlugin::build`.

use bevy::prelude::*;
use crate::components::{
    ActiveActivity, ActivePane, ActivityMarker, LayoutCells, PaneMarker, SessionMarker,
};

/// When a Pane is despawned, any Session whose `ActivePane(Entity)`
/// pointed at it must be repointed. Otherwise the field would dangle
/// and downstream systems would dereference a freed entity.
///
/// The observer reads the removed Pane's parent Session via `ChildOf`
/// (still valid in the pre-removal observer window), then uses
/// `LayoutCells::ordered_pane_cells` to pick a survivor from the remaining
/// pane cells.
pub fn on_remove_pane_marker(
    ev: On<Remove, PaneMarker>,
    panes: Query<&ChildOf, With<PaneMarker>>,
    mut sessions: Query<(&LayoutCells, &mut ActivePane), With<SessionMarker>>,
) {
    let pane = ev.entity;
    let Ok(child_of) = panes.get(pane) else {
        return;
    };
    let session = child_of.parent();
    let Ok((cells, mut active)) = sessions.get_mut(session) else {
        return;
    };
    if active.0 != pane {
        return;
    }
    let panes_in_layout = cells.cells.ordered_pane_cells(&cells.root).unwrap_or_default();
    if let Some((_, survivor)) = panes_in_layout.into_iter().find(|(_, p)| *p != pane) {
        active.0 = survivor;
    }
}

/// When an Activity is despawned, any Pane whose `ActiveActivity(Entity)`
/// pointed at it must be repointed. Mirror of `on_remove_pane_marker`.
pub fn on_remove_activity_marker(
    ev: On<Remove, ActivityMarker>,
    activities: Query<&ChildOf, With<ActivityMarker>>,
    children: Query<&Children>,
    mut panes: Query<&mut ActiveActivity, With<PaneMarker>>,
) {
    let activity = ev.entity;
    let Ok(child_of) = activities.get(activity) else {
        return;
    };
    let pane = child_of.parent();
    let Ok(mut active) = panes.get_mut(pane) else {
        return;
    };
    if active.0 != activity {
        return;
    }
    let Ok(sibs) = children.get(pane) else {
        return;
    };
    if let Some(survivor) = sibs.iter().find(|&e| e != activity && activities.get(e).is_ok()) {
        active.0 = survivor;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cells::{Side, SplitOrientation};
    use crate::commands::MultiplexerCommands;
    use bevy::ecs::system::RunSystemOnce;

    #[test]
    fn removing_pane_repoints_active_pane_to_survivor() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(crate::plugin::MultiplexerPlugin);

        let outcome = app
            .world_mut()
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_session(None))
            .unwrap();
        let new_pane = app
            .world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.split_pane(outcome.pane, Side::After, SplitOrientation::Horizontal).unwrap()
            })
            .unwrap();

        app.world_mut().entity_mut(outcome.session).insert(ActivePane(new_pane));
        app.world_mut().entity_mut(new_pane).despawn();
        app.update();

        assert_eq!(
            app.world().get::<ActivePane>(outcome.session).map(|a| a.0),
            Some(outcome.pane),
            "observer must repoint ActivePane to the surviving sibling",
        );
    }
}
