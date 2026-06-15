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
use ozma_tty_renderer::schema::{
    SelectionKind, SelectionRange, TerminalGrid, ViCursor, ViewportPoint,
};
use ozmux_tmux::{
    CopyModeQueries, CopyModeReply, CopyQueryKind, CopyState, PaneId, TmuxConnection, TmuxPane,
    TmuxProjectionSet, absolute_to_visible_row, copy_mode_capture_command,
    copy_state_query_command, parse_copy_state,
};
use std::collections::{HashMap, HashSet};
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
                apply_copy_overlay
                    .run_if(any_with_component::<CopyModeSnapshot>)
                    .after(consume_copy_reply),
            )
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

/// The latest copy-mode state snapshot for a pane. Written only when the state
/// changes (so `Changed<CopyModeSnapshot>` is meaningful), and read back to
/// diff against the next reply.
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
    mut live_handles: Query<&mut TerminalHandle>,
    panes: Query<&TmuxPane>,
) {
    let entity = ev.entity;
    if let Ok(mut handle) = live_handles.get_mut(entity) {
        handle.repaint_full(&mut commands, entity);
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
    snapshots: Query<&CopyModeSnapshot>,
) {
    let entity_of: HashMap<PaneId, Entity> = panes.iter().map(|(e, p)| (p.id, e)).collect();
    for reply in replies.read() {
        let Some(&entity) = entity_of.get(&reply.pane) else {
            continue;
        };
        match reply.kind {
            CopyQueryKind::State => {
                refresh.state_in_flight.remove(&reply.pane);
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
                apply_capture_reply(&mut commands, &mut render_handles, entity, reply);
            }
            CopyQueryKind::Buffer => {}
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
fn apply_copy_overlay(mut grids: Query<(&mut TerminalGrid, &CopyModeSnapshot)>) {
    for (mut grid, snapshot) in &mut grids {
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

    #[test]
    fn copy_mode_exit_repaints_live_grid_and_prunes_refresh_state() {
        use bevy::ecs::system::RunSystemOnce;
        use ozma_tty_renderer::prelude::TerminalGridPlugin;
        use ozma_tty_renderer::schema::TerminalGrid;
        use tmux_control_parser::{CellDims, PaneId};

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(TerminalGridPlugin);
        app.add_message::<CopyModeReply>();
        app.init_resource::<CopyModeQueries>();
        app.insert_non_send_resource(TmuxConnection::default());
        app.add_plugins(OzmuxTmuxCopyModePlugin);

        let pane_id = PaneId(1);
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
                TerminalGrid::default(),
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
                    cap.flush_emit(&mut commands, entity);
                },
            )
            .unwrap();
        app.update();
        let before: String = app.world().get::<TerminalGrid>(entity).unwrap().cells[0]
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
            refresh.state_in_flight.insert(pane_id);
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

        let after: String = app.world().get::<TerminalGrid>(entity).unwrap().cells[0]
            .iter()
            .map(|c| c.text.as_str())
            .collect();
        assert!(
            after.starts_with("LIVE"),
            "exit forces the grid back to the live handle content, got {after:?}",
        );

        let refresh = app.world().resource::<CopyRefreshState>();
        assert!(
            !refresh.state_in_flight.contains(&pane_id),
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
            refresh.state_in_flight.insert(pane_id);
            refresh.last_scroll.insert(pane_id, 7);
        }
        app.world_mut().entity_mut(entity).despawn();
        app.update();

        let refresh = app.world().resource::<CopyRefreshState>();
        assert!(
            !refresh.state_in_flight.contains(&pane_id),
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
}
