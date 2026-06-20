//! Routes `ozma_terminal`'s `TerminalForwardInput` (backend-bound bytes from the
//! shared mouse apply observer) to the owning tmux pane via `send-keys -H`.

use bevy::prelude::*;
use ozma_terminal::TerminalForwardInput;
use ozmux_tmux::{PaneId, SendBytes, TmuxConnection, TmuxPane};

/// Registers the `TerminalForwardInput` → tmux `send-keys -H` observer.
pub(crate) struct ForwardPlugin;

impl Plugin for ForwardPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(forward_pane_input);
    }
}

fn forward_pane_input(
    ev: On<TerminalForwardInput>,
    panes: Query<&TmuxPane>,
    connection: NonSend<TmuxConnection>,
) {
    let Ok(pane) = panes.get(ev.entity) else {
        return;
    };
    let Some(client) = connection.client() else {
        return;
    };
    let target = pane_target(pane.id);
    if let Err(e) = client.handle().send(SendBytes {
        pane: &target,
        bytes: &ev.bytes,
    }) {
        tracing::warn!(?e, "tmux mouse-report forward failed");
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
}
