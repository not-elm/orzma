//! Apply observer for tmux mouse effects.
//!
//! Receives `TmuxMouseEffects` triggered by the arbiter and sends the
//! corresponding tmux control-mode commands in the same order as the
//! pre-refactor `arbiter` body. Bookkeeping on `TmuxMouseGesture` is done
//! here exactly when a send succeeds, preserving invariant 8.

use super::TmuxMouseGesture;
use super::effect::{MultiSelectKind, TmuxMouseEffect, TmuxMouseEffects};
use crate::tmux::copy_mode::cursor_deltas;
use bevy::prelude::*;
use ozmux_tmux::{
    CopyModeQueries, CopyQueryKind, PaneId, TmuxConnection, resize_pane_x_command,
    resize_pane_y_command, select_pane_command, show_buffer_command,
};
use tmux_control_parser::DividerAxis;

/// Observer that applies a frame's decided `TmuxMouseEffects` by sending the
/// corresponding tmux control-mode commands. Sends are gated on an active
/// client; when none is present every effect is a no-op and the gesture state
/// is left unchanged.
pub(super) fn on_tmux_mouse_effects(
    ev: On<TmuxMouseEffects>,
    mut queries: ResMut<CopyModeQueries>,
    mut gesture: ResMut<TmuxMouseGesture>,
    connection: NonSend<TmuxConnection>,
) {
    let Some(client) = connection.client() else {
        return;
    };
    let handle = client.handle();
    for effect in &ev.effects {
        match *effect {
            TmuxMouseEffect::SelectPane(pane_id) => {
                let cmd = select_pane_command(pane_id);
                if let Err(e) = handle.send(&cmd) {
                    tracing::warn!(?e, pane = pane_id.0, "select-pane send failed");
                }
            }
            TmuxMouseEffect::ResizePane {
                axis,
                primary,
                size,
            } => {
                let cmd = match axis {
                    DividerAxis::Vertical => resize_pane_x_command(primary, size),
                    DividerAxis::Horizontal => resize_pane_y_command(primary, size),
                };
                if let Err(e) = handle.send(&cmd) {
                    tracing::warn!(?e, pane = primary.0, "resize-pane send failed");
                    continue;
                }
                if let super::GestureState::Resizing {
                    last_sent, resized, ..
                } = &mut gesture.state
                {
                    *last_sent = size;
                    *resized = true;
                }
            }
            TmuxMouseEffect::BeginCopyDrag {
                pane,
                snapshot_cursor,
                anchor,
            } => {
                for cmd in cursor_deltas(snapshot_cursor, anchor) {
                    if let Err(e) = handle.send(&target_copy_cmd(pane, &cmd)) {
                        tracing::warn!(?e, pane = pane.0, "drag-select anchor delta send failed");
                    }
                }
                if let Err(e) = handle.send(&target_copy_cmd(pane, "send-keys -X begin-selection"))
                {
                    tracing::warn!(?e, pane = pane.0, "drag-select begin-selection send failed");
                } else if let super::GestureState::Selecting {
                    begun, last_target, ..
                } = &mut gesture.state
                {
                    *begun = true;
                    *last_target = Some(anchor);
                }
            }
            TmuxMouseEffect::ExtendCopyDrag {
                pane,
                snapshot_cursor,
                cell,
            } => {
                for cmd in cursor_deltas(snapshot_cursor, cell) {
                    if let Err(e) = handle.send(&target_copy_cmd(pane, &cmd)) {
                        tracing::warn!(?e, pane = pane.0, "drag-select extend delta send failed");
                    }
                }
                if let super::GestureState::Selecting { last_target, .. } = &mut gesture.state {
                    *last_target = Some(cell);
                }
            }
            TmuxMouseEffect::MultiSelect {
                pane,
                kind,
                snapshot_cursor,
                cell,
            } => {
                for cmd in multi_select_commands(kind, snapshot_cursor, cell, pane) {
                    if let Err(e) = handle.send(&cmd) {
                        tracing::warn!(?e, pane = pane.0, "multi-select cmd send failed");
                    }
                }
                match handle.send(&show_buffer_command()) {
                    Ok(id) => queries.register(id, pane, CopyQueryKind::Buffer),
                    Err(e) => {
                        tracing::warn!(?e, pane = pane.0, "multi-select show-buffer send failed")
                    }
                }
            }
            TmuxMouseEffect::CopySelection { pane } => {
                let copy = target_copy_cmd(pane, "send-keys -X copy-selection");
                if let Err(e) = handle.send(&copy) {
                    tracing::warn!(?e, pane = pane.0, "drag-select copy-selection send failed");
                } else {
                    match handle.send(&show_buffer_command()) {
                        Ok(id) => queries.register(id, pane, CopyQueryKind::Buffer),
                        Err(e) => {
                            tracing::warn!(?e, pane = pane.0, "drag-select show-buffer send failed")
                        }
                    }
                }
            }
        }
    }
}

