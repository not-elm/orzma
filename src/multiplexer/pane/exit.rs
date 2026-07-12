//! Shell-exit cascade: when a pane's shell process exits, closes that pane —
//! collapsing its window's layout tree onto a neighbor, or, when it was the
//! last pane, closing the whole window and, if that was the last window,
//! exiting the app.

use crate::multiplexer::bootstrap::WindowContainer;
use crate::multiplexer::pane::MultiplexerPane;
use crate::multiplexer::request::KillPaneRequest;
use crate::multiplexer::window::{MultiplexerLayoutComp, MultiplexerWindow};
use bevy::prelude::*;
use orzma_tty_engine::TerminalChildExit;

/// Registers the pane-close cascade observers: shell-exit and kill-pane
/// request.
pub(in crate::multiplexer) struct ExitPlugin;

impl Plugin for ExitPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_pane_shell_exit)
            .add_observer(on_kill_pane);
    }
}

/// Closes `pane`: removes its leaf from its window's layout tree. If a
/// sibling survives, that window's `active_pane` moves to it and the pane's
/// container (its `ChildOf` parent) despawns recursively, taking the pane
/// with it — falling back to despawning `pane` directly when it has no
/// `ChildOf` parent (a PTY-less unit test). If `pane` was the window's last
/// leaf, the whole window (its `WindowContainer` subtree plus the window
/// entity itself) despawns instead, and — when it was also the last window —
/// `AppExit` is written.
///
/// A no-op when `pane` does not carry `MultiplexerPane` (already closed, or
/// not a multiplexer pane at all). Reused by `on_kill_pane` below.
pub(in crate::multiplexer) fn close_pane(
    commands: &mut Commands,
    exit: &mut MessageWriter<AppExit>,
    windows: &mut Query<&mut MultiplexerWindow>,
    layouts: &mut Query<&mut MultiplexerLayoutComp>,
    panes: &Query<&MultiplexerPane>,
    all_windows: &Query<Entity, With<MultiplexerWindow>>,
    containers: &Query<(Entity, &WindowContainer)>,
    child_ofs: &Query<&ChildOf>,
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
            if let Ok(child_of) = child_ofs.get(pane) {
                commands.entity(child_of.parent()).despawn();
            } else {
                commands.entity(pane).despawn();
            }
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
    child_ofs: Query<&ChildOf>,
) {
    close_pane(
        &mut commands,
        &mut exit,
        &mut windows,
        &mut layouts,
        &panes,
        &all_windows,
        &containers,
        &child_ofs,
        ev.event_target(),
    );
}

/// Observer: on `KillPaneRequest`, runs the close-pane cascade for the
/// targeted pane. Despawning the pane's `TerminalHandle` in that cascade also
/// drives `orzma_webview`'s `gc_despawned_surfaces`
/// (`crates/orzma_webview/src/control_plane.rs:410`), which unmounts any
/// webview the pane hosted — no separate webview teardown is needed here.
fn on_kill_pane(
    ev: On<KillPaneRequest>,
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
    mut windows: Query<&mut MultiplexerWindow>,
    mut layouts: Query<&mut MultiplexerLayoutComp>,
    panes: Query<&MultiplexerPane>,
    all_windows: Query<Entity, With<MultiplexerWindow>>,
    containers: Query<(Entity, &WindowContainer)>,
    child_ofs: Query<&ChildOf>,
) {
    close_pane(
        &mut commands,
        &mut exit,
        &mut windows,
        &mut layouts,
        &panes,
        &all_windows,
        &containers,
        &child_ofs,
        ev.event_target(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multiplexer::layout::{MultiplexerLayout, SplitAxis};
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

    /// Spawns a 2-leaf window — `pane_a` split from `pane_b` along
    /// `SplitAxis::Vertical`, `pane_a` active — with no `ChildOf` container
    /// hierarchy (a PTY-less unit test, per `close_pane`'s fallback).
    /// Returns `(window, pane_a, pane_b)`.
    fn spawn_two_pane_window(app: &mut App) -> (Entity, Entity, Entity) {
        let pane_a = app.world_mut().spawn_empty().id();
        let pane_b = app.world_mut().spawn_empty().id();
        let mut layout = MultiplexerLayout::new(pane_a);
        layout.split(pane_a, pane_b, SplitAxis::Vertical);
        let window = app
            .world_mut()
            .spawn((
                MultiplexerWindow {
                    index: 0,
                    name: None,
                    active_pane: pane_a,
                },
                ActiveMultiplexerWindow,
                MultiplexerLayoutComp(layout),
            ))
            .id();
        app.world_mut()
            .entity_mut(pane_a)
            .insert(MultiplexerPane { window });
        app.world_mut()
            .entity_mut(pane_b)
            .insert(MultiplexerPane { window });
        (window, pane_a, pane_b)
    }

    #[test]
    fn neighbor_survives_pane_exit() {
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

        let (window, pane_a, pane_b) = spawn_two_pane_window(&mut app);

        app.world_mut().trigger(TerminalChildExit {
            entity: pane_a,
            code: Some(0),
        });
        app.world_mut().flush();
        app.update();

        assert!(
            app.world().get_entity(pane_a).is_err(),
            "the closed pane must not survive the same frame"
        );
        assert!(
            app.world().get_entity(pane_b).is_ok(),
            "the surviving neighbor must not be despawned"
        );
        assert_eq!(
            app.world()
                .get::<MultiplexerWindow>(window)
                .unwrap()
                .active_pane,
            pane_b,
            "active_pane must move to the surviving neighbor"
        );
        assert!(
            !app.world().resource::<Got>().0,
            "closing a pane with a surviving neighbor must not exit the app"
        );
    }

    #[test]
    fn kill_pane_request_closes_pane_and_moves_active_to_neighbor() {
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
        app.add_observer(on_kill_pane);

        let (window, pane_a, pane_b) = spawn_two_pane_window(&mut app);

        app.world_mut().trigger(KillPaneRequest { pane: pane_a });
        app.world_mut().flush();
        app.update();

        assert!(
            app.world().get_entity(pane_a).is_err(),
            "the killed pane entity (and its MultiplexerPane/TerminalHandle) must be fully \
             despawned, which drives orzma_webview's TerminalHandle-removal GC"
        );
        assert!(
            app.world().get_entity(pane_b).is_ok(),
            "the surviving neighbor must not be despawned"
        );
        assert_eq!(
            app.world()
                .get::<MultiplexerWindow>(window)
                .unwrap()
                .active_pane,
            pane_b,
            "active_pane must move to the surviving neighbor"
        );
        assert_eq!(
            app.world()
                .get::<MultiplexerLayoutComp>(window)
                .unwrap()
                .0
                .leaves(),
            vec![pane_b],
            "the layout tree must drop the killed pane's leaf"
        );
        assert!(
            !app.world().resource::<Got>().0,
            "killing a pane with a surviving neighbor must not exit the app"
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
