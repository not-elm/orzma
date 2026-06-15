//! Capture-driven copy-mode rendering for tmux panes.
//!
//! While a pane is in tmux copy mode its live `TerminalHandle` keeps advancing
//! (`route_tmux_output` never drops `%output`), but its emit to the rendered
//! grid is gated (see `tmux_render::route_tmux_output`). This plugin paints the
//! scrolled view instead: it polls `#{...}` copy-mode state, captures the
//! scrolled viewport with `capture-pane`, and feeds the captured bytes into a
//! per-pane scratch handle whose `flush_emit` rebuilds the pane's `TerminalGrid`.
//!
//! Reply correlation rides the crate-side [`CopyModeQueries`] / [`CopyModeReply`]
//! channel: the binary registers each command by `CommandId`, and `ozmux_tmux`
//! (the single transport drainer) surfaces the matched reply as a message here.
//! Cursor/selection overlay (Task 9) and the clipboard bridge (Task 10) read the
//! stashed [`CopyModeSnapshot`] / handle the `Buffer` reply later.

use crate::ui::copy_mode::CopyModeState;
use bevy::prelude::*;
use ozma_tty_engine::TerminalHandle;
use ozmux_tmux::{
    CopyModeQueries, CopyModeReply, CopyQueryKind, CopyState, PaneId, TmuxConnection, TmuxPane,
    TmuxProjectionSet, copy_mode_capture_command, copy_state_query_command, parse_copy_state,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// Wires the capture-driven copy-mode refresh systems after the projection chain.
pub struct OzmuxTmuxCopyModePlugin;

impl Plugin for OzmuxTmuxCopyModePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CopyRefreshState>();
        app.add_systems(
            Update,
            (
                issue_copy_state.run_if(any_pane_in_copy_mode),
                consume_copy_reply.run_if(on_message::<CopyModeReply>),
            )
                .chain()
                .after(TmuxProjectionSet),
        );
    }
}

/// Bookkeeping for the copy-mode refresh loop: which panes have a state query in
/// flight (so at most one is outstanding per pane) and the last captured
/// `scroll_position` per pane (so an unchanged viewport skips re-capturing).
#[derive(Resource, Default)]
struct CopyRefreshState {
    state_in_flight: HashSet<PaneId>,
    last_scroll: HashMap<PaneId, u32>,
}

/// The latest copy-mode state snapshot for a pane, stashed for the overlay step
/// (Task 9). Updated each `State` reply.
// TODO: Task 9 reads this to map the cursor/selection into the rendered grid.
#[expect(dead_code, reason = "consumed by the Task 9 cursor/selection overlay")]
#[derive(Component)]
struct CopyModeSnapshot(CopyState);

/// A per-pane scratch terminal used only to parse `capture-pane` bytes into the
/// pane's rendered grid while in copy mode. The pane's live handle stays
/// untouched (it keeps streaming `%output`); this handle is what `flush_emit`
/// paints onto the pane entity's `TerminalGrid` for the scrolled view.
#[derive(Component)]
struct CopyRenderHandle(TerminalHandle);

/// True while at least one pane carries `CopyModeState`. Gates the state-query
/// system so it does not run (or acquire its data) when copy mode is inactive.
fn any_pane_in_copy_mode(copy_modes: Query<(), With<CopyModeState>>) -> bool {
    !copy_modes.is_empty()
}

/// Issues one `display-message` copy-state query per in-copy-mode pane, coalesced
/// to at most one in-flight query per pane. Gated by [`any_pane_in_copy_mode`].
fn issue_copy_state(
    mut refresh: ResMut<CopyRefreshState>,
    mut queries: ResMut<CopyModeQueries>,
    connection: NonSend<TmuxConnection>,
    panes: Query<&TmuxPane, With<CopyModeState>>,
) {
    let Some(client) = connection.client() else {
        return;
    };
    for pane in panes.iter() {
        if !refresh.state_in_flight.insert(pane.id) {
            continue;
        }
        match client.handle().send(&copy_state_query_command(pane.id)) {
            Ok(id) => queries.register(id, pane.id, CopyQueryKind::State),
            Err(error) => {
                refresh.state_in_flight.remove(&pane.id);
                tracing::warn!(?error, pane = pane.id.0, "copy-state query send failed");
            }
        }
    }
}

/// Applies each correlated [`CopyModeReply`]: a `State` reply drives exit (when
/// the pane left the mode) or a follow-up `capture-pane`; a `Capture` reply
/// repaints the pane's grid from the scrolled view. `Buffer` is ignored here
/// (Task 10 handles the clipboard). Gated by `run_if(on_message::<CopyModeReply>)`.
fn consume_copy_reply(
    mut commands: Commands,
    mut refresh: ResMut<CopyRefreshState>,
    mut queries: ResMut<CopyModeQueries>,
    mut replies: MessageReader<CopyModeReply>,
    mut render_handles: Query<&mut CopyRenderHandle>,
    connection: NonSend<TmuxConnection>,
    panes: Query<(Entity, &TmuxPane)>,
) {
    let entity_of: HashMap<PaneId, Entity> = panes.iter().map(|(e, p)| (p.id, e)).collect();
    for reply in replies.read() {
        let Some(&entity) = entity_of.get(&reply.pane) else {
            continue;
        };
        match reply.kind {
            CopyQueryKind::State => {
                refresh.state_in_flight.remove(&reply.pane);
                apply_state_reply(
                    &mut commands,
                    &mut refresh,
                    &mut queries,
                    &connection,
                    entity,
                    reply,
                );
            }
            CopyQueryKind::Capture => {
                apply_capture_reply(&mut commands, &mut render_handles, entity, reply);
            }
            CopyQueryKind::Buffer => {}
        }
    }
}

