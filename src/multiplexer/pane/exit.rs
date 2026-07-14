//! Shell-exit cascade: when a pane's shell process exits, closes that pane —
//! collapsing its window's layout tree onto a neighbor, or, when it was the
//! last pane, handing the whole window off to the `KillWindowRequest`
//! teardown (`crate::multiplexer::window::on_kill_window`), which reactivates
//! a neighbor window and, if that was the last window, exits the app.

#[cfg(test)]
use crate::multiplexer::bootstrap::WindowContainer;
use crate::multiplexer::pane::MultiplexerPane;
use crate::multiplexer::request::{KillPaneRequest, KillWindowRequest};
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
/// sibling survives, the pane's container (its `ChildOf` parent) despawns
/// recursively, taking the pane with it — falling back to despawning `pane`
/// directly when it has no `ChildOf` parent (a PTY-less unit test) — and the
/// window's `active_pane` moves to the surviving neighbor ONLY when `pane`
/// was the active one, so closing a background pane never steals focus. If
/// `pane` was the window's last leaf, the whole window's teardown is handed
/// off to a `KillWindowRequest` (`crate::multiplexer::window::on_kill_window`)
/// rather than duplicated here, so both close paths reactivate a neighbor
/// window, renumber, and — when it was the last window — exit the app
/// identically.
///
/// A no-op when `pane` does not carry `MultiplexerPane` (already closed, or
/// not a multiplexer pane at all). Reused by `on_kill_pane` below.
pub(in crate::multiplexer) fn close_pane(
    commands: &mut Commands,
    windows: &mut Query<&mut MultiplexerWindow>,
    layouts: &mut Query<&mut MultiplexerLayoutComp>,
    panes: &Query<&MultiplexerPane>,
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
            if let Ok(mut w) = windows.get_mut(window)
                && w.active_pane == pane
            {
                w.active_pane = neighbor;
            }
            if let Ok(child_of) = child_ofs.get(pane) {
                commands.entity(child_of.parent()).despawn();
            } else {
                commands.entity(pane).despawn();
            }
        }
        None => {
            commands.trigger(KillWindowRequest { window });
        }
    }
}

/// Observer: on `TerminalChildExit`, runs the close-pane cascade for the
/// exited entity when it is a `MultiplexerPane`.
fn on_pane_shell_exit(
    ev: On<TerminalChildExit>,
    mut commands: Commands,
    mut windows: Query<&mut MultiplexerWindow>,
    mut layouts: Query<&mut MultiplexerLayoutComp>,
    panes: Query<&MultiplexerPane>,
    child_ofs: Query<&ChildOf>,
) {
    close_pane(
        &mut commands,
        &mut windows,
        &mut layouts,
        &panes,
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
    mut windows: Query<&mut MultiplexerWindow>,
    mut layouts: Query<&mut MultiplexerLayoutComp>,
    panes: Query<&MultiplexerPane>,
    child_ofs: Query<&ChildOf>,
) {
    close_pane(
        &mut commands,
        &mut windows,
        &mut layouts,
        &panes,
        &child_ofs,
        ev.event_target(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multiplexer::layout::{MultiplexerLayout, SplitAxis};
    use crate::multiplexer::window::{ActiveMultiplexerWindow, WindowPlugin};
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
        app.add_plugins(WindowPlugin);

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
        // A WindowContainer wired as the pane's ChildOf ancestor mirrors the
        // real spawn shape (`bootstrap.rs`, `window.rs::spawn_window`) closely
        // enough for on_kill_window's container-despawn cascade to reach the
        // pane, since close_pane's last-leaf branch no longer despawns the
        // pane itself.
        let window_container = app.world_mut().spawn(WindowContainer { window }).id();
        app.world_mut()
            .entity_mut(pane)
            .insert((MultiplexerPane { window }, ChildOf(window_container)));

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
    fn close_active_window_last_pane_reactivates_neighbor() {
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
        app.add_plugins(WindowPlugin);

        // Window 0: bootstrap-style, inactive, single pane.
        let pane0 = app.world_mut().spawn_empty().id();
        let window0 = app
            .world_mut()
            .spawn((
                MultiplexerWindow {
                    index: 0,
                    name: None,
                    active_pane: pane0,
                },
                MultiplexerLayoutComp(MultiplexerLayout::new(pane0)),
            ))
            .id();
        app.world_mut()
            .entity_mut(pane0)
            .insert(MultiplexerPane { window: window0 });

        // Window 1: active, single pane -- the one whose last pane is about
        // to close, reproducing the soft-lock: with no unify-via-KillWindowRequest
        // fix, this leaves zero ActiveMultiplexerWindow entities.
        let pane1 = app.world_mut().spawn_empty().id();
        let window1 = app
            .world_mut()
            .spawn((
                MultiplexerWindow {
                    index: 1,
                    name: None,
                    active_pane: pane1,
                },
                ActiveMultiplexerWindow,
                MultiplexerLayoutComp(MultiplexerLayout::new(pane1)),
            ))
            .id();
        app.world_mut()
            .entity_mut(pane1)
            .insert(MultiplexerPane { window: window1 });

        app.world_mut().trigger(TerminalChildExit {
            entity: pane1,
            code: Some(0),
        });
        app.world_mut().flush();
        app.update();

        let world = app.world_mut();
        let mut actives = world.query_filtered::<Entity, With<ActiveMultiplexerWindow>>();
        let active_windows: Vec<Entity> = actives.iter(world).collect();
        assert_eq!(
            active_windows,
            vec![window0],
            "exactly one active window must remain, and it must be the surviving window"
        );
        assert!(
            !app.world().resource::<Got>().0,
            "closing the active window's last pane while another window exists must not exit \
             the app"
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
    fn close_background_pane_keeps_active_pane() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<AppExit>();
        app.add_observer(on_pane_shell_exit);

        let (window, pane_a, pane_b) = spawn_two_pane_window(&mut app);
        app.world_mut()
            .get_mut::<MultiplexerWindow>(window)
            .unwrap()
            .active_pane = pane_b;

        app.world_mut().trigger(TerminalChildExit {
            entity: pane_a,
            code: Some(0),
        });
        app.world_mut().flush();
        app.update();

        assert!(
            app.world().get_entity(pane_a).is_err(),
            "the closed background pane must not survive the same frame"
        );
        assert_eq!(
            app.world()
                .get::<MultiplexerWindow>(window)
                .unwrap()
                .active_pane,
            pane_b,
            "closing a background (non-active) pane must not move active_pane"
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
