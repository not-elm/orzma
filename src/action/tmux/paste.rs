//! Tmux paste: applies `PasteAction` for a tmux pane by chunking the
//! clipboard into `SendBytes`, relocated from an inline match arm in the
//! tmux keyboard-input dispatcher.

use crate::action::terminal::PasteText;
use crate::clipboard::build_paste_bytes;
use crate::input::tmux::forward::{pane_target, snap_pane_to_bottom};
use bevy::prelude::*;
use orzma_tmux::{SendBytes, TmuxClient, TmuxPane};
use orzma_tty_engine::TerminalHandle;

/// Registers the tmux `PasteAction` apply observer.
pub(super) struct TmuxPastePlugin;

impl Plugin for TmuxPastePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_paste_tmux);
    }
}

/// tmux `send-keys -H` caps its argument count; paste bytes are sent in
/// fixed-size chunks so a large clipboard doesn't overflow one command.
const PASTE_CHUNK_BYTES: usize = 256;

fn on_paste_tmux(
    ev: On<PasteText>,
    mut commands: Commands,
    mut handles: Query<&mut TerminalHandle>,
    mut client: Option<Single<&mut TmuxClient>>,
    panes: Query<&TmuxPane>,
) {
    let Ok(pane) = panes.get(ev.entity) else {
        return;
    };
    // NOTE: gate on a live client BEFORE snap/flush — a scrolled-back pane
    // with no client must not snap-to-bottom + repaint while pasting nothing.
    let Some(client) = client.as_deref_mut() else {
        return;
    };
    let target = pane_target(pane.id);
    snap_pane_to_bottom(&mut commands, &mut handles, ev.entity);
    let bytes = build_paste_bytes(&ev.text, false);
    for chunk in bytes.chunks(PASTE_CHUNK_BYTES) {
        if let Err(e) = client.send(SendBytes {
            pane: &target,
            bytes: chunk,
        }) {
            tracing::warn!(?e, "paste send failed");
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orzma_tmux::PaneId;
    use tmux_control_parser::CellDims;

    fn spawn_pane(app: &mut App, id: u32) -> Entity {
        app.world_mut()
            .spawn((
                TmuxPane {
                    id: PaneId(id),
                    dims: CellDims {
                        width: 10,
                        height: 5,
                        xoff: 0,
                        yoff: 0,
                    },
                },
                TerminalHandle::detached(10, 5),
            ))
            .id()
    }

    #[test]
    fn on_paste_tmux_without_client_does_not_snap_scrolled_back_pane() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins).add_plugins(TmuxPastePlugin);

        // Build a pane scrolled back into history: feed more lines than the
        // viewport holds, then scroll one line up so display_offset > 0.
        let mut handle = TerminalHandle::detached(10, 5);
        for _ in 0..20 {
            handle.advance(b"line\r\n");
        }
        handle.scroll_vt_only(1);
        assert!(
            !handle.is_at_bottom(),
            "fixture precondition: pane must start scrolled back",
        );
        let pane = app
            .world_mut()
            .spawn((
                TmuxPane {
                    id: PaneId(3),
                    dims: CellDims {
                        width: 10,
                        height: 5,
                        xoff: 0,
                        yoff: 0,
                    },
                },
                handle,
            ))
            .id();

        // No live tmux client: the early `else { return }` runs BEFORE
        // snap/flush, so the scrolled-back viewport is left untouched (no
        // repaint of a paste that never happened) — and no panic.
        app.world_mut().trigger(PasteText {
            entity: pane,
            text: "hello".to_string(),
        });
        app.update();

        let handle = app.world().get::<TerminalHandle>(pane).unwrap();
        assert!(
            !handle.is_at_bottom(),
            "no-client paste must not snap a scrolled-back pane to the bottom",
        );
    }

    #[test]
    fn on_paste_tmux_chunks_large_clipboard_into_multiple_send_bytes() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins).add_plugins(TmuxPastePlugin);
        let text = "a".repeat(PASTE_CHUNK_BYTES * 2 + 10);
        let client = app.world_mut().spawn(TmuxClient::new_adopted()).id();
        let pane = spawn_pane(&mut app, 5);

        app.world_mut().trigger(PasteText { entity: pane, text });
        app.update();

        let mut client = app.world_mut().get_mut::<TmuxClient>(client).unwrap();
        let out = client.take_outgoing();
        let sends = out
            .split(|&b| b == b'\n')
            .filter(|line| !line.is_empty())
            .count();
        assert_eq!(
            sends, 3,
            "a clipboard spanning two full chunks plus a remainder must be sent as three send-keys commands"
        );
    }
}
