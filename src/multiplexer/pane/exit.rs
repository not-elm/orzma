//! Shell-exit cascade: when a pane's shell process exits, closes that pane —
//! collapsing its window's layout tree onto a neighbor, or, when it was the
//! last pane, closing the whole window and, if that was the last window,
//! exiting the app.

use crate::multiplexer::bootstrap::WindowContainer;
use crate::multiplexer::pane::MultiplexerPane;
use crate::multiplexer::window::{MultiplexerLayoutComp, MultiplexerWindow};
use bevy::prelude::*;
use orzma_tty_engine::TerminalChildExit;

/// Registers the shell-exit → pane-close cascade observer.
pub(in crate::multiplexer) struct ExitPlugin;

impl Plugin for ExitPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_pane_shell_exit);
    }
}

/// Closes `pane`: removes its leaf from its window's layout tree. If a
/// sibling survives, that window's `active_pane` moves to it and only the
/// pane despawns. If `pane` was the window's last leaf, the whole window
/// (its `WindowContainer` subtree plus the window entity itself) despawns
/// instead, and — when it was also the last window — `AppExit` is written.
///
/// A no-op when `pane` does not carry `MultiplexerPane` (already closed, or
/// not a multiplexer pane at all). Reused by the PR-2 `KillPaneRequest`
/// observer.
pub(in crate::multiplexer) fn close_pane(
    commands: &mut Commands,
    exit: &mut MessageWriter<AppExit>,
    windows: &mut Query<&mut MultiplexerWindow>,
    layouts: &mut Query<&mut MultiplexerLayoutComp>,
    panes: &Query<&MultiplexerPane>,
    all_windows: &Query<Entity, With<MultiplexerWindow>>,
    containers: &Query<(Entity, &WindowContainer)>,
    pane: Entity,
) {
    let Ok(window) = panes.get(pane).map(|p| p.window) else {
        return;
    };
    let Ok(mut layout) = layouts.get_mut(window) else {
        return;
    };
    match layout.0.remove(pane) {
        Some(neighbor) => {
            if let Ok(mut w) = windows.get_mut(window) {
                w.active_pane = neighbor;
            }
            commands.entity(pane).despawn();
        }
        None => {
            let last_window = all_windows.iter().count() <= 1;
            // NOTE: despawn the pane directly instead of relying solely on
            // the WindowContainer cascade below — a world with no UI
            // hierarchy under the container (e.g. a PTY-less unit test) has
            // no container to cascade through, and the pane would leak.
            commands.entity(pane).despawn();
            if let Some((container, _)) = containers.iter().find(|(_, c)| c.window == window) {
                commands.entity(container).despawn();
            }
            commands.entity(window).despawn();
            if last_window {
                exit.write(AppExit::Success);
            }
        }
    }
}

/// Observer: on `TerminalChildExit`, runs the close-pane cascade for the
/// exited entity when it is a `MultiplexerPane`.
fn on_pane_shell_exit(
    ev: On<TerminalChildExit>,
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
    mut windows: Query<&mut MultiplexerWindow>,
    mut layouts: Query<&mut MultiplexerLayoutComp>,
    panes: Query<&MultiplexerPane>,
    all_windows: Query<Entity, With<MultiplexerWindow>>,
    containers: Query<(Entity, &WindowContainer)>,
) {
    close_pane(
        &mut commands,
        &mut exit,
        &mut windows,
        &mut layouts,
        &panes,
        &all_windows,
        &containers,
        ev.event_target(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multiplexer::layout::MultiplexerLayout;
    use crate::multiplexer::window::ActiveMultiplexerWindow;
    use bevy::ecs::message::MessageReader;

    #[test]
    fn last_pane_exit_sends_app_exit() {
        #[derive(Resource, Default)]
        struct Got(bool);
        fn capture(mut reader: MessageReader<AppExit>, mut got: ResMut<Got>) {
            if reader.read().next().is_some() {
                got.0 = true;
            }
        }
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<AppExit>();
        app.init_resource::<Got>();
        app.add_systems(Update, capture);
        app.add_observer(on_pane_shell_exit);

        let pane = app.world_mut().spawn_empty().id();
        let window = app
            .world_mut()
            .spawn((
                MultiplexerWindow {
                    index: 0,
                    name: None,
                    active_pane: pane,
                },
                ActiveMultiplexerWindow,
                MultiplexerLayoutComp(MultiplexerLayout::new(pane)),
            ))
            .id();
        app.world_mut()
            .entity_mut(pane)
            .insert(MultiplexerPane { window });

        app.world_mut().trigger(TerminalChildExit {
            entity: pane,
            code: Some(0),
        });
        app.world_mut().flush();
        app.update();

        assert!(app.world().resource::<Got>().0, "last pane exit exits app");
        assert!(
            app.world().get_entity(pane).is_err(),
            "the closed pane must not survive the same frame"
        );
        assert!(
            app.world().get_entity(window).is_err(),
            "the closed window must not survive the same frame"
        );
    }

    #[test]
    fn exit_of_non_multiplexer_pane_is_a_no_op() {
        #[derive(Resource, Default)]
        struct Got(bool);
        fn capture(mut reader: MessageReader<AppExit>, mut got: ResMut<Got>) {
            if reader.read().next().is_some() {
                got.0 = true;
            }
        }
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<AppExit>();
        app.init_resource::<Got>();
        app.add_systems(Update, capture);
        app.add_observer(on_pane_shell_exit);

        let stray = app.world_mut().spawn_empty().id();
        app.world_mut().trigger(TerminalChildExit {
            entity: stray,
            code: Some(0),
        });
        app.update();

        assert!(
            !app.world().resource::<Got>().0,
            "a TerminalChildExit for a non-MultiplexerPane entity must not exit the app"
        );
    }
}
