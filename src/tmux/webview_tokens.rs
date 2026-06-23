//! Host overlay: binds each tmux pane's `%N` id (matching `$TMUX_PANE`) to its
//! entity in `ozma_webview`'s token registry, so a program's `hello %N` resolves
//! to the pane that owns it. This is the multiplexer-specific half of webview
//! token resolution; the generic per-surface `$OZMA_TOKEN` path lives in the
//! webview crate.
//!
//! This module also propagates `$OZMA_SOCK` (the control-plane socket path) into
//! the tmux global environment so panes can reach the live control plane. The
//! global `-g` flag covers all existing sessions and any sessions created later,
//! so no per-session enumeration is needed. On app exit the global is unset and
//! the runtime dir is removed.

use bevy::prelude::*;
use ozma_tty_engine::TerminalRawWrite;
use ozma_webview::ControlPlaneHandle;
use ozmux_tmux::{
    SetEnvironmentGlobal, TmuxConnection, TmuxPane, TmuxPresence, UnsetEnvironmentGlobal,
};

/// Registers the tmux `%N` token binder and `$OZMA_SOCK` propagation systems.
pub(crate) struct WebviewTokensPlugin;

impl Plugin for WebviewTokensPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            bind_tmux_pane_tokens.run_if(resource_exists::<TmuxPresence>),
        )
        .add_systems(
            Update,
            refresh_ozma_sock.run_if(resource_added::<TmuxPresence>),
        )
        .add_systems(Last, cleanup_ozma_sock.run_if(on_message::<AppExit>));
    }
}

/// Binds `%<pane-id>` → pane entity for every newly projected tmux pane.
fn bind_tmux_pane_tokens(
    new_panes: Query<(Entity, &TmuxPane), Added<TmuxPane>>,
    handle: Option<Res<ControlPlaneHandle>>,
) {
    let Some(handle) = handle else {
        return;
    };
    for (entity, pane) in new_panes.iter() {
        handle.tokens.insert(format!("%{}", pane.id.0), entity);
    }
}

/// Propagates `$OZMA_SOCK` into the tmux global environment on each attach edge.
///
/// Runs exactly once per attach (`resource_added::<TmuxPresence>`). The global
/// `-g` flag writes to the tmux server's global environment, which all existing
/// sessions inherit and new sessions pick up automatically — no per-session
/// enumeration is required.
///
/// For a remote-adopted session the socket path refers to a local path the
/// remote host cannot reach, but the set is harmless; the next local attach
/// overwrites it.
///
/// No-op when the control plane is absent (no `ControlPlaneHandle`).
fn refresh_ozma_sock(
    connection: NonSend<TmuxConnection>,
    control: Option<Res<ControlPlaneHandle>>,
) {
    let Some(control) = control else {
        return;
    };
    let Some(handle) = connection.handle() else {
        return;
    };
    let sock = control.sock_path.to_string_lossy();
    if let Err(e) = handle.send(SetEnvironmentGlobal {
        key: "OZMA_SOCK",
        value: sock.as_ref(),
    }) {
        tracing::warn!(?e, "failed to set global $OZMA_SOCK");
    }
}

