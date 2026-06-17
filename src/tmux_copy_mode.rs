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

use crate::clipboard::Clipboard;
use crate::tmux_render::TerminalRenderRef;
use crate::ui::copy_mode::CopyModeState;
use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use ozma_tty_engine::TerminalHandle;
use ozma_tty_renderer::schema::{
    SelectionKind, SelectionRange, TerminalGrid, ViCursor, ViewportPoint,
};
use ozmux_tmux::{
    CopyModeQueries, CopyModeReply, CopyQueryKind, CopyState, PaneId, TmuxConnection, TmuxPane,
    TmuxProjectionSet, absolute_to_visible_row, copy_mode_capture_command,
    copy_state_query_command, parse_copy_state,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// Wires the capture-driven copy-mode refresh systems after the projection chain.
pub struct OzmuxTmuxCopyModePlugin;

impl Plugin for OzmuxTmuxCopyModePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CopyRefreshState>();
        app.add_observer(on_copy_mode_exit);
        app.add_systems(
            Update,
            (
                issue_copy_state.run_if(any_pane_in_copy_mode),
                consume_copy_reply.run_if(on_message::<CopyModeReply>),
                // NOTE: .after(consume_copy_reply) is load-bearing — the ApplyDeferred
                // between them flushes the capture FrameSnapshot (which clears
                // vi_cursor/selection) before this system re-asserts the overlay.
                apply_copy_overlay
                    .run_if(any_with_component::<CopyModeSnapshot>)
                    .after(consume_copy_reply),
            )
                .after(TmuxProjectionSet),
        );
    }
}

/// Bookkeeping for the copy-mode refresh loop: per-pane the number of updates a
/// state query has been outstanding (so at most one is in flight, but a stale one
/// is re-issued — see `issue_copy_state`), and the last captured `scroll_position`
/// per pane (so an unchanged viewport skips re-capturing).
#[derive(Resource, Default)]
struct CopyRefreshState {
    state_in_flight: HashMap<PaneId, u32>,
    last_scroll: HashMap<PaneId, u32>,
}

/// Updates a still-in-flight state query waits before `issue_copy_state` re-sends
/// it. The reply normally clears the entry in `consume_copy_reply`; the re-send is
/// a backstop for the rare case where a reply is never correlated back (a parser
/// gap, or a command tmux never answers), which would otherwise freeze the refresh
/// until the pane exits and re-enters copy mode. The transport reader is a
/// continuous loop, so a reply tmux has emitted is always read — this is not a
/// flush of "stuck" traffic, just a fresh query whose reply can still land.
const STALE_STATE_RESEND_UPDATES: u32 = 12;

/// The latest copy-mode state snapshot for a pane. Written only when the state
/// changes (so `Changed<CopyModeSnapshot>` is meaningful), and read back to
/// diff against the next reply.
#[derive(Component)]
pub(crate) struct CopyModeSnapshot(pub(crate) CopyState);

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

