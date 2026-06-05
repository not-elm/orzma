//! Lifecycle observers for the multiplexer. Both fire on Bevy's
//! `On<Remove, _>` lifecycle event, which runs *before* the component is
//! actually removed — so the removed component's value is still readable
//! on the triggering entity.
//!
//! Observer registration happens in `MultiplexerPlugin::build`.

use crate::components::{
    ActivePane, ActiveSurface, OwningWorkspace, PaneMarker, SurfaceMarker, WorkspaceMarker,
    WorkspaceUiSubtree,
};
use bevy::prelude::*;

/// When a Pane is despawned, repoint any Workspace `ActivePane` that pointed at
/// it to a surviving pane. Walks the workspace's layout tree (via the
/// `OwningWorkspace` back-pointer + `WorkspaceUiSubtree` root) and picks the
/// first pane that is NOT the dying one. The dying pane is still in the tree
/// during this pre-removal observer window, so it must be filtered explicitly.
pub fn on_remove_pane_marker(
    ev: On<Remove, PaneMarker>,
    owners: Query<&OwningWorkspace, With<PaneMarker>>,
    subtrees: Query<&WorkspaceUiSubtree>,
    children: Query<&Children>,
    panes: Query<(), With<PaneMarker>>,
    mut workspaces: Query<&mut ActivePane, With<WorkspaceMarker>>,
) {
    let dying = ev.entity;
    let Ok(owner) = owners.get(dying) else {
        return;
    };
    let workspace = owner.0;
    let Ok(mut active) = workspaces.get_mut(workspace) else {
        return;
    };
    if active.0 != dying {
        return;
    }
    let Ok(root) = subtrees.get(workspace) else {
        return;
    };
    let survivor = crate::layout::ordered_panes(root.0, &children, &panes)
        .into_iter()
        .find(|&p| p != dying);
    if let Some(s) = survivor {
        active.0 = s;
    }
}

/// When a Surface is despawned, any Pane whose `ActiveSurface(Entity)`
/// pointed at it must be repointed. Mirror of `on_remove_pane_marker`.
pub fn on_remove_surface_marker(
    ev: On<Remove, SurfaceMarker>,
    surfaces: Query<&ChildOf, With<SurfaceMarker>>,
    children: Query<&Children>,
    mut panes: Query<&mut ActiveSurface, With<PaneMarker>>,
) {
    let surface = ev.entity;
    let Ok(child_of) = surfaces.get(surface) else {
        return;
    };
    let pane = child_of.parent();
    let Ok(mut active) = panes.get_mut(pane) else {
        return;
    };
    if active.0 != surface {
        return;
    }
    let Ok(sibs) = children.get(pane) else {
        return;
    };
    if let Some(survivor) = sibs
        .iter()
        .find(|&e| e != surface && surfaces.get(e).is_ok())
    {
        active.0 = survivor;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::MultiplexerCommands;
    use crate::layout::{Side, SplitOrientation};
    use bevy::ecs::system::RunSystemOnce;

    #[test]
    fn removing_pane_repoints_active_pane_to_survivor() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(crate::plugin::MultiplexerPlugin);

        let outcome = app
            .world_mut()
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_workspace(None))
            .unwrap();
        let new_pane = app
            .world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.split_pane(outcome.pane, Side::After, SplitOrientation::Horizontal)
                    .unwrap()
            })
            .unwrap();

        app.world_mut()
            .entity_mut(outcome.workspace)
            .insert(ActivePane(new_pane));
        app.world_mut().entity_mut(new_pane).despawn();
        app.update();

        assert_eq!(
            app.world()
                .get::<ActivePane>(outcome.workspace)
                .map(|a| a.0),
            Some(outcome.pane),
            "observer must repoint ActivePane to the surviving sibling",
        );
    }

    #[test]
    fn removing_pane_repoints_active_to_remaining_tree_pane() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(crate::plugin::MultiplexerPlugin);

        let outcome = app
            .world_mut()
            .run_system_once(|mut mux: MultiplexerCommands| mux.create_workspace(None))
            .unwrap();
        let new_pane = app
            .world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                mux.split_pane(outcome.pane, Side::After, SplitOrientation::Horizontal)
                    .unwrap()
            })
            .unwrap();
        app.world_mut().flush();

        app.world_mut()
            .entity_mut(outcome.workspace)
            .insert(ActivePane(new_pane));
        app.world_mut().entity_mut(new_pane).despawn();
        app.update();

        assert_eq!(
            app.world()
                .get::<ActivePane>(outcome.workspace)
                .map(|a| a.0),
            Some(outcome.pane),
        );
    }
}