/// Unsets `$OZMA_SOCK` from the tmux global environment on app exit and removes
/// the control runtime dir.
///
/// Gated by `run_if(on_message::<AppExit>)` so it runs only on the exit frame.
/// The unset is best-effort; a hard kill (SIGKILL / unhandled SIGTERM) skips
/// all of this, and the next attach overwrites the value via
/// [`refresh_ozma_sock`].
///
/// The runtime dir (`sock_path`'s grandparent — `<temp>/<pid>/control`) is
/// removed explicitly here rather than relying solely on `RuntimeRoot`'s `Drop`,
/// which can be skipped when noisy CEF teardown ends the process before the
/// world is dropped.
///
/// NOTE: only the `control` subdir is removed, not the `<pid>` parent — sibling
/// webview runtime roots can live under the same pid dir, so `remove_dir_all` on
/// the parent would delete theirs.
///
/// NOTE: this is an exclusive system so it can call `world.trigger(TerminalRawWrite
/// { .. })` synchronously before removing the runtime dir. `flush_tmux_outgoing`
/// (the scheduled flusher) ran in `Update` and will not run again this frame, so
/// bytes queued with `handle.send()` alone would never reach the PTY. Triggering
/// `TerminalRawWrite` directly bypasses that gap.
fn cleanup_ozma_sock(world: &mut World) {
    let runtime_root = world
        .get_resource::<ControlPlaneHandle>()
        .and_then(|c| c.sock_path.parent()?.parent().map(|p| p.to_path_buf()));

    let write = world
        .get_non_send_resource::<TmuxConnection>()
        .and_then(|conn| {
            let handle = conn.handle()?;
            let _ = handle.send(UnsetEnvironmentGlobal { key: "OZMA_SOCK" });
            let bytes = conn.take_outgoing();
            let gateway = conn.gateway()?;
            if bytes.is_empty() {
                None
            } else {
                Some((gateway, bytes))
            }
        });

    if let Some((gateway, bytes)) = write {
        world.trigger(TerminalRawWrite {
            entity: gateway,
            bytes,
        });
    }

    if let Some(runtime_root) = runtime_root {
        let _ = std::fs::remove_dir_all(runtime_root);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ozma_webview::TokenRegistry;
    use ozmux_tmux::PaneId;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use tmux_control_parser::CellDims;

    #[test]
    fn binds_pane_id_token_to_entity() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        let tokens = TokenRegistry::default();
        app.insert_resource(ControlPlaneHandle {
            sock_path: PathBuf::from("/tmp/x.sock"),
            tokens: tokens.clone(),
        });
        app.add_systems(Update, bind_tmux_pane_tokens);

        let dims = CellDims {
            width: 80,
            height: 24,
            xoff: 0,
            yoff: 0,
        };
        let pane = app
            .world_mut()
            .spawn(TmuxPane {
                id: PaneId(7),
                dims,
            })
            .id();
        app.update();

        assert_eq!(
            tokens.resolve("%7"),
            Some(pane),
            "%N resolves to its pane entity"
        );
    }

    #[test]
    fn refresh_sends_set_environment_global() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_non_send_resource(TmuxConnection::default());

        let gateway = app.world_mut().spawn_empty().id();
        app.world_mut()
            .non_send_resource_mut::<TmuxConnection>()
            .adopt(gateway);

        app.insert_resource(ControlPlaneHandle {
            sock_path: PathBuf::from("/tmp/ctl.sock"),
            tokens: TokenRegistry::default(),
        });

        app.add_systems(
            Update,
            refresh_ozma_sock.run_if(resource_added::<TmuxPresence>),
        );
        app.insert_resource(TmuxPresence);
        app.update();

        let out = app
            .world()
            .non_send_resource::<TmuxConnection>()
            .take_outgoing();
        assert_eq!(
            out, b"set-environment -g OZMA_SOCK /tmp/ctl.sock\n",
            "refresh sends a global set-environment over the adopted connection"
        );
    }

    #[test]
    fn refresh_is_no_op_without_control_plane() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_non_send_resource(TmuxConnection::default());

        let gateway = app.world_mut().spawn_empty().id();
        app.world_mut()
            .non_send_resource_mut::<TmuxConnection>()
            .adopt(gateway);

        app.add_systems(
            Update,
            refresh_ozma_sock.run_if(resource_added::<TmuxPresence>),
        );
        app.insert_resource(TmuxPresence);
        app.update();

        let out = app
            .world()
            .non_send_resource::<TmuxConnection>()
            .take_outgoing();
        assert!(
            out.is_empty(),
            "no ControlPlaneHandle means no set-environment is sent"
        );
    }

    #[test]
    fn cleanup_sends_unset_environment_global() {
        #[derive(Resource, Default, Clone)]
        struct Written(Arc<Mutex<Vec<(Entity, Vec<u8>)>>>);

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<Written>();
        app.add_observer(|ev: On<TerminalRawWrite>, written: Res<Written>| {
            written
                .0
                .lock()
                .unwrap()
                .push((ev.entity, ev.bytes.clone()));
        });
        app.insert_non_send_resource(TmuxConnection::default());

        let gateway = app.world_mut().spawn_empty().id();
        app.world_mut()
            .non_send_resource_mut::<TmuxConnection>()
            .adopt(gateway);

        // NOTE: cleanup_ozma_sock remove_dir_all's the sock_path's GRANDPARENT.
        // This path must stay deep + under a non-existent temp subdir so that
        // grandparent is a harmless absent dir — never a shallow system dir. A
        // `/tmp/ctl.sock` here has grandparent `/`, i.e. `remove_dir_all("/")`.
        app.insert_resource(ControlPlaneHandle {
            sock_path: std::env::temp_dir()
                .join("ozmux-cleanup-test-nonexistent")
                .join("control")
                .join("sock")
                .join("control.sock"),
            tokens: TokenRegistry::default(),
        });

        app.add_message::<AppExit>();
        app.add_systems(Last, cleanup_ozma_sock.run_if(on_message::<AppExit>));
        app.world_mut().write_message(AppExit::Success);
        app.update();

        let written = app.world().resource::<Written>().0.lock().unwrap().clone();
        assert_eq!(
            written,
            vec![(gateway, b"set-environment -gu OZMA_SOCK\n".to_vec())],
            "cleanup synchronously triggers TerminalRawWrite with the unset command"
        );
    }

    #[test]
    fn bind_pane_runs_in_default_mode_with_presence() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        let tokens = TokenRegistry::default();
        app.insert_resource(ControlPlaneHandle {
            sock_path: PathBuf::from("/tmp/x.sock"),
            tokens: tokens.clone(),
        });
        app.insert_resource(TmuxPresence);
        app.add_systems(
            Update,
            bind_tmux_pane_tokens.run_if(resource_exists::<TmuxPresence>),
        );

        let dims = CellDims {
            width: 80,
            height: 24,
            xoff: 0,
            yoff: 0,
        };
        let pane = app
            .world_mut()
            .spawn(TmuxPane {
                id: PaneId(42),
                dims,
            })
            .id();
        app.update();

        assert_eq!(
            tokens.resolve("%42"),
            Some(pane),
            "%N token bound even when TmuxPresence exists but no AppMode::Tmux state"
        );
    }
}