/// Observer for `On<Remove, CopyModeState>`. Fires for every copy-mode exit —
/// `pane_in_mode==0`, an input-driven `CopyAction::Exit`, and despawn (e.g.
/// `TmuxConnectionReset`). Forces a FULL repaint of the pane's live handle so
/// the rendered grid switches back from the captured scrolled view to live
/// content (`route_tmux_output` only emits on new `%output`, so an idle pane
/// would otherwise stay frozen on capture content, and a later delta would
/// paint over it), drops the scratch `CopyRenderHandle`, and prunes this pane's
/// refresh bookkeeping (otherwise a stale `PaneId` wedges `issue_copy_state`'s
/// coalescing guard and blocks re-entry capture at the same scroll position).
fn on_copy_mode_exit(
    ev: On<Remove, CopyModeState>,
    mut commands: Commands,
    mut refresh: ResMut<CopyRefreshState>,
    mut live_handles: Query<(&mut TerminalHandle, Option<&TerminalRenderRef>)>,
    panes: Query<&TmuxPane>,
) {
    let entity = ev.entity;
    if let Ok((mut handle, maybe_ref)) = live_handles.get_mut(entity) {
        let target = maybe_ref.map(|r| r.0).unwrap_or(entity);
        handle.repaint_full(&mut commands, target);
    }
    commands
        .entity(entity)
        .remove::<CopyRenderHandle>()
        .remove::<CopyModeSnapshot>();
    if let Ok(pane) = panes.get(entity) {
        refresh.state_in_flight.remove(&pane.id);
        refresh.last_scroll.remove(&pane.id);
    }
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
    // NOTE: re-send a state query that has been in flight for too long. The reply
    // normally clears the entry in consume_copy_reply; the re-send is a backstop
    // for the rare case where a reply is never correlated back (a parser gap, or a
    // command tmux never answers), which would otherwise freeze the refresh until
    // the pane exits and re-enters copy mode.
    for pane in panes.iter() {
        let send_now = match refresh.state_in_flight.get_mut(&pane.id) {
            None => true,
            Some(age) => {
                *age += 1;
                *age >= STALE_STATE_RESEND_UPDATES
            }
        };
        if !send_now {
            continue;
        }
        match client.handle().send(&copy_state_query_command(pane.id)) {
            Ok(id) => {
                queries.register(id, pane.id, CopyQueryKind::State);
                refresh.state_in_flight.insert(pane.id, 0);
            }
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
    mut clipboard: ResMut<Clipboard>,
    mut render_handles: Query<&mut CopyRenderHandle>,
    connection: NonSend<TmuxConnection>,
    panes: Query<(Entity, &TmuxPane, Option<&TerminalRenderRef>)>,
    copy_modes: Query<(), With<CopyModeState>>,
    snapshots: Query<&CopyModeSnapshot>,
) {
    let entity_of: HashMap<PaneId, (Entity, Option<Entity>)> = panes
        .iter()
        .map(|(e, p, r)| (p.id, (e, r.map(|rr| rr.0))))
        .collect();
    for reply in replies.read() {
        let Some(&(entity, maybe_render)) = entity_of.get(&reply.pane) else {
            continue;
        };
        match reply.kind {
            CopyQueryKind::State => {
                refresh.state_in_flight.remove(&reply.pane);
                // NOTE: a State/Capture reply can land AFTER the pane left copy
                // mode (the query was in flight at exit). Ignoring it is required:
                // applying a stale Capture would re-create the CopyRenderHandle and
                // repaint scrolled content over the live grid the exit observer just
                // restored. (Buffer replies are still applied below — a copy's
                // buffer is valid after the copy-and-cancel that ended the mode.)
                if copy_modes.get(entity).is_err() {
                    continue;
                }
                let stored = snapshots.get(entity).map(|s| s.0).ok();
                apply_state_reply(
                    &mut commands,
                    &mut refresh,
                    &mut queries,
                    &connection,
                    entity,
                    reply,
                    stored.as_ref(),
                );
            }
            CopyQueryKind::Capture => {
                if copy_modes.get(entity).is_err() {
                    continue;
                }
                let render_target = maybe_render.unwrap_or(entity);
                apply_capture_reply(&mut commands, &mut render_handles, entity, render_target, reply);
            }
            CopyQueryKind::Buffer => {
                if reply.ok {
                    clipboard.write(buffer_reply_to_text(&reply.output));
                }
            }
        }
    }
}

/// Handles a `State` reply: on `pane_in_mode == 0` removes `CopyModeState` (the
/// `on_copy_mode_exit` observer then forces the live repaint and prunes refresh
/// state); otherwise stashes the snapshot — only when it changed, so a steady
/// state does not re-mark `Changed` every frame — and, when the scrolled region
/// changed, issues a capture. `stored` is the pane's current snapshot, if any.
fn apply_state_reply(
    commands: &mut Commands,
    refresh: &mut CopyRefreshState,
    queries: &mut CopyModeQueries,
    connection: &TmuxConnection,
    entity: Entity,
    reply: &CopyModeReply,
    stored: Option<&CopyState>,
) {
    if !reply.ok {
        return;
    }
    let Some(state) = reply.output.first().and_then(|line| parse_copy_state(line)) else {
        return;
    };
    if !state.pane_in_mode {
        commands.entity(entity).remove::<CopyModeState>();
        return;
    }
    if stored != Some(&state) {
        commands.entity(entity).insert(CopyModeSnapshot(state));
    }
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
/// pane's `TerminalGrid` shows the scrolled copy-mode view. `pane_entity` holds
/// `CopyRenderHandle`; `render_target` is the entity carrying `TerminalGrid`
/// (the `TerminalRenderRef` child, or the pane itself in tests without one).
fn apply_capture_reply(
    commands: &mut Commands,
    render_handles: &mut Query<&mut CopyRenderHandle>,
    pane_entity: Entity,
    render_target: Entity,
    reply: &CopyModeReply,
) {
    if !reply.ok {
        return;
    }
    let bytes = capture_to_bytes(&reply.output);
    let (cols, rows) = capture_dims(&reply.output);
    if let Ok(mut render) = render_handles.get_mut(pane_entity) {
        let (cur_cols, cur_rows, _) = render.0.read_geometry();
        if (cur_cols, cur_rows) != (cols, rows) {
            render.0.resize_grid_only(cols, rows);
        }
        render.0.advance(&bytes);
        render.0.flush_emit(commands, render_target);
        return;
    }
    let mut handle = TerminalHandle::detached(cols, rows, Arc::new(AtomicBool::new(false)));
    handle.advance(&bytes);
    handle.flush_emit(commands, render_target);
    commands.entity(pane_entity).insert(CopyRenderHandle(handle));
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

/// Writes the copy cursor and selection from `CopyModeSnapshot` onto the pane's
/// `TerminalGrid`. Runs each frame while any pane has a `CopyModeSnapshot`,
/// ordered after `consume_copy_reply`.
///
/// # Ordering
///
/// `flush_emit` (called by `consume_copy_reply`) clears `vi_cursor`/`selection`
/// because the capture render handle is not in vi mode. This system re-asserts
/// the overlay each frame after a capture paint. The conditional writes avoid
/// per-frame change notifications when no snapshot changed.
// NOTE: The conditional mutation is load-bearing: without it, every frame
// unconditionally marks the grid changed and triggers downstream repaint. If the
// guard is removed the renderer will repaint every frame even in steady state,
// defeating Bevy's change-detection optimizations for the glyph pipeline.
// NOTE: `CopyModeSnapshot` lives on the `TmuxPane` entity while `TerminalGrid`
// lives on the `TerminalRenderRef` child. The two queries are split so each
// entity is addressed at the correct level.
fn apply_copy_overlay(
    mut grids: Query<&mut TerminalGrid>,
    panes: Query<(&CopyModeSnapshot, Option<&TerminalRenderRef>)>,
) {
    for (snapshot, maybe_ref) in panes.iter() {
        let render_entity = maybe_ref.map(|r| r.0);
        let Some(target) = render_entity else {
            continue;
        };
        let Ok(mut grid) = grids.get_mut(target) else {
            continue;
        };
        let (vi_cursor, selection) = build_overlay(&snapshot.0);
        let want_cursor = Some(vi_cursor);
        if grid.vi_cursor != want_cursor {
            grid.vi_cursor = want_cursor;
        }
        if grid.selection != selection {
            grid.selection = selection;
        }
    }
}

/// Builds the `ViCursor` and optional `SelectionRange` overlay from a copy-mode
/// state snapshot. Maps `cursor_y` directly (it is already a visible viewport
/// row), and maps absolute selection rows through `absolute_to_visible_row`,
/// clamping off-screen endpoints to `-1` (above) or `pane_height` (below).
/// Rectangle selections render as `Char` in v1 (the grid schema has no block
/// selection kind).
fn build_overlay(state: &CopyState) -> (ViCursor, Option<SelectionRange>) {
    let vi_cursor = ViCursor {
        row: state.cursor_y as i16,
        column: state.cursor_x,
        in_scrollback: false,
    };
    let selection = state.selection_present.then(|| {
        let start_row =
            absolute_to_visible_row(state.sel_start_y, state.history_size, state.scroll_position);
        let end_row =
            absolute_to_visible_row(state.sel_end_y, state.history_size, state.scroll_position);
        SelectionRange {
            start: ViewportPoint {
                row: clamp_row(start_row, state.pane_height),
                column: state.sel_start_x,
            },
            end: ViewportPoint {
                row: clamp_row(end_row, state.pane_height),
                column: state.sel_end_x,
            },
            kind: SelectionKind::Char,
        }
    });
    (vi_cursor, selection)
}

/// Clamps a visible row to `-1` (above viewport) or `rows` (below) for
/// off-screen selection endpoints, matching `ViewportPoint`'s convention.
fn clamp_row(row: i32, rows: u16) -> i16 {
    row.clamp(-1, rows as i32) as i16
}

/// Joins `show-buffer` reply lines into the clipboard text string.
/// tmux strips trailing newlines from buffer content; the join preserves
/// internal newlines so multi-line selections paste correctly.
fn buffer_reply_to_text(lines: &[String]) -> String {
    lines.join("\n")
}

/// The `send-keys -X -N <n> cursor-<dir>` commands that move the copy cursor
/// from `cur` to `target` (visible cell coords). Empty when already there.
///
/// tmux has no primitive to set the copy cursor to an absolute `(x, y)`; the
/// only way to position it is relative motion. Each axis contributes at most
/// one command: a positive horizontal delta emits `cursor-right`, negative
/// `cursor-left`; a positive vertical delta emits `cursor-down`, negative
/// `cursor-up`. A zero delta on an axis emits nothing.
pub(crate) fn cursor_deltas(cur: (u16, u16), target: (u16, u16)) -> Vec<String> {
    let mut out = Vec::new();
    let (cx, cy) = (cur.0 as i32, cur.1 as i32);
    let (tx, ty) = (target.0 as i32, target.1 as i32);
    let dx = tx - cx;
    if dx > 0 {
        out.push(format!("send-keys -X -N {dx} cursor-right"));
    } else if dx < 0 {
        out.push(format!("send-keys -X -N {} cursor-left", -dx));
    }
    let dy = ty - cy;
    if dy > 0 {
        out.push(format!("send-keys -X -N {dy} cursor-down"));
    } else if dy < 0 {
        out.push(format!("send-keys -X -N {} cursor-up", -dy));
    }
    out
}

/// Maps a window cursor position (physical px) to the active `TmuxPane`'s
/// visible `(col, row)`, clamped to `[0, cols) × [0, rows)`. Returns `None` when
/// the projection is degenerate (zero-area node). The point is clamped (not
/// rejected) when it falls outside the pane so a drag that leaves the pane edge
/// still extends the selection to the nearest cell.
pub(crate) fn cell_at_pane(
    node: &ComputedNode,
    transform: &UiGlobalTransform,
    cursor_phys: Vec2,
    cell_w_phys: f32,
    cell_h_phys: f32,
    cols: u16,
    rows: u16,
) -> Option<(u16, u16)> {
    let local = phys_to_pane_local(node, transform, cursor_phys)?;
    let col = ((local.x / cell_w_phys).floor().max(0.0) as u32).min(cols.saturating_sub(1) as u32);
    let row = ((local.y / cell_h_phys).floor().max(0.0) as u32).min(rows.saturating_sub(1) as u32);
    Some((col as u16, row as u16))
}

/// Maps a window physical-pixel point to a node's local physical px with origin
/// at the node's top-left corner. Mirrors `tmux_pane_hit::phys_to_pane_local`
/// (the affine inverse of `UiGlobalTransform` via `ComputedNode::normalize_point`).
fn phys_to_pane_local(
    node: &ComputedNode,
    transform: &UiGlobalTransform,
    cursor_phys: Vec2,
) -> Option<Vec2> {
    node.normalize_point(*transform, cursor_phys)
        .map(|normalized| (normalized + Vec2::splat(0.5)) * node.size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_deltas_right_and_down() {
        assert_eq!(
            cursor_deltas((2, 3), (5, 7)),
            vec![
                "send-keys -X -N 3 cursor-right".to_string(),
                "send-keys -X -N 4 cursor-down".to_string(),
            ],
        );
    }

    #[test]
    fn cursor_deltas_left_and_up() {
        assert_eq!(
            cursor_deltas((5, 7), (2, 3)),
            vec![
                "send-keys -X -N 3 cursor-left".to_string(),
                "send-keys -X -N 4 cursor-up".to_string(),
            ],
        );
    }

    #[test]
    fn cursor_deltas_zero_is_empty() {
        assert!(cursor_deltas((4, 9), (4, 9)).is_empty());
    }

    #[test]
    fn cursor_deltas_pure_horizontal() {
        assert_eq!(
            cursor_deltas((1, 5), (8, 5)),
            vec!["send-keys -X -N 7 cursor-right".to_string()],
        );
    }

    #[test]
    fn cursor_deltas_pure_vertical() {
        assert_eq!(
            cursor_deltas((3, 9), (3, 2)),
            vec!["send-keys -X -N 7 cursor-up".to_string()],
        );
    }

    #[test]
    fn cell_at_pane_maps_and_clamps() {
        // A point at local (40, 48) with 8x16 px cells maps to col 5, row 3
        // (floor(40/8)=5, floor(48/16)=3); cols/rows bound the clamp, not the node.
        let node = ComputedNode {
            size: Vec2::new(640.0, 384.0),
            ..ComputedNode::DEFAULT
        };
        let transform = UiGlobalTransform::from_xy(320.0, 192.0);
        let cell = cell_at_pane(&node, &transform, Vec2::new(40.0, 48.0), 8.0, 16.0, 80, 24);
        assert_eq!(cell, Some((5, 3)));
    }

    #[test]
    fn cell_at_pane_clamps_past_the_far_edge() {
        let node = ComputedNode {
            size: Vec2::new(640.0, 384.0),
            ..ComputedNode::DEFAULT
        };
        let transform = UiGlobalTransform::from_xy(320.0, 192.0);
        // A point well past the bottom-right clamps to (cols-1, rows-1).
        let cell = cell_at_pane(
            &node,
            &transform,
            Vec2::new(9999.0, 9999.0),
            8.0,
            16.0,
            80,
            24,
        );
        assert_eq!(cell, Some((79, 23)));
    }

    #[test]
    fn cell_at_pane_clamps_negative_to_origin() {
        let node = ComputedNode {
            size: Vec2::new(640.0, 384.0),
            ..ComputedNode::DEFAULT
        };
        let transform = UiGlobalTransform::from_xy(320.0, 192.0);
        // A point above-left of the node clamps to (0, 0).
        let cell = cell_at_pane(
            &node,
            &transform,
            Vec2::new(-50.0, -50.0),
            8.0,
            16.0,
            80,
            24,
        );
        assert_eq!(cell, Some((0, 0)));
    }

    #[test]
    fn buffer_reply_to_text_joins_lines_with_newline() {
        let lines = vec![
            "first line".to_string(),
            "second line".to_string(),
            "third line".to_string(),
        ];
        assert_eq!(
            buffer_reply_to_text(&lines),
            "first line\nsecond line\nthird line",
        );
    }

    #[test]
    fn buffer_reply_to_text_single_line_no_trailing_newline() {
        let lines = vec!["hello world".to_string()];
        assert_eq!(buffer_reply_to_text(&lines), "hello world");
    }

    #[test]
    fn buffer_reply_to_text_empty_is_empty_string() {
        assert_eq!(buffer_reply_to_text(&[]), "");
    }

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

    #[test]
    fn copy_mode_exit_repaints_live_grid_and_prunes_refresh_state() {
        use bevy::ecs::system::RunSystemOnce;
        use ozma_tty_renderer::prelude::TerminalGridPlugin;
        use tmux_control_parser::{CellDims, PaneId};

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(TerminalGridPlugin);
        app.add_message::<CopyModeReply>();
        app.init_resource::<CopyModeQueries>();
        app.insert_non_send_resource(TmuxConnection::default());
        app.add_plugins(OzmuxTmuxCopyModePlugin);

        let pane_id = PaneId(1);
        let render_child = app.world_mut().spawn(TerminalGrid::default()).id();
        let entity = app
            .world_mut()
            .spawn((
                TmuxPane {
                    id: pane_id,
                    dims: CellDims {
                        width: 20,
                        height: 3,
                        xoff: 0,
                        yoff: 0,
                    },
                },
                TerminalHandle::detached(20, 3, Arc::new(AtomicBool::new(false))),
                TerminalRenderRef(render_child),
                CopyModeState,
            ))
            .id();

        // Seed the live handle with "LIVE" (it was advanced during copy mode but
        // its emit was gated), and paint a *different* capture view ("CAP") into
        // the grid via a scratch handle, modelling the captured scrolled view.
        app.world_mut()
            .run_system_once(
                move |mut commands: Commands, mut handles: Query<&mut TerminalHandle>| {
                    let mut live = handles.get_mut(entity).unwrap();
                    live.advance(b"LIVE");
                    let mut cap = TerminalHandle::detached(20, 3, Arc::new(AtomicBool::new(false)));
                    cap.advance(b"CAP");
                    cap.flush_emit(&mut commands, render_child);
                },
            )
            .unwrap();
        app.update();
        let before: String = app.world().get::<TerminalGrid>(render_child).unwrap().cells[0]
            .iter()
            .map(|c| c.text.as_str())
            .collect();
        assert!(
            before.starts_with("CAP"),
            "grid shows the captured view before exit, got {before:?}",
        );

        // Pre-load the refresh bookkeeping + scratch handle for this pane, as the
        // live loop would.
        {
            let mut refresh = app.world_mut().resource_mut::<CopyRefreshState>();
            refresh.state_in_flight.insert(pane_id, 0);
            refresh.last_scroll.insert(pane_id, 7);
        }
        app.world_mut()
            .entity_mut(entity)
            .insert(CopyRenderHandle(TerminalHandle::detached(
                20,
                3,
                Arc::new(AtomicBool::new(false)),
            )));

        // Exit copy mode: the On<Remove, CopyModeState> observer forces a full
        // live repaint and prunes the refresh state.
        app.world_mut().entity_mut(entity).remove::<CopyModeState>();
        app.update();

        let after: String = app.world().get::<TerminalGrid>(render_child).unwrap().cells[0]
            .iter()
            .map(|c| c.text.as_str())
            .collect();
        assert!(
            after.starts_with("LIVE"),
            "exit forces the grid back to the live handle content, got {after:?}",
        );

        let refresh = app.world().resource::<CopyRefreshState>();
        assert!(
            !refresh.state_in_flight.contains_key(&pane_id),
            "exit prunes the in-flight mark (else a reconnect wedges re-query)",
        );
        assert!(
            !refresh.last_scroll.contains_key(&pane_id),
            "exit prunes last_scroll (else re-entry skips the capture)",
        );
        assert!(
            app.world().get::<CopyRenderHandle>(entity).is_none(),
            "exit drops the scratch CopyRenderHandle",
        );
    }

    #[test]
    fn pane_despawn_prunes_refresh_state() {
        use tmux_control_parser::{CellDims, PaneId};

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_message::<CopyModeReply>();
        app.init_resource::<CopyModeQueries>();
        app.insert_non_send_resource(TmuxConnection::default());
        app.add_plugins(OzmuxTmuxCopyModePlugin);

        let pane_id = PaneId(7);
        let entity = app
            .world_mut()
            .spawn((
                TmuxPane {
                    id: pane_id,
                    dims: CellDims {
                        width: 20,
                        height: 3,
                        xoff: 0,
                        yoff: 0,
                    },
                },
                TerminalHandle::detached(20, 3, Arc::new(AtomicBool::new(false))),
                CopyModeState,
            ))
            .id();

        // Seed the bookkeeping as the live refresh loop would, then DESPAWN the
        // pane (e.g. on TmuxConnectionReset) — not an explicit CopyModeState
        // remove. The On<Remove, CopyModeState> observer must still fire and read
        // TmuxPane to prune, so a reconnect that re-projects %7 cannot wedge.
        {
            let mut refresh = app.world_mut().resource_mut::<CopyRefreshState>();
            refresh.state_in_flight.insert(pane_id, 0);
            refresh.last_scroll.insert(pane_id, 7);
        }
        app.world_mut().entity_mut(entity).despawn();
        app.update();

        let refresh = app.world().resource::<CopyRefreshState>();
        assert!(
            !refresh.state_in_flight.contains_key(&pane_id),
            "despawn must prune the in-flight mark (else reconnect wedges re-query)",
        );
        assert!(
            !refresh.last_scroll.contains_key(&pane_id),
            "despawn must prune last_scroll (else re-entry skips the capture)",
        );
    }

    #[test]
    fn overlay_maps_cursor_and_selection_to_viewport() {
        let state = CopyState {
            pane_in_mode: true,
            scroll_position: 3,
            pane_height: 8,
            history_size: 53,
            cursor_x: 6,
            cursor_y: 7,
            selection_present: true,
            rectangle: false,
            sel_start_x: 2,
            sel_start_y: 54,
            sel_end_x: 6,
            sel_end_y: 57,
        };
        let (vi_cursor, selection) = build_overlay(&state);
        assert_eq!((vi_cursor.column, vi_cursor.row), (6, 7));
        let sel = selection.expect("selection present");
        // sel_start_y=54: absolute_to_visible_row(54, 53, 3) = 54 - (53-3) = 4
        assert_eq!((sel.start.column, sel.start.row), (2, 4));
        // sel_end_y=57: absolute_to_visible_row(57, 53, 3) = 57 - 50 = 7
        assert_eq!((sel.end.column, sel.end.row), (6, 7));
        assert_eq!(sel.kind, SelectionKind::Char);
    }

    #[test]
    fn overlay_omits_selection_when_absent() {
        let state = CopyState {
            pane_in_mode: true,
            scroll_position: 0,
            pane_height: 8,
            history_size: 0,
            cursor_x: 1,
            cursor_y: 1,
            selection_present: false,
            rectangle: false,
            sel_start_x: 0,
            sel_start_y: 0,
            sel_end_x: 0,
            sel_end_y: 0,
        };
        let (_c, selection) = build_overlay(&state);
        assert!(selection.is_none());
    }

    #[test]
    fn overlay_clips_offscreen_selection_rows() {
        let state = CopyState {
            pane_in_mode: true,
            scroll_position: 0,
            pane_height: 8,
            history_size: 100,
            cursor_x: 0,
            cursor_y: 0,
            selection_present: true,
            rectangle: false,
            sel_start_x: 0,
            sel_start_y: 10,
            sel_end_x: 3,
            sel_end_y: 95,
        };
        let (_c, selection) = build_overlay(&state);
        let sel = selection.unwrap();
        // sel_start_y=10: 10 - (100-0) = -90 → clamped to -1
        assert_eq!(sel.start.row, -1);
    }

    #[test]
    #[ignore = "requires a real tmux binary and a controlling PTY"]
    fn copy_mode_scroll_without_selection_repaints_grid() {
        // Regression: scrolling in copy mode WITHOUT an active selection must
        // repaint the grid with older lines. tmux expands the selection_* format
        // vars to EMPTY when no selection exists; if parse_copy_state rejects
        // those, no snapshot/capture forms and the view freezes (the original
        // "scroll movement doesn't work" bug). The integration test below masked
        // this by starting a selection first.
        use crate::clipboard::Clipboard;
        use crate::tmux_render::OzmuxTmuxRenderPlugin;
        use bevy::window::{PrimaryWindow, Window, WindowResolution};
        use ozma_tty_renderer::material::TerminalUiMaterial;
        use ozma_tty_renderer::prelude::TerminalGridPlugin;
        use ozma_tty_renderer::{CellMetrics, TerminalCellMetricsResource};
        use ozmux_tmux::{ConnectionState, TmuxSessionPlugin};
        use std::time::{Duration, Instant};
        use tmux_control::TmuxServer;
        use tmux_control_parser::PaneId;

        let socket = format!("ozmux-scroll-{}", std::process::id());
        let server = TmuxServer::new().socket_name(&socket);
        let client = server.new_session().expect("spawn tmux -CC new-session");

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(TerminalGridPlugin);
        app.init_resource::<Assets<TerminalUiMaterial>>();
        app.add_plugins(TmuxSessionPlugin);
        app.insert_resource(TerminalCellMetricsResource {
            metrics: CellMetrics {
                advance_phys: 8.0,
                line_height_phys: 16.0,
                ascent_phys: 13.0,
                descent_phys: 3.0,
                underline_position_phys: -1.0,
                underline_thickness_phys: 1.0,
                max_overflow_phys: 0.0,
            },
            phys_font_size: 16,
        });
        let mut window = Window {
            resolution: WindowResolution::new(800, 600),
            ..default()
        };
        window.resolution.set_scale_factor(1.0);
        app.world_mut().spawn((window, PrimaryWindow));
        app.insert_resource(Clipboard::new());
        app.add_plugins(OzmuxTmuxRenderPlugin);
        app.add_plugins(OzmuxTmuxCopyModePlugin);
        app.world_mut()
            .get_non_send_resource_mut::<ozmux_tmux::TmuxConnection>()
            .expect("TmuxConnection")
            .set(client);

        // NOTE: yield generously between updates so the transport reader thread
        // is scheduled to deliver replies (a tight busy-loop starves it; the real
        // app yields each frame at the refresh rate). The yield+sleep+yield gives
        // the reader a scheduling window both before and after the sleep.
        let tick = |app: &mut App| {
            app.update();
            std::thread::yield_now();
            std::thread::sleep(Duration::from_millis(25));
            std::thread::yield_now();
        };
        let snap_scroll = |app: &mut App, e: bevy::ecs::entity::Entity| -> Option<u32> {
            app.world()
                .get::<CopyModeSnapshot>(e)
                .map(|s| s.0.scroll_position)
        };

        let attach_deadline = Instant::now() + Duration::from_secs(5);
        let mut pane: Option<(bevy::ecs::entity::Entity, PaneId)> = None;
        while Instant::now() < attach_deadline {
            tick(&mut app);
            if *app.world().resource::<ConnectionState>() == ConnectionState::Attached {
                let mut q = app
                    .world_mut()
                    .query_filtered::<(bevy::ecs::entity::Entity, &ozmux_tmux::TmuxPane), With<TerminalHandle>>();
                if let Some((e, p)) = q.iter(app.world()).next() {
                    pane = Some((e, p.id));
                    break;
                }
            }
        }
        let (pane_entity, pane_id) = pane.expect("a pane must be projected within 5 s");
        let target = format!("%{}", pane_id.0);
        let handle = app
            .world()
            .get_non_send_resource::<ozmux_tmux::TmuxConnection>()
            .unwrap()
            .client()
            .unwrap()
            .handle();

        handle
            .send(&format!(
                "send-keys -t {target} -l -- 'for i in $(seq 1 60); do printf ROW-$i; printf \"\\n\"; done'"
            ))
            .expect("seed");
        handle
            .send(&format!("send-keys -t {target} Enter"))
            .expect("enter");
        let seed = Instant::now() + Duration::from_secs(3);
        while Instant::now() < seed {
            tick(&mut app);
        }

        // Enter copy mode WITHOUT starting any selection.
        handle
            .send(&format!("copy-mode -t {target}"))
            .expect("copy-mode");
        app.world_mut()
            .entity_mut(pane_entity)
            .insert(CopyModeState);

        // A snapshot must form even with NO selection (this is the regression:
        // the parse bug rejected the empty selection_* fields and never produced
        // one, so the refresh never ran).
        let init = Instant::now() + Duration::from_secs(6);
        while Instant::now() < init {
            tick(&mut app);
            if snap_scroll(&mut app, pane_entity).is_some() {
                break;
            }
        }
        let snapshot_ok = snap_scroll(&mut app, pane_entity).is_some();
        let scroll_before = snap_scroll(&mut app, pane_entity).unwrap_or(0);

        // Scroll up; the refresh must observe the new scroll_position (which in
        // turn drives the capture that repaints the scrolled view).
        handle
            .send(&format!("send-keys -X -t {target} -N 15 scroll-up"))
            .expect("scroll-up");
        let scroll = Instant::now() + Duration::from_secs(10);
        let mut scroll_after = scroll_before;
        while Instant::now() < scroll {
            tick(&mut app);
            scroll_after = snap_scroll(&mut app, pane_entity).unwrap_or(scroll_after);
            if scroll_after > scroll_before {
                break;
            }
        }

        // The refresh must REPAINT, not merely track scroll: a changed
        // scroll_position drives a capture whose reply creates the pane's
        // CopyRenderHandle (the scratch grid the scrolled view renders into).
        // Under the regression no snapshot formed, so no capture was ever sent and
        // this handle is never created — asserting snapshot+scroll alone would miss
        // a break anywhere in the capture→repaint path.
        let repaint = Instant::now() + Duration::from_secs(6);
        let mut repaint_ok = false;
        while Instant::now() < repaint {
            tick(&mut app);
            if app.world().get::<CopyRenderHandle>(pane_entity).is_some() {
                repaint_ok = true;
                break;
            }
        }

        if let Some(c) = app
            .world_mut()
            .get_non_send_resource_mut::<ozmux_tmux::TmuxConnection>()
            .unwrap()
            .take()
        {
            c.handle().send("kill-server").ok();
        }

        assert!(
            snapshot_ok,
            "a CopyModeSnapshot must form even with NO selection \
             (parse_copy_state must accept empty selection_* fields)"
        );
        assert!(
            scroll_after > scroll_before,
            "scrolling up without a selection must advance the tracked \
             scroll_position (was {scroll_before}, still {scroll_after})"
        );
        assert!(
            repaint_ok,
            "scrolling without a selection must drive a capture that creates the \
             pane's CopyRenderHandle (the capture→repaint path must run, not just \
             snapshot/scroll bookkeeping)"
        );
    }

    #[test]
    #[ignore = "requires a real tmux binary and a controlling PTY"]
    fn copy_mode_integration_drives_real_tmux() {
        use crate::clipboard::Clipboard;
        use crate::tmux_render::OzmuxTmuxRenderPlugin;
        use bevy::window::{PrimaryWindow, Window, WindowResolution};
        use ozma_tty_renderer::material::TerminalUiMaterial;
        use ozma_tty_renderer::prelude::TerminalGridPlugin;
        use ozma_tty_renderer::{CellMetrics, TerminalCellMetricsResource};
        use ozmux_tmux::{ConnectionState, TmuxSessionPlugin};
        use std::time::{Duration, Instant};
        use tmux_control::TmuxServer;
        use tmux_control_parser::PaneId;

        let socket = format!("ozmux-copymode-{}", std::process::id());
        let server = TmuxServer::new().socket_name(&socket);
        let client = server.new_session().expect("spawn tmux -CC new-session");

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(TerminalGridPlugin);
        app.init_resource::<Assets<TerminalUiMaterial>>();
        app.add_plugins(TmuxSessionPlugin);

        // Cell metrics for layout_tmux_panes (stub 8×16 font so pixel math works).
        app.insert_resource(TerminalCellMetricsResource {
            metrics: CellMetrics {
                advance_phys: 8.0,
                line_height_phys: 16.0,
                ascent_phys: 13.0,
                descent_phys: 3.0,
                underline_position_phys: -1.0,
                underline_thickness_phys: 1.0,
                max_overflow_phys: 0.0,
            },
            phys_font_size: 16,
        });
        let mut window = Window {
            resolution: WindowResolution::new(800, 600),
            ..default()
        };
        window.resolution.set_scale_factor(1.0);
        app.world_mut().spawn((window, PrimaryWindow));
        app.insert_resource(Clipboard::new());

        // OzmuxTmuxRenderPlugin registers attach_tmux_pane_terminal,
        // route_tmux_output, and layout_tmux_panes after TmuxProjectionSet.
        app.add_plugins(OzmuxTmuxRenderPlugin);
        app.add_plugins(OzmuxTmuxCopyModePlugin);

        app.world_mut()
            .get_non_send_resource_mut::<ozmux_tmux::TmuxConnection>()
            .expect("TmuxConnection inserted by TmuxSessionPlugin")
            .set(client);

        // --- Phase 1: wait for attach and a projected pane with a TerminalHandle.
        let attach_deadline = Instant::now() + Duration::from_secs(5);
        let mut pane_entity: Option<(bevy::ecs::entity::Entity, PaneId)> = None;
        while Instant::now() < attach_deadline {
            app.update();
            if *app.world().resource::<ConnectionState>() == ConnectionState::Attached {
                let mut q = app
                    .world_mut()
                    .query_filtered::<(bevy::ecs::entity::Entity, &ozmux_tmux::TmuxPane), With<TerminalHandle>>();
                if let Some((e, p)) = q.iter(app.world()).next() {
                    pane_entity = Some((e, p.id));
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let (pane_entity, pane_id) =
            pane_entity.expect("a pane with a TerminalHandle must be projected within 5 s");
        let target = format!("%{}", pane_id.0);

        let handle = app
            .world()
            .get_non_send_resource::<ozmux_tmux::TmuxConnection>()
            .unwrap()
            .client()
            .unwrap()
            .handle();

        // --- Phase 2: seed the pane with 40 numbered rows so there is scrollback.
        // Use `printf` in a one-shot subshell; the pane's shell picks up after it.
        handle
            .send(&format!(
                "send-keys -t {target} -l -- 'for i in $(seq 1 40); do printf ROW-$i; printf \"\\n\"; done'"
            ))
            .expect("send-keys printf");
        handle
            .send(&format!("send-keys -t {target} Enter"))
            .expect("send-keys Enter");

        // Pump until the pane has some content (best-effort 3 s).
        let seed_deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < seed_deadline {
            app.update();
            std::thread::sleep(Duration::from_millis(80));
        }

        // --- Phase 3: enter copy mode.
        handle
            .send(&format!("copy-mode -t {target}"))
            .expect("copy-mode");
        // Insert CopyModeState so the OzmuxTmuxCopyModePlugin systems engage.
        app.world_mut()
            .entity_mut(pane_entity)
            .insert(CopyModeState);

        // --- Phase 4: scroll up so there is a non-zero scroll_position, then
        // begin a selection, then move the cursor down a couple of rows.
        let step_deadline = Instant::now() + Duration::from_millis(200);
        while Instant::now() < step_deadline {
            app.update();
            std::thread::sleep(Duration::from_millis(50));
        }
        handle
            .send(&format!("send-keys -X -t {target} -N 5 cursor-up"))
            .expect("cursor-up");
        handle
            .send(&format!("send-keys -X -t {target} begin-selection"))
            .expect("begin-selection");
        handle
            .send(&format!("send-keys -X -t {target} -N 2 cursor-down"))
            .expect("cursor-down");

        // Pump for up to 5 s so issue_copy_state and consume_copy_reply can
        // complete at least one full state→capture round-trip.
        // Wait for a snapshot that REFLECTS THE SELECTION. A pre-selection
        // snapshot (selection_present=false) can land first now that
        // parse_copy_state accepts the empty selection_* fields tmux emits before
        // a selection exists; breaking on the first snapshot would race the
        // begin-selection just sent above.
        let copy_deadline = Instant::now() + Duration::from_secs(5);
        let mut got_snapshot = false;
        while Instant::now() < copy_deadline {
            app.update();
            if app
                .world()
                .get::<CopyModeSnapshot>(pane_entity)
                .is_some_and(|s| s.0.selection_present)
            {
                got_snapshot = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(80));
        }

        // Evaluate overlay assertions while still in copy mode.
        let snapshot_ok = got_snapshot;
        let render_child_entity = app
            .world()
            .get::<TerminalRenderRef>(pane_entity)
            .map(|r| r.0);
        let vi_cursor_present = render_child_entity
            .and_then(|e| app.world().get::<ozma_tty_renderer::schema::TerminalGrid>(e))
            .map(|g| g.vi_cursor.is_some())
            .unwrap_or(false);
        let selection_present = app
            .world()
            .get::<CopyModeSnapshot>(pane_entity)
            .map(|s| s.0.selection_present)
            .unwrap_or(false);

        // --- Phase 5: copy and bridge the buffer to the clipboard.
        handle
            .send(&format!("send-keys -X -t {target} copy-selection"))
            .expect("copy-selection");
        let show_id = handle
            .send(&ozmux_tmux::show_buffer_command())
            .expect("show-buffer");
        // Register the show-buffer command so consume_copy_reply routes it as Buffer.
        app.world_mut().resource_mut::<CopyModeQueries>().register(
            show_id,
            pane_id,
            CopyQueryKind::Buffer,
        );

        // Poll until the clipboard holds the SELECTED content (a ROW-N line), not
        // merely until it is non-empty — a stale pre-existing clipboard value (or
        // a wrong-buffer bug) would otherwise short-circuit the loop. The last
        // non-empty read is retained for the failure message either way.
        let clipboard_deadline = Instant::now() + Duration::from_secs(5);
        let mut clipboard_text: Option<String> = None;
        while Instant::now() < clipboard_deadline {
            app.update();
            if let Some(text) = app.world_mut().resource_mut::<Clipboard>().read()
                && !text.is_empty()
            {
                let matched = text.contains("ROW-");
                clipboard_text = Some(text);
                if matched {
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(80));
        }

        // --- Phase 6: exit copy mode; the on_copy_mode_exit observer must fire and
        // restore the live view (repaint_full).
        app.world_mut()
            .entity_mut(pane_entity)
            .remove::<CopyModeState>();
        // Pump so the observer and ApplyDeferred flush.
        let exit_deadline = Instant::now() + Duration::from_millis(500);
        while Instant::now() < exit_deadline {
            app.update();
            std::thread::sleep(Duration::from_millis(50));
        }

        // CopyRenderHandle must have been dropped by the observer.
        let render_handle_gone = app.world().get::<CopyRenderHandle>(pane_entity).is_none();
        // CopyModeSnapshot must also be removed.
        let snapshot_gone = app.world().get::<CopyModeSnapshot>(pane_entity).is_none();
        // Refresh bookkeeping must be pruned.
        let refresh = app.world().resource::<CopyRefreshState>();
        let state_pruned = !refresh.state_in_flight.contains_key(&pane_id);
        let scroll_pruned = !refresh.last_scroll.contains_key(&pane_id);

        // --- Cleanup: kill the tmux server unconditionally.
        if let Some(c) = app
            .world_mut()
            .get_non_send_resource_mut::<ozmux_tmux::TmuxConnection>()
            .unwrap()
            .take()
        {
            c.handle().send("kill-server").ok();
        }

        // --- Assertions (all collected above so cleanup always runs first).
        assert!(
            snapshot_ok,
            "OzmuxTmuxCopyModePlugin must receive at least one CopyModeSnapshot \
             (state→capture round-trip) within 5 s of entering copy mode"
        );
        assert!(
            vi_cursor_present,
            "apply_copy_overlay must set vi_cursor on the pane's TerminalGrid \
             while a CopyModeSnapshot is present"
        );
        assert!(
            selection_present,
            "begin-selection followed by cursor-down must produce \
             selection_present=true in the CopyModeSnapshot"
        );
        assert!(
            clipboard_text
                .as_deref()
                .is_some_and(|t| t.contains("ROW-")),
            "show-buffer routed as CopyQueryKind::Buffer must write the SELECTED \
             content (a ROW-N line) to the Clipboard resource within 5 s, got {clipboard_text:?}"
        );
        assert!(
            render_handle_gone,
            "on_copy_mode_exit must remove the CopyRenderHandle from the pane entity"
        );
        assert!(
            snapshot_gone,
            "on_copy_mode_exit must remove the CopyModeSnapshot from the pane entity"
        );
        assert!(
            state_pruned,
            "on_copy_mode_exit must prune the pane from state_in_flight \
             (otherwise re-entry would wedge the coalescing guard)"
        );
        assert!(
            scroll_pruned,
            "on_copy_mode_exit must prune the pane from last_scroll \
             (otherwise re-entry would skip the first capture)"
        );
    }
}
