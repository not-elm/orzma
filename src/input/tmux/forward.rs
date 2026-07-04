//! Routes `crate::action::terminal`'s `TerminalForwardInput` (backend-bound bytes from the
//! shared mouse apply observer) to the owning tmux pane via `send-keys -H`.

use crate::action::terminal::TerminalForwardInput;
use crate::input::ime::ImeCommit;
use bevy::prelude::*;
use ozma_tty_engine::TerminalHandle;
use ozmux_tmux::{PaneId, SendBytes, SendPaneKeys, TmuxClient, TmuxPane};

/// Registers the `TerminalForwardInput` → tmux `send-keys -H` observer.
pub(super) struct ForwardPlugin;

impl Plugin for ForwardPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(forward_pane_input)
            .add_observer(forward_ime_commit)
            .add_observer(on_forward_pane_keys);
    }
}

/// Carries a frame's accumulated tmux key names for the active pane, to be
/// delivered in a single `send-keys` call by `on_forward_pane_keys`.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct ForwardPaneKeysRequest {
    /// The active pane surface the keys are forwarded to.
    #[event_target]
    pub(crate) entity: Entity,
    /// The frame's tmux key names, in event order.
    pub(crate) names: Vec<String>,
}

fn forward_pane_input(
    ev: On<TerminalForwardInput>,
    mut client: Option<Single<&mut TmuxClient>>,
    panes: Query<&TmuxPane>,
) {
    let Ok(pane) = panes.get(ev.entity) else {
        return;
    };
    let Some(client) = client.as_deref_mut() else {
        return;
    };
    send_to_pane(
        client,
        pane.id,
        &ev.bytes,
        "tmux mouse-report forward failed",
    );
}

fn forward_ime_commit(
    ev: On<ImeCommit>,
    mut commands: Commands,
    mut client: Option<Single<&mut TmuxClient>>,
    mut handles: Query<&mut TerminalHandle>,
    panes: Query<&TmuxPane>,
) {
    let Ok(pane) = panes.get(ev.entity) else {
        return;
    };
    if let Ok(mut handle) = handles.get_mut(ev.entity)
        && handle.snap_to_bottom_vt_only()
    {
        handle.flush_emit(&mut commands, ev.entity);
    }
    let Some(client) = client.as_deref_mut() else {
        return;
    };
    send_to_pane(
        client,
        pane.id,
        ev.text.as_bytes(),
        "IME commit send failed",
    );
}

/// Delivers a frame's batched tmux key names to the target pane in one
/// `send-keys` call, snapping the pane's local view to the bottom first —
/// matching the plain-key forward path this observer relocates.
fn on_forward_pane_keys(
    ev: On<ForwardPaneKeysRequest>,
    mut commands: Commands,
    mut handles: Query<&mut TerminalHandle>,
    mut client: Option<Single<&mut TmuxClient>>,
    panes: Query<&TmuxPane>,
) {
    if ev.names.is_empty() {
        return;
    }
    let Ok(pane) = panes.get(ev.entity) else {
        return;
    };
    let target = pane_target(pane.id);
    if let Ok(mut handle) = handles.get_mut(ev.entity)
        && handle.snap_to_bottom_vt_only()
    {
        handle.flush_emit(&mut commands, ev.entity);
    }
    let Some(client) = client.as_deref_mut() else {
        return;
    };
    if let Err(e) = client.send(SendPaneKeys {
        pane: &target,
        names: &ev.names,
    }) {
        tracing::warn!(?e, "tmux key forward failed");
    }
}

/// Sends `bytes` to tmux pane `pane` via `send-keys -H`, logging `context` on
/// failure.
fn send_to_pane(client: &mut TmuxClient, pane: PaneId, bytes: &[u8], context: &str) {
    let target = pane_target(pane);
    if let Err(e) = client.send(SendBytes {
        pane: &target,
        bytes,
    }) {
        tracing::warn!(?e, "{context}");
    }
}

/// The tmux target string (`%<id>`) for a pane id.
fn pane_target(pane: PaneId) -> String {
    format!("%{}", pane.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ozmux_tmux::TmuxCommand;

    #[test]
    fn forward_builds_send_keys_hex_for_pane() {
        let target = pane_target(PaneId(3));
        let cmd = SendBytes {
            pane: &target,
            bytes: b"\x1b[<0;1;1M",
        }
        .into_raw_command();
        assert_eq!(cmd, "send-keys -H -t %3 1b 5b 3c 30 3b 31 3b 31 4d");
    }

    #[test]
    fn ime_commit_observer_no_panic_without_client() {
        use crate::input::ime::ImeCommit;
        use ozma_tty_engine::TerminalHandle;

        use tmux_control_parser::{CellDims, PaneId};

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_observer(forward_ime_commit);

        let pane = app
            .world_mut()
            .spawn((
                TmuxPane {
                    id: PaneId(1),
                    dims: CellDims {
                        width: 10,
                        height: 5,
                        xoff: 0,
                        yoff: 0,
                    },
                },
                TerminalHandle::detached(10, 5),
            ))
            .id();
        // No live tmux client: the send is skipped; assert no panic.
        app.world_mut().trigger(ImeCommit {
            entity: pane,
            text: "あ".into(),
        });
        app.update();
    }

    #[test]
    fn forward_pane_keys_sends_one_batched_send_keys() {
        use tmux_control_parser::{CellDims, PaneId};

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_observer(on_forward_pane_keys);

        let client = app.world_mut().spawn(TmuxClient::new_adopted()).id();
        let pane = app
            .world_mut()
            .spawn((
                TmuxPane {
                    id: PaneId(7),
                    dims: CellDims {
                        width: 10,
                        height: 5,
                        xoff: 0,
                        yoff: 0,
                    },
                },
                TerminalHandle::detached(10, 5),
            ))
            .id();
        app.world_mut().trigger(ForwardPaneKeysRequest {
            entity: pane,
            names: vec!["a".to_string(), "C-c".to_string()],
        });
        app.update();

        let mut client = app.world_mut().get_mut::<TmuxClient>(client).unwrap();
        let out = String::from_utf8(client.take_outgoing()).unwrap();
        assert_eq!(out, "send-keys -t %7 -- a C-c\n", "got {out:?}");
    }

    #[test]
    fn forward_pane_keys_observer_no_panic_without_client() {
        use tmux_control_parser::{CellDims, PaneId};

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_observer(on_forward_pane_keys);

        let pane = app
            .world_mut()
            .spawn((
                TmuxPane {
                    id: PaneId(2),
                    dims: CellDims {
                        width: 10,
                        height: 5,
                        xoff: 0,
                        yoff: 0,
                    },
                },
                TerminalHandle::detached(10, 5),
            ))
            .id();
        // No live tmux client: the send is skipped; assert no panic.
        app.world_mut().trigger(ForwardPaneKeysRequest {
            entity: pane,
            names: vec!["a".to_string()],
        });
        app.update();
    }
}