/// Handles a `State` reply: on `pane_in_mode == 0` removes `CopyModeState` (the
/// live grid resumes via the gated emit in `route_tmux_output`); otherwise
/// stashes the snapshot and, when the scrolled region changed, issues a capture.
fn apply_state_reply(
    commands: &mut Commands,
    refresh: &mut CopyRefreshState,
    queries: &mut CopyModeQueries,
    connection: &TmuxConnection,
    entity: Entity,
    reply: &CopyModeReply,
) {
    if !reply.ok {
        return;
    }
    let Some(state) = reply.output.first().and_then(|line| parse_copy_state(line)) else {
        return;
    };
    if !state.pane_in_mode {
        commands.entity(entity).remove::<CopyModeState>();
        commands.entity(entity).remove::<CopyModeSnapshot>();
        refresh.last_scroll.remove(&reply.pane);
        return;
    }
    commands.entity(entity).insert(CopyModeSnapshot(state));
    let changed = refresh.last_scroll.get(&reply.pane) != Some(&state.scroll_position);
    if !changed {
        return;
    }
    let Some(client) = connection.client() else {
        return;
    };
    match client.handle().send(&copy_mode_capture_command(
        reply.pane,
        state.scroll_position,
        state.pane_height,
    )) {
        Ok(id) => {
            queries.register(id, reply.pane, CopyQueryKind::Capture);
            refresh
                .last_scroll
                .insert(reply.pane, state.scroll_position);
        }
        Err(error) => tracing::warn!(?error, pane = reply.pane.0, "copy-capture send failed"),
    }
}

/// Handles a `Capture` reply: feeds the captured bytes into the pane's
/// `CopyRenderHandle` (created/resized to the pane on demand) and emits, so the
/// pane's `TerminalGrid` shows the scrolled copy-mode view.
fn apply_capture_reply(
    commands: &mut Commands,
    render_handles: &mut Query<&mut CopyRenderHandle>,
    entity: Entity,
    reply: &CopyModeReply,
) {
    if !reply.ok {
        return;
    }
    let bytes = capture_to_bytes(&reply.output);
    let (cols, rows) = capture_dims(&reply.output);
    if let Ok(mut render) = render_handles.get_mut(entity) {
        let (cur_cols, cur_rows, _) = render.0.read_geometry();
        if (cur_cols, cur_rows) != (cols, rows) {
            render.0.resize_grid_only(cols, rows);
        }
        render.0.advance(&bytes);
        render.0.flush_emit(commands, entity);
        return;
    }
    let mut handle = TerminalHandle::detached(cols, rows, Arc::new(AtomicBool::new(false)));
    handle.advance(&bytes);
    handle.flush_emit(commands, entity);
    commands.entity(entity).insert(CopyRenderHandle(handle));
}

/// Joins `capture-pane -p -e` reply lines into VT bytes for the scratch handle:
/// a cursor-home + clear-screen prefix so the snapshot repaints from a clean
/// grid, then the rows CRLF-joined (the reply omits line terminators). Mirrors
/// `ozmux_tmux`'s `capture_to_bytes` for the live seed path.
fn capture_to_bytes(lines: &[String]) -> Vec<u8> {
    let mut bytes = b"\x1b[H\x1b[2J".to_vec();
    bytes.extend_from_slice(lines.join("\r\n").as_bytes());
    bytes
}

/// Derives a scratch-grid size from a capture reply: the row count, and the
/// widest line's char count (clamped into `1..=u16::MAX`). The scrolled capture
/// already matches the pane's visible region, so its own extent sizes the grid.
fn capture_dims(lines: &[String]) -> (u16, u16) {
    let clamp = |v: usize| v.clamp(1, u16::MAX as usize) as u16;
    let rows = clamp(lines.len());
    let cols = clamp(lines.iter().map(|l| l.chars().count()).max().unwrap_or(1));
    (cols, rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_to_bytes_prefixes_reset_and_crlf_joins() {
        let lines = vec!["row one".to_string(), "row two".to_string()];
        assert_eq!(
            capture_to_bytes(&lines),
            b"\x1b[H\x1b[2Jrow one\r\nrow two".to_vec(),
        );
    }

    #[test]
    fn capture_to_bytes_empty_is_just_the_reset() {
        assert_eq!(capture_to_bytes(&[]), b"\x1b[H\x1b[2J".to_vec());
    }

    #[test]
    fn capture_dims_uses_widest_line_and_row_count() {
        let lines = vec!["abc".to_string(), "abcdef".to_string(), "".to_string()];
        assert_eq!(capture_dims(&lines), (6, 3));
    }

    #[test]
    fn capture_dims_clamps_empty_to_one_by_one() {
        assert_eq!(capture_dims(&[]), (1, 1));
        assert_eq!(capture_dims(&["".to_string()]), (1, 1));
    }
}