/// Inserts `-t %<id>` into a `send-keys -X ...` copy-mode command so it targets
/// a specific pane instead of the client's active pane. Non-`send-keys -X`
/// commands are returned unchanged.
fn target_copy_cmd(pane: PaneId, cmd: &str) -> String {
    match cmd.strip_prefix("send-keys -X") {
        Some(rest) => format!("send-keys -X -t %{}{}", pane.0, rest),
        None => cmd.to_string(),
    }
}

/// Pane-targeted copy-mode commands to position the copy cursor at `cell`
/// (relative to the snapshot cursor) and select a word/line. Does NOT include
/// `show-buffer` — the caller sends that separately to register the reply.
fn multi_select_commands(
    kind: MultiSelectKind,
    snapshot_cursor: (u16, u16),
    cell: (u16, u16),
    pane: PaneId,
) -> Vec<String> {
    let mut out: Vec<String> = cursor_deltas(snapshot_cursor, cell)
        .iter()
        .map(|c| target_copy_cmd(pane, c))
        .collect();
    let select = match kind {
        MultiSelectKind::Word => "send-keys -X select-word",
        MultiSelectKind::Line => "send-keys -X select-line",
    };
    out.push(target_copy_cmd(pane, select));
    out.push(target_copy_cmd(pane, "send-keys -X copy-selection"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ozmux_tmux::TmuxConnection;

    #[test]
    fn observer_applies_without_panic_when_no_client() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_non_send_resource(TmuxConnection::default())
            .init_resource::<CopyModeQueries>()
            .init_resource::<TmuxMouseGesture>()
            .add_observer(on_tmux_mouse_effects);
        let e = app.world_mut().spawn_empty().id();
        app.world_mut().trigger(TmuxMouseEffects {
            entity: e,
            effects: vec![
                TmuxMouseEffect::SelectPane(PaneId(1)),
                TmuxMouseEffect::CopySelection { pane: PaneId(1) },
            ],
        });
    }

    #[test]
    fn multi_select_commands_match_current_bytes() {
        let cmds = multi_select_commands(MultiSelectKind::Word, (0, 0), (2, 0), PaneId(3));
        assert!(cmds.iter().any(|c| c == "send-keys -X -t %3 select-word"));
        assert!(cmds.last().unwrap() == "send-keys -X -t %3 copy-selection");
    }

    #[test]
    fn multi_select_word_commands() {
        let cmds = multi_select_commands(MultiSelectKind::Word, (0, 0), (3, 0), PaneId(2));
        assert_eq!(
            cmds,
            vec![
                "send-keys -X -t %2 -N 3 cursor-right".to_string(),
                "send-keys -X -t %2 select-word".to_string(),
                "send-keys -X -t %2 copy-selection".to_string(),
            ]
        );
    }

    #[test]
    fn target_copy_cmd_inserts_pane_target_after_send_keys_x() {
        assert_eq!(
            target_copy_cmd(PaneId(2), "send-keys -X begin-selection"),
            "send-keys -X -t %2 begin-selection",
        );
    }

    #[test]
    fn target_copy_cmd_preserves_flags_after_send_keys_x() {
        assert_eq!(
            target_copy_cmd(PaneId(2), "send-keys -X -N 3 cursor-right"),
            "send-keys -X -t %2 -N 3 cursor-right",
        );
    }

    #[test]
    fn target_copy_cmd_passes_non_matching_through() {
        assert_eq!(
            target_copy_cmd(PaneId(2), "copy-mode -t %2"),
            "copy-mode -t %2",
        );
    }
}
