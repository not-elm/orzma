//! Per-pane stable chrome containers. `PaneChrome` records the two UI
//! entities (tab-bar root + activity slot) that survive geometry rebuilds
//! for one Pane, so tab/host content is not torn down on every layout
//! change. `despawn_pane_chrome_on_pane_removal` despawns those containers
//! when the Pane closes (they are not children of the despawned pane frame,
//! so they need explicit cleanup).

use bevy::prelude::*;
use ozmux_multiplexer::PaneMarker;

/// The stable tab-bar root and activity slot for one Pane. Inserted lazily on
/// the Pane entity by the chrome systems; read by the geometry rebuild (to
/// reparent the containers under the new pane frame) and by
/// `sync_pane_activities` (to fill them).
#[derive(Component, Debug, Clone, Copy)]
pub(crate) struct PaneChrome {
    /// The stable Row node that holds one tab per activity.
    pub(crate) tab_bar_root: Entity,
    /// The stable node the active activity's host is parented under.
    pub(crate) activity_slot: Entity,
}

/// Despawns a closed Pane's stable chrome containers. Driven by
/// `On<Remove, PaneMarker>` so it reads `PaneChrome` while the Pane still
/// exists.
pub(crate) fn despawn_pane_chrome_on_pane_removal(
    ev: On<Remove, PaneMarker>,
    chromes: Query<&PaneChrome>,
    mut commands: Commands,
) {
    if let Ok(chrome) = chromes.get(ev.entity) {
        commands.entity(chrome.tab_bar_root).despawn();
        commands.entity(chrome.activity_slot).despawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::MinimalPlugins;
    use bevy::app::App;

    #[test]
    fn observer_despawns_chrome_containers_when_pane_is_removed() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_observer(despawn_pane_chrome_on_pane_removal);

        let tab_bar = app.world_mut().spawn(Node::default()).id();
        let slot = app.world_mut().spawn(Node::default()).id();
        let pane = app
            .world_mut()
            .spawn((
                PaneMarker,
                PaneChrome {
                    tab_bar_root: tab_bar,
                    activity_slot: slot,
                },
            ))
            .id();

        app.world_mut().despawn(pane);
        app.world_mut().flush();

        assert!(
            app.world().get_entity(tab_bar).is_err(),
            "tab_bar_root must be despawned on pane removal"
        );
        assert!(
            app.world().get_entity(slot).is_err(),
            "activity_slot must be despawned on pane removal"
        );
    }
}
