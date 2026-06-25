//! Host overlay: binds each tmux pane's `%N` id (matching `$TMUX_PANE`) to its
//! entity in `ozma_webview`'s token registry, so a program's `hello %N` resolves
//! to the pane that owns it. This is the multiplexer-specific half of webview
//! token resolution; the generic per-surface `$OZMA_TOKEN` path lives in the
//! webview crate.
//!
//! This module also propagates `$OZMA_SOCK` (the control-plane socket path) into
//! the tmux environment so panes can reach the live control plane. Two writes
//! cover the two ways a program reads it back:
//!
//! - A global `-g` set (on the attach edge) so the process environment of any
//!   pane spawned *after* the set inherits `$OZMA_SOCK` directly.
//! - A per-session `-t` set (on each session add or switch) so
//!   `tmux show-environment OZMA_SOCK` recovers the value from *any* pane of the
//!   session — including a pane that forked before the global set (the adopted
//!   control-mode pane, and any pane attached to a pre-existing session). A
//!   global-scope variable is invisible to session-scope `show-environment`, so
//!   the per-session set is what makes the ratatui-ozma SDK's tmux recovery path
//!   work in the first pane.
//!
//! On app exit the global is unset and the runtime dir is removed.

use bevy::prelude::*;
use ozma_tty_engine::TerminalRawWrite;
use ozma_webview::ControlPlaneHandle;
use ozmux_tmux::{
    SetEnvironmentGlobal, SetEnvironmentInSession, TmuxClient, TmuxClientMut, TmuxPane,
    TmuxSession, UnsetEnvironmentGlobal,
};

/// Registers the tmux `%N` token binder and `$OZMA_SOCK` propagation systems.
pub(crate) struct WebviewTokensPlugin;

impl Plugin for WebviewTokensPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            bind_tmux_pane_tokens.run_if(any_with_component::<TmuxClient>),
        )
        .add_systems(Update, refresh_ozma_sock.run_if(tmux_client_added))
        .add_systems(
            Update,
            bind_ozma_sock_to_session.run_if(tmux_session_changed),
        )
        .add_systems(Last, cleanup_ozma_sock.run_if(on_message::<AppExit>));
    }
}

/// Run condition: true on the frame a [`TmuxClient`] is newly adopted.
fn tmux_client_added(added: Query<(), Added<TmuxClient>>) -> bool {
    !added.is_empty()
}

