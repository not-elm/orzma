//! Pane spawn requests: pre-spawns the pane entity, binds its control
//! token, and asks the backend for the PTY.

use crate::surface::OrzmaTerminal;
use crate::ui::ShellSurfaceUi;
use bevy::prelude::*;
use bevy_orzma_webview::ControlPlaneHandle;
use bevy_orzmux::prelude::{OrzmuxConnection, PaneRegistry, absolute_px_node};
use orzmux::prelude::{NewPaneAt, OrzmuxCommand, RequestId};

/// Asks for a new pane at `at`.
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct PaneSpawnRequest {
    /// Where the new pane goes in the layout tree.
    pub at: NewPaneAt,
}

/// Registers the spawn observer.
pub(super) struct SpawnPlugin;

impl Plugin for SpawnPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_pane_spawn_request.run_if(resource_exists::<OrzmuxConnection>));
    }
}

/// Pre-spawns the entity (zero-sized until its first layout), binds the
/// control-plane token so a fast shell can connect before `PaneOpened`
/// is drained, then sends `NewPane`.
fn on_pane_spawn_request(
    ev: On<PaneSpawnRequest>,
    mut commands: Commands,
    mut registry: ResMut<PaneRegistry>,
    connection: Res<OrzmuxConnection>,
    container: Query<Entity, With<ShellSurfaceUi>>,
    control: Option<Res<ControlPlaneHandle>>,
) {
    let Ok(container) = container.single() else {
        return;
    };
    let entity = commands
        .spawn((OrzmaTerminal, pending_pane_node(), ChildOf(container)))
        .id();
    let env = control
        .as_deref()
        .map(|c| {
            c.bind_surface(entity);
            c.surface_env(entity).to_vec()
        })
        .unwrap_or_default();
    let request = RequestId::next();
    registry.pending_spawns.insert(request, entity);
    connection.0.send(OrzmuxCommand::NewPane {
        request,
        at: ev.at,
        cwd: None,
        env,
    });
}

/// A zero-sized absolute node, so a pending pane covers nothing until
/// its first `Layout`.
fn pending_pane_node() -> Node {
    absolute_px_node(0.0, 0.0, 0.0, 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_orzma_webview::TokenRegistry;
    use orzmux::prelude::OrzmuxClient;
    use std::path::PathBuf;

    /// Asserts that a spawn request pre-spawns the pane entity, binds its
    /// token, and sends `NewPane` carrying that token in `env`.
    ///
    /// Case: the user splits a pane; the new shell connects to the
    /// control socket before the backend's `PaneOpened` is drained.
    #[test]
    fn a_spawn_request_binds_the_token_then_sends_new_pane() {
        let (client, _events, commands) = OrzmuxClient::detached();
        let tokens = TokenRegistry::default();
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(SpawnPlugin)
            .init_resource::<PaneRegistry>()
            .insert_resource(OrzmuxConnection(client))
            .insert_resource(ControlPlaneHandle {
                sock_path: PathBuf::from("/tmp/ctl.sock"),
                tokens: tokens.clone(),
            });
        app.world_mut().spawn((Node::default(), ShellSurfaceUi));
        app.world_mut().trigger(PaneSpawnRequest {
            at: NewPaneAt::Root,
        });
        app.update();

        let registry = app.world().resource::<PaneRegistry>();
        assert_eq!(registry.pending_spawns.len(), 1);
        let (request, entity) = registry
            .pending_spawns
            .iter()
            .next()
            .map(|(r, e)| (*r, *e))
            .unwrap();
        assert!(app.world().get::<OrzmaTerminal>(entity).is_some());
        let token = format!("orzma:{}", entity.to_bits());
        assert_eq!(tokens.resolve(&token), Some(entity));

        let sent: Vec<OrzmuxCommand> = commands.try_iter().map(|(_, c)| c).collect();
        let [
            OrzmuxCommand::NewPane {
                request: sent_request,
                at: NewPaneAt::Root,
                cwd: None,
                env,
            },
        ] = sent.as_slice()
        else {
            panic!("expected one NewPane, got {sent:?}");
        };
        assert_eq!(*sent_request, request);
        assert!(env.contains(&("ORZMA_TOKEN".to_string(), token)));
    }

    /// Asserts that a spawn request without a connection spawns no
    /// entity and records no pending spawn, rather than panicking.
    ///
    /// Case: the backend thread has died, and a split shortcut fires
    /// before `AppExit` takes effect.
    #[test]
    fn a_spawn_request_without_a_connection_does_nothing() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(SpawnPlugin)
            .init_resource::<PaneRegistry>();
        app.world_mut().spawn((Node::default(), ShellSurfaceUi));
        app.world_mut().trigger(PaneSpawnRequest {
            at: NewPaneAt::Root,
        });
        app.update();

        assert!(
            app.world()
                .resource::<PaneRegistry>()
                .pending_spawns
                .is_empty()
        );
        let mut terminals = app
            .world_mut()
            .query_filtered::<Entity, With<OrzmaTerminal>>();
        assert_eq!(terminals.iter(app.world()).count(), 0);
    }
}
