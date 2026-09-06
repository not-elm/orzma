//! Pane spawn requests: pre-spawns the pane entity, binds its control
//! token, and asks the backend for the PTY.

use crate::surface::OrzmaTerminal;
use crate::ui::ShellSurfaceUi;
use bevy::prelude::*;
use bevy_orzma_mux::prelude::{MuxConnection, PaneRegistry, absolute_px_node};
use bevy_orzma_webview::ControlPlaneHandle;
use orzma_mux::prelude::{MuxCommand, NewPaneAt, RequestId};

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
        app.add_observer(on_pane_spawn_request);
    }
}

/// Pre-spawns the entity (zero-sized until its first layout), binds the
/// control-plane token so a fast shell can connect before `PaneOpened`
/// is drained, then sends `NewPane`.
fn on_pane_spawn_request(
    ev: On<PaneSpawnRequest>,
    mut commands: Commands,
    mut registry: ResMut<PaneRegistry>,
    connection: Res<MuxConnection>,
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
    connection.0.send(MuxCommand::NewPane {
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
    use orzma_mux::prelude::MuxClient;
    use std::path::PathBuf;

    /// Asserts that a spawn request pre-spawns the pane entity, binds its
    /// token, and sends `NewPane` carrying that token in `env`.
    ///
    /// Case: the user splits a pane; the new shell connects to the
    /// control socket before the backend's `PaneOpened` is drained.
    #[test]
    fn a_spawn_request_binds_the_token_then_sends_new_pane() {
        let (client, _events, commands) = MuxClient::detached();
        let tokens = TokenRegistry::default();
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(SpawnPlugin)
            .init_resource::<PaneRegistry>()
            .insert_resource(MuxConnection(client))
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

        let sent: Vec<MuxCommand> = commands.try_iter().map(|(_, c)| c).collect();
        let [
            MuxCommand::NewPane {
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
}