/// Run condition: true on a frame where any [`TmuxSession`] is added or changed.
fn tmux_session_changed(changed: Query<(), Changed<TmuxSession>>) -> bool {
    !changed.is_empty()
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
/// Runs exactly once per attach (`Added<TmuxClient>`). The global `-g` set seeds
/// the process environment of every pane spawned *after* it. It does not reach a
/// pane that forked earlier, and a global-scope value is invisible to
/// session-scope `tmux show-environment`; [`bind_ozma_sock_to_session`] covers
/// those panes with a per-session set.
///
/// For a remote-adopted session the socket path refers to a local path the
/// remote host cannot reach, but the set is harmless; the next local attach
/// overwrites it.
///
/// No-op when the control plane is absent (no `ControlPlaneHandle`).
fn refresh_ozma_sock(mut client: TmuxClientMut<'_, '_>, control: Option<Res<ControlPlaneHandle>>) {
    let Some(control) = control else {
        return;
    };
    let sock = control.sock_path.to_string_lossy();
    if let Err(e) = client.send(SetEnvironmentGlobal {
        key: "OZMA_SOCK",
        value: sock.as_ref(),
    }) {
        tracing::warn!(?e, "failed to set global $OZMA_SOCK");
    }
}

/// Sets `$OZMA_SOCK` at session scope for each tmux session as it appears or the
/// attached session changes.
///
/// Gated on `Changed<TmuxSession>`, so it fires on the initial projection and
/// again whenever the control client switches to (or is re-attached to) a
/// different session — `on_session_changed` reuses one session entity, so a
/// switch is a component re-insert, not an `Added` edge. The global set in
/// [`refresh_ozma_sock`] only reaches the process environment of panes spawned
/// *after* it and is invisible to session-scope `show-environment`; a pane that
/// forked earlier (the adopted control-mode pane, or any pane of a pre-existing
/// session switched into) therefore cannot see it. tmux injects `$TMUX` into
/// every pane, so the ratatui-ozma SDK recovers the value with
/// `tmux show-environment OZMA_SOCK` — but only when it is set at session scope,
/// which this system does. The session is targeted by id (`$N`), which is stable
/// and unique, rather than name, which may be empty or duplicated.
///
/// No-op when the control plane is absent (no `ControlPlaneHandle`).
fn bind_ozma_sock_to_session(
    mut client: TmuxClientMut<'_, '_>,
    sessions: Query<&TmuxSession, Changed<TmuxSession>>,
    control: Option<Res<ControlPlaneHandle>>,
) {
    let Some(control) = control else {
        return;
    };
    let sock = control.sock_path.to_string_lossy();
    for session in sessions.iter() {
        let target = format!("${}", session.id.0);
        if let Err(e) = client.send(SetEnvironmentInSession {
            session: &target,
            key: "OZMA_SOCK",
            value: sock.as_ref(),
        }) {
            tracing::warn!(?e, session = %target, "failed to set session $OZMA_SOCK");
        }
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
        .query::<(Entity, &mut TmuxClient)>()
        .single_mut(world)
        .ok()
        .and_then(|(gateway, mut client)| {
            let _ = client.send(UnsetEnvironmentGlobal { key: "OZMA_SOCK" });
            let bytes = client.take_outgoing();
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
    use ozmux_tmux::{PaneId, SessionId};
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

        let gateway = app.world_mut().spawn(TmuxClient::new_adopted()).id();

        app.insert_resource(ControlPlaneHandle {
            sock_path: PathBuf::from("/tmp/ctl.sock"),
            tokens: TokenRegistry::default(),
        });

        app.add_systems(Update, refresh_ozma_sock.run_if(tmux_client_added));
        app.update();

        let out = app
            .world_mut()
            .get_mut::<TmuxClient>(gateway)
            .unwrap()
            .take_outgoing();
        assert_eq!(
            out, b"set-environment -g OZMA_SOCK /tmp/ctl.sock\n",
            "refresh sends a global set-environment over the adopted connection"
        );
    }

    #[test]
    fn session_added_sends_session_scoped_set_environment() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);

        let gateway = app.world_mut().spawn(TmuxClient::new_adopted()).id();

        app.insert_resource(ControlPlaneHandle {
            sock_path: PathBuf::from("/tmp/ctl.sock"),
            tokens: TokenRegistry::default(),
        });

        app.add_systems(
            Update,
            bind_ozma_sock_to_session.run_if(tmux_session_changed),
        );

        app.world_mut().spawn(TmuxSession {
            id: SessionId(3),
            name: "work".into(),
        });
        app.update();

        let out = app
            .world_mut()
            .get_mut::<TmuxClient>(gateway)
            .unwrap()
            .take_outgoing();
        assert_eq!(
            out, b"set-environment -t '$3' OZMA_SOCK /tmp/ctl.sock\n",
            "a newly projected session gets a session-scoped set so show-environment recovers it"
        );
    }

    #[test]
    fn session_switch_resends_session_scoped_set_for_new_id() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);

        let gateway = app.world_mut().spawn(TmuxClient::new_adopted()).id();
        app.insert_resource(ControlPlaneHandle {
            sock_path: PathBuf::from("/tmp/ctl.sock"),
            tokens: TokenRegistry::default(),
        });
        app.add_systems(
            Update,
            bind_ozma_sock_to_session.run_if(tmux_session_changed),
        );

        let session = app
            .world_mut()
            .spawn(TmuxSession {
                id: SessionId(3),
                name: "work".into(),
            })
            .id();
        app.update();
        let _ = app
            .world_mut()
            .get_mut::<TmuxClient>(gateway)
            .unwrap()
            .take_outgoing();

        // A control-client switch re-inserts TmuxSession on the same entity with a
        // new id (no `Added` edge), mirroring `on_session_changed`.
        app.world_mut().entity_mut(session).insert(TmuxSession {
            id: SessionId(5),
            name: "other".into(),
        });
        app.update();

        let out = app
            .world_mut()
            .get_mut::<TmuxClient>(gateway)
            .unwrap()
            .take_outgoing();
        assert_eq!(
            out, b"set-environment -t '$5' OZMA_SOCK /tmp/ctl.sock\n",
            "switching to a different session re-sends the session-scoped set for the new id"
        );
    }

    #[test]
    fn session_set_is_no_op_without_control_plane() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);

        let gateway = app.world_mut().spawn(TmuxClient::new_adopted()).id();

        app.add_systems(
            Update,
            bind_ozma_sock_to_session.run_if(tmux_session_changed),
        );

        app.world_mut().spawn(TmuxSession {
            id: SessionId(1),
            name: String::new(),
        });
        app.update();

        let out = app
            .world_mut()
            .get_mut::<TmuxClient>(gateway)
            .unwrap()
            .take_outgoing();
        assert!(
            out.is_empty(),
            "no ControlPlaneHandle means no session set-environment is sent"
        );
    }

    #[test]
    fn refresh_is_no_op_without_control_plane() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);

        let gateway = app.world_mut().spawn(TmuxClient::new_adopted()).id();

        app.add_systems(Update, refresh_ozma_sock.run_if(tmux_client_added));
        app.update();

        let out = app
            .world_mut()
            .get_mut::<TmuxClient>(gateway)
            .unwrap()
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

        let gateway = app.world_mut().spawn(TmuxClient::new_adopted()).id();

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
    fn bind_pane_runs_in_default_mode_with_client() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        let tokens = TokenRegistry::default();
        app.insert_resource(ControlPlaneHandle {
            sock_path: PathBuf::from("/tmp/x.sock"),
            tokens: tokens.clone(),
        });
        app.world_mut().spawn(TmuxClient::new_adopted());
        app.add_systems(
            Update,
            bind_tmux_pane_tokens.run_if(any_with_component::<TmuxClient>),
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
            "%N token bound when a TmuxClient exists but no AppMode::Tmux state"
        );
    }
}
