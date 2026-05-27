//! Bevy plugin that drives mouse-button selection. Reads
//! `MouseButtonInput` and `CursorMoved` events, hit-tests against
//! activity hosts, builds `ButtonEvent`s, dispatches them through
//! `bevy_terminal::ButtonAction::route`, and applies the result.
//!
//! State is owned by the `MouseSelectionState` resource — see spec
//! §6.

use bevy::prelude::*;
use bevy_terminal::{CellCoord, Column, Line, Point, SelectionType, Side};
use std::time::Instant;

/// Per-frame state for the mouse-selection system.
#[derive(Resource, Default)]
pub(crate) struct MouseSelectionState {
    drag: Option<ActiveDrag>,
    last_click: Option<LastClick>,
    /// Next allowed autoscroll tick. `None` outside autoscroll.
    next_autoscroll_at: Option<Instant>,
}

#[derive(Clone)]
struct ActiveDrag {
    entity: Entity,
    anchor_cell: CellCoord,
    /// Last cell where a `Drag` event was synthesized. `None` until the
    /// first inter-cell move; used by `dispatch_mouse_buttons`'s
    /// drag-event synthesizer to deduplicate within-cell motion.
    last_drag_cell: Option<CellCoord>,
    phase: DragPhase,
}

impl ActiveDrag {
    /// Returns `true` once the selection has been materialized
    /// (`selection_start_at` has run). The Armed phase represents a
    /// click-press where the user has not yet moved past the anchor
    /// cell — no `Term::selection` exists yet.
    fn is_active(&self) -> bool {
        matches!(self.phase, DragPhase::Active)
    }
}

#[derive(Clone)]
enum DragPhase {
    /// Press has armed a drag; no inter-cell motion has occurred yet.
    /// `selection_start_at` has NOT been called. The renderer shows
    /// no highlight for this drag.
    Armed {
        ty: SelectionType,
        anchor_side: Side,
    },
    /// Drag has been materialized — `selection_start_at` has run and
    /// the selection lives in `Term::selection`. The original `ty` and
    /// `anchor_side` are now baked into the alacritty selection and
    /// no longer need to be stored on `ActiveDrag`.
    Active,
}

struct LastClick {
    entity: Entity,
    cell: CellCoord,
    cursor_pos_logical_px: Vec2,
    at: Instant,
    count: u8,
}

/// Bevy plugin that registers `MouseSelectionState` and the per-frame
/// `dispatch_mouse_buttons` system in `OzmuxSystems::Input`.
pub(crate) struct MouseButtonsInputPlugin;

impl Plugin for MouseButtonsInputPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MouseSelectionState>().add_systems(
            Update,
            dispatch_mouse_buttons
                .in_set(crate::system_set::OzmuxSystems::Input)
                .before(crate::input::dispatch_focused_key),
        );
    }
}

/// Hit-tests `cursor_phys_px` against all `ActivityHostNode` entities
/// and returns `(entity, local_phys_px)` for the first pane that
/// contains the cursor. `local_phys_px` is in pane-local pixels with
/// origin at the top-left corner of the node (i.e., `(0, 0)` is the
/// top-left, `(size.x, size.y)` is the bottom-right).
///
/// `cursor_phys_px` is in physical (DPR-scaled) pixels — the caller
/// must convert from `Window::cursor_position()` (logical) by
/// multiplying by `Window::scale_factor()` first.
pub(crate) fn resolve_pane_at_phys(
    hosts: &Query<
        (
            Entity,
            &bevy::ui::ComputedNode,
            &bevy::ui::UiGlobalTransform,
        ),
        With<crate::ui::ActivityHostNode>,
    >,
    cursor_phys_px: Vec2,
) -> Option<(Entity, Vec2)> {
    for (entity, node, transform) in hosts.iter() {
        if !node.contains_point(*transform, cursor_phys_px) {
            continue;
        }
        // NOTE: normalize_point returns None if the affine transform is
        // degenerate (zero-size node or non-invertible). contains_point
        // returning true normally implies Some here, but skip defensively
        // to avoid an unwrap on the degenerate case.
        let Some(normalized) = node.normalize_point(*transform, cursor_phys_px) else {
            continue;
        };
        let local = (normalized + Vec2::splat(0.5)) * node.size;
        return Some((entity, local));
    }
    None
}

/// Projects a pane-local physical-pixel point onto 1-indexed
/// `(col, row, side)`. Clamps `col` to `1..=cols` and `row` to
/// `1..=rows`. `cell_w_phys` / `cell_h_phys` are the physical-pixel
/// cell pitch from `TerminalCellMetricsResource`.
pub(crate) fn cell_at_local(
    local_phys: Vec2,
    cell_w_phys: f32,
    cell_h_phys: f32,
    cols: u16,
    rows: u16,
) -> (u32, u32, Side) {
    let col_f = (local_phys.x / cell_w_phys).max(0.0);
    let row_f = (local_phys.y / cell_h_phys).max(0.0);
    let col = (col_f.floor() as u32 + 1).min(cols as u32).max(1);
    let row = (row_f.floor() as u32 + 1).min(rows as u32).max(1);
    let frac_x = col_f - col_f.floor();
    let side = if frac_x < 0.5 {
        Side::Left
    } else {
        Side::Right
    };
    (col, row, side)
}

/// Computes the click count (1, 2, or 3) for a new left-press and
/// updates `state.last_click`. Per spec §6 step 3:
///   - 1 if `last_click.entity != entity`
///   - 1 if `last_click.cell != cell`
///   - 1 if `now - last_click.at >= double_click_timeout`
///   - 1 if cursor drift exceeds `click_drift_px`
///   - else `(last_click.count % 3) + 1` (triple wraps to 1)
pub(crate) fn next_click_count(
    state: &mut MouseSelectionState,
    cfg: &ozmux_configs::mouse::MouseConfig,
    entity: Entity,
    cell: CellCoord,
    cursor_logical: Vec2,
    now: Instant,
) -> u8 {
    let timeout = std::time::Duration::from_millis(cfg.double_click_timeout_ms as u64);
    let drift_sq = cfg.click_drift_px * cfg.click_drift_px;
    let count = match state.last_click.as_ref() {
        Some(prev)
            if prev.entity == entity
                && prev.cell == cell
                && now.duration_since(prev.at) < timeout
                && (cursor_logical - prev.cursor_pos_logical_px).length_squared() <= drift_sq =>
        {
            (prev.count % 3) + 1
        }
        _ => 1,
    };
    state.last_click = Some(LastClick {
        entity,
        cell,
        cursor_pos_logical_px: cursor_logical,
        at: now,
        count,
    });
    count
}

/// Pre-route helper. If `target_entity` belongs to a pane that is not
/// the currently active pane in `attached_sid`'s session, mutates
/// `Session::active_pane` to that pane and bumps the change-detection
/// signals (`bump_epoch` + `set_changed`). No-op when:
///   - The entity isn't registered as the active activity of any pane
///     in `attached_sid`.
///   - The pane is already active.
///   - The session isn't found.
///
/// Returns `true` when a focus change actually happened.
///
/// Cross-session clicks (where `target_entity` belongs to a pane in a
/// different session) are NOT handled here — they're rejected per the
/// spec §10 edge cases. The reverse-lookup only scans the attached
/// session's panes; an unknown target falls through as a no-op.
pub(crate) fn try_click_to_focus(
    mux: &mut ResMut<crate::multiplexer::Multiplexer>,
    registry: &crate::ui::registry::ActivityEntityRegistry,
    attached_sid: ozmux_multiplexer::SessionId,
    target_entity: Entity,
) -> bool {
    let mux_ref = mux.bypass_change_detection();
    let target_pane: Option<ozmux_multiplexer::PaneId> = {
        let Some(session) = mux_ref.sessions.get(&attached_sid) else {
            return false;
        };
        let mut found: Option<ozmux_multiplexer::PaneId> = None;
        for pane_id in session.pane_ids() {
            if let Ok(pane) = session.pane(pane_id) {
                for activity in &pane.activities {
                    if registry.get(&activity.id) == Some(target_entity) {
                        found = Some(pane_id.clone());
                        break;
                    }
                }
                if found.is_some() {
                    break;
                }
            }
        }
        found
    };
    let Some(target_pane) = target_pane else {
        return false;
    };

    let mutated =
        crate::multiplexer::commands::focus_pane_by_id(mux_ref, &attached_sid, &target_pane);
    if mutated {
        mux.bump_epoch(&attached_sid);
        mux.set_changed();
    }
    mutated
}

/// Drag-scroll tick period in ms, given distance past the pane edge in
/// cells. Linear-step decay from `autoscroll_base_period_ms` floored at
/// `autoscroll_min_period_ms`.
pub(crate) fn autoscroll_period_ms(
    cfg: &ozmux_configs::mouse::MouseConfig,
    distance_cells: u32,
) -> u32 {
    cfg.autoscroll_base_period_ms
        .saturating_sub(distance_cells * cfg.autoscroll_step_ms)
        .max(cfg.autoscroll_min_period_ms)
}

/// True when an in-flight drag should be dropped because alacritty
/// wiped `Term::selection` out from under us (alt-screen swap, screen
/// reset). See spec §6 "End-of-frame guards" and
/// `term/mod.rs:682, 733, 1803, 1847`.
pub(crate) fn should_drop_stale_drag(handle: &bevy_terminal::TerminalHandle) -> bool {
    handle.selection_type().is_none()
}

/// Runs a single autoscroll tick if conditions are met. Called once per
/// frame from the end-of-frame guard section. Updates `next_autoscroll_at`
/// and performs the scroll+selection-update.
fn run_autoscroll_tick(
    state: &mut MouseSelectionState,
    drag: &ActiveDrag,
    cursor_phys: Vec2,
    now: Instant,
    node: bevy::ui::ComputedNode,
    transform: bevy::ui::UiGlobalTransform,
    cell_h_phys: f32,
    cell_w_phys: f32,
    configs: &ozmux_configs::mouse::MouseConfig,
    handles: &mut Query<(
        &mut bevy_terminal::TerminalHandle,
        &mut bevy_terminal::PtyHandle,
        &mut bevy_terminal::Coalescer,
    )>,
    copy_mode_q: &Query<(), With<crate::ui::copy_mode::CopyModeState>>,
) {
    // NOTE: UiGlobalTransform.translation is the node CENTER, not the
    // top-left corner — every hit-test in this file relies on the
    // `translation ± half * size` form. The inner Affine2 field is
    // private; we access `translation` via the type's Deref impl.
    let translation = transform.translation;
    let half = node.size * 0.5;
    let pane_top = translation.y - half.y;
    let pane_bot = translation.y + half.y;

    let above = cursor_phys.y < pane_top;
    let below = cursor_phys.y > pane_bot;
    if !above && !below {
        state.next_autoscroll_at = None;
        return;
    }

    let distance_cells = if above {
        ((pane_top - cursor_phys.y) / cell_h_phys).floor().max(0.0) as u32
    } else {
        ((cursor_phys.y - pane_bot) / cell_h_phys).floor().max(0.0) as u32
    };
    let period_ms = autoscroll_period_ms(configs, distance_cells);
    let period = std::time::Duration::from_millis(period_ms as u64);

    let next_at = state.next_autoscroll_at.unwrap_or(now + period);
    if now < next_at {
        state.next_autoscroll_at = Some(next_at);
        return;
    }

    // Time to tick. Compute the edge cell (clamped to pane bounds).
    let edge_local_y = if above { 0.0 } else { node.size.y };
    let edge_local_x = (cursor_phys.x - (translation.x - half.x)).clamp(0.0, node.size.x);
    let edge_local = bevy::math::Vec2::new(edge_local_x, edge_local_y);

    let Ok((mut handle, _pty, mut coalescer)) = handles.get_mut(drag.entity) else {
        return;
    };
    let (cols, rows, _) = handle.read_geometry();
    let (col, row, side) = cell_at_local(edge_local, cell_w_phys, cell_h_phys, cols, rows);
    // 1-indexed cell → 0-indexed viewport point.
    let pt = bevy_terminal::Point::new(
        bevy_terminal::Line((row as i32) - 1),
        bevy_terminal::Column((col as usize) - 1),
    );

    let in_copy_mode = copy_mode_q.get(drag.entity).is_ok();
    let scroll_delta: i32 = if above { 1 } else { -1 };

    if in_copy_mode {
        // NOTE: vi_goto must run BEFORE scroll_display in copy mode.
        // scroll_display calls vi_mode_recompute_selection, which sets
        // selection.end = vi_cursor.point. Without the pre-scroll
        // vi_goto, the selection end snaps back to the stale vi cursor
        // before we overwrite it via selection_update_to.
        handle.vi_goto(&mut coalescer, pt);
        handle.scroll(&mut coalescer, scroll_delta);
        handle.selection_update_to(&mut coalescer, pt, side);
    } else {
        handle.scroll(&mut coalescer, scroll_delta);
        handle.selection_update_to(&mut coalescer, pt, side);
    }

    state.next_autoscroll_at = Some(now + period);
}

/// Per-frame system entrypoint. Drains `MouseButtonInput`, hit-tests
/// against activity hosts, tracks click count, dispatches every
/// press/release through `ButtonAction::route`, and pre-routes
/// click-to-focus per spec §6 step 4. Drag-state tracking + autoscroll
/// (Tasks 19-20) are layered on later.
fn dispatch_mouse_buttons(
    mut state: ResMut<MouseSelectionState>,
    mut mux: ResMut<crate::multiplexer::Multiplexer>,
    mut buttons_msg: MessageReader<bevy::input::mouse::MouseButtonInput>,
    mut cursor_msg: MessageReader<bevy::window::CursorMoved>,
    keys: Res<ButtonInput<KeyCode>>,
    configs: Res<crate::configs::OzmuxConfigsResource>,
    hosts_q: Query<
        (
            Entity,
            &bevy::ui::ComputedNode,
            &bevy::ui::UiGlobalTransform,
        ),
        With<crate::ui::ActivityHostNode>,
    >,
    mut handles: Query<(
        &mut bevy_terminal::TerminalHandle,
        &mut bevy_terminal::PtyHandle,
        &mut bevy_terminal::Coalescer,
    )>,
    copy_mode_q: Query<(), With<crate::ui::copy_mode::CopyModeState>>,
    windows_q: Query<&Window, With<bevy::window::PrimaryWindow>>,
    metrics: Res<bevy_terminal_renderer::TerminalCellMetricsResource>,
    time: Res<Time<Real>>,
    attached_sid_q: Query<
        &crate::multiplexer::SessionEntityId,
        With<crate::multiplexer::AttachedSession>,
    >,
    registry: Res<crate::ui::registry::ActivityEntityRegistry>,
) {
    let Ok(window) = windows_q.single() else {
        buttons_msg.clear();
        cursor_msg.clear();
        return;
    };
    let scale = window.scale_factor();
    let Some(cursor_logical) = window.cursor_position() else {
        buttons_msg.clear();
        cursor_msg.clear();
        return;
    };
    let cursor_phys = cursor_logical * scale;
    let cell_w_phys = metrics.metrics.advance_phys.floor().max(1.0);
    let cell_h_phys = metrics.metrics.line_height_phys.floor().max(1.0);

    let mods = crate::input::current_modifiers(&keys);
    let proto_mods = bevy_terminal::ProtocolModifiers {
        shift: mods.shift,
        ctrl: mods.ctrl,
        alt: mods.alt,
        meta: mods.meta,
    };
    let cfg = bevy_terminal::ButtonConfig {
        max_protocol_events_per_frame: configs.mouse.max_protocol_events_per_frame,
    };

    // Drain CursorMoved events — Drag events are NOT synthesized in
    // this task; Task 19's drag-scroll loop reads cursor position
    // separately, and the dispatch_mouse_buttons system only fires on
    // explicit button transitions.
    cursor_msg.clear();

    for ev in buttons_msg.read() {
        let bevy_button = match ev.button {
            bevy::input::mouse::MouseButton::Left => bevy_terminal::MouseButtonKind::Left,
            bevy::input::mouse::MouseButton::Middle => bevy_terminal::MouseButtonKind::Middle,
            bevy::input::mouse::MouseButton::Right => bevy_terminal::MouseButtonKind::Right,
            _ => continue,
        };
        let Some((entity, local)) = resolve_pane_at_phys(&hosts_q, cursor_phys) else {
            continue;
        };
        let (cols, rows) = match handles.get(entity) {
            Ok((h, _, _)) => {
                let (c, r, _) = h.read_geometry();
                (c, r)
            }
            Err(_) => continue,
        };
        let (col, row, side) = cell_at_local(local, cell_w_phys, cell_h_phys, cols, rows);
        let cell = bevy_terminal::CellCoord { col, row };

        let kind = match ev.state {
            bevy::input::ButtonState::Pressed => bevy_terminal::ButtonEventKind::Press,
            bevy::input::ButtonState::Released => bevy_terminal::ButtonEventKind::Release,
        };

        let click_count = if matches!(kind, bevy_terminal::ButtonEventKind::Press)
            && matches!(bevy_button, bevy_terminal::MouseButtonKind::Left)
        {
            next_click_count(
                &mut state,
                &configs.mouse,
                entity,
                cell,
                cursor_logical,
                time.last_update().unwrap_or_else(Instant::now),
            )
        } else {
            1
        };

        if matches!(kind, bevy_terminal::ButtonEventKind::Press) {
            let Ok(attached_sid) = attached_sid_q.single() else {
                continue;
            };
            try_click_to_focus(&mut mux, &registry, attached_sid.0, entity);
        }

        let evt = bevy_terminal::ButtonEvent {
            kind,
            button: bevy_button,
            cell,
            side,
            click_count,
        };
        let modes = match handles.get(entity) {
            Ok((h, _, _)) => h.current_modes(),
            Err(_) => continue,
        };
        let action = bevy_terminal::ButtonAction::route(modes, evt, proto_mods, &cfg);

        // Drag-state lifecycle: set on local left-press, clear on
        // left-release. Drag is only meaningful when the press routed
        // locally (the action carries the SelectionType).
        if matches!(bevy_button, bevy_terminal::MouseButtonKind::Left) {
            match (&action, kind) {
                (
                    bevy_terminal::ButtonAction::StartLocalSelection { .. },
                    bevy_terminal::ButtonEventKind::Press,
                ) => {
                    state.drag = Some(ActiveDrag {
                        entity,
                        anchor_cell: cell,
                        last_drag_cell: None,
                        phase: DragPhase::Active,
                    });
                }
                (_, bevy_terminal::ButtonEventKind::Release) => {
                    state.drag = None;
                    state.next_autoscroll_at = None;
                }
                _ => {}
            }
        }

        apply_action(action, entity, &mut handles, &copy_mode_q);
    }

    // Drag-scroll loop. Runs every frame while a left-drag is active
    // and the cursor is past the pane's vertical rect.
    let now = time.last_update().unwrap_or_else(Instant::now);
    let Some(drag) = state.drag.clone() else {
        state.next_autoscroll_at = None;
        return;
    };

    // Capture pane geometry by value (ComputedNode is Copy + Clone).
    let Ok((_, node_ref, transform_ref)) = hosts_q.get(drag.entity) else {
        // Entity is gone or not found; fall through to end-of-frame guards.
        return;
    };
    let node = *node_ref;
    let transform = *transform_ref;

    // Run autoscroll tick if time permits.
    run_autoscroll_tick(
        &mut state,
        &drag,
        cursor_phys,
        now,
        node,
        transform,
        cell_h_phys,
        cell_w_phys,
        &configs.mouse,
        &mut handles,
        &copy_mode_q,
    );

    // End-of-frame guards.

    // 1. Drop stale drag when alacritty wiped Term::selection (e.g.
    //    alt-screen swap, screen reset). Without this, the next drag
    //    tick would re-arm a phantom anchor.
    if let Some(drag) = state.drag.as_ref() {
        match handles.get(drag.entity) {
            Ok((handle, _, _)) if should_drop_stale_drag(handle) => {
                state.drag = None;
                state.next_autoscroll_at = None;
            }
            Err(_) => {
                // Entity is gone (e.g. pane closed mid-drag).
                state.drag = None;
                state.next_autoscroll_at = None;
            }
            _ => {}
        }
    }

    // 2. Resize clamp: clamp anchor_cell to current geometry so a
    //    mid-drag pane resize doesn't leave us pointing past the new
    //    bottom-right.
    if let Some(drag) = state.drag.as_mut()
        && let Ok((handle, _, _)) = handles.get(drag.entity)
    {
        let (cols, rows, _) = handle.read_geometry();
        drag.anchor_cell.col = drag.anchor_cell.col.min(cols as u32).max(1);
        drag.anchor_cell.row = drag.anchor_cell.row.min(rows as u32).max(1);
    }
}

/// Dispatches the router's `ButtonAction` against the focused entity's
/// `TerminalHandle`. In copy mode, `vi_goto` is issued before any
/// selection mutation so the vi cursor tracks the moving end of the
/// selection (see `bevy_terminal::TerminalHandle::vi_goto` docs).
fn apply_action(
    action: bevy_terminal::ButtonAction,
    entity: Entity,
    handles: &mut Query<(
        &mut bevy_terminal::TerminalHandle,
        &mut bevy_terminal::PtyHandle,
        &mut bevy_terminal::Coalescer,
    )>,
    copy_mode_q: &Query<(), With<crate::ui::copy_mode::CopyModeState>>,
) {
    use bevy_terminal::ButtonAction as A;
    let Ok((mut handle, mut pty, mut coalescer)) = handles.get_mut(entity) else {
        return;
    };
    let in_copy_mode = copy_mode_q.get(entity).is_ok();
    let to_viewport_point =
        |c: CellCoord| Point::new(Line((c.row as i32) - 1), Column((c.col as usize) - 1));

    match action {
        A::Noop => {}
        A::WriteToPty(bytes) => {
            if let Err(e) = handle.write(&mut pty, &bytes) {
                tracing::warn!(?e, ?entity, "mouse-button PTY write failed");
            }
        }
        A::ClearAndWriteToPty(bytes) => {
            handle.selection_clear(&mut coalescer);
            if let Err(e) = handle.write(&mut pty, &bytes) {
                tracing::warn!(?e, ?entity, "mouse-button forwarded press PTY write failed");
            }
        }
        A::StartLocalSelection { ty, cell, side } => {
            let pt = to_viewport_point(cell);
            if in_copy_mode {
                handle.vi_goto(&mut coalescer, pt);
            }
            handle.selection_start_at(&mut coalescer, pt, side, ty);
        }
        A::UpdateLocalSelection { cell, side } => {
            let pt = to_viewport_point(cell);
            if in_copy_mode {
                handle.vi_goto(&mut coalescer, pt);
            }
            handle.selection_update_to(&mut coalescer, pt, side);
        }
        A::ClearLocalSelection => {
            handle.selection_clear(&mut coalescer);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_registers_state_resource() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(MouseButtonsInputPlugin);
        assert!(app.world().contains_resource::<MouseSelectionState>());
    }

    #[test]
    fn cell_at_local_projects_to_one_indexed_coords_and_side() {
        // 10x10 physical px cell. local (15, 25) → col 2 (15/10=1.5→floor 1, +1=2), row 3 (25/10=2.5→floor 2, +1=3).
        // frac_x = 0.5 → Side::Right.
        let (col, row, side) = super::cell_at_local(Vec2::new(15.0, 25.0), 10.0, 10.0, 80, 24);
        assert_eq!(col, 2);
        assert_eq!(row, 3);
        assert_eq!(side, Side::Right);
    }

    #[test]
    fn cell_at_local_left_half_returns_side_left() {
        // local (2, 5): col 1, row 1; frac_x = 0.2 < 0.5 → Side::Left.
        let (_col, _row, side) = super::cell_at_local(Vec2::new(2.0, 5.0), 10.0, 10.0, 80, 24);
        assert_eq!(side, Side::Left);
    }

    #[test]
    fn cell_at_local_clamps_to_grid() {
        // Local position past grid bounds clamps to (cols, rows).
        let (col, row, _side) =
            super::cell_at_local(Vec2::new(10_000.0, 10_000.0), 10.0, 10.0, 80, 24);
        assert_eq!(col, 80);
        assert_eq!(row, 24);
    }

    #[test]
    fn click_count_resets_on_first_press() {
        let mut state = MouseSelectionState::default();
        let cfg = mock_cfg();
        let now = Instant::now();
        let count = super::next_click_count(
            &mut state,
            &cfg,
            Entity::from_bits(1),
            CellCoord { col: 5, row: 5 },
            Vec2::new(10.0, 10.0),
            now,
        );
        assert_eq!(count, 1);
    }

    #[test]
    fn click_count_increments_within_timeout_same_cell() {
        let mut state = MouseSelectionState::default();
        let cfg = mock_cfg();
        let now = Instant::now();
        let e = Entity::from_bits(1);
        let cell = CellCoord { col: 5, row: 5 };
        let pos = Vec2::new(10.0, 10.0);
        let _ = super::next_click_count(&mut state, &cfg, e, cell, pos, now);
        let c2 = super::next_click_count(&mut state, &cfg, e, cell, pos, now);
        let c3 = super::next_click_count(&mut state, &cfg, e, cell, pos, now);
        let c4 = super::next_click_count(&mut state, &cfg, e, cell, pos, now);
        assert_eq!(c2, 2);
        assert_eq!(c3, 3);
        assert_eq!(c4, 1, "triple-click wraps back to 1");
    }

    #[test]
    fn click_count_resets_on_different_entity() {
        let mut state = MouseSelectionState::default();
        let cfg = mock_cfg();
        let now = Instant::now();
        let cell = CellCoord { col: 5, row: 5 };
        let pos = Vec2::new(10.0, 10.0);
        let _ = super::next_click_count(&mut state, &cfg, Entity::from_bits(1), cell, pos, now);
        let c2 = super::next_click_count(&mut state, &cfg, Entity::from_bits(2), cell, pos, now);
        assert_eq!(c2, 1, "different entity must reset the counter");
    }

    #[test]
    fn click_count_resets_on_drift_beyond_threshold() {
        let mut state = MouseSelectionState::default();
        let cfg = mock_cfg();
        let now = Instant::now();
        let e = Entity::from_bits(1);
        let cell = CellCoord { col: 5, row: 5 };
        let _ = super::next_click_count(&mut state, &cfg, e, cell, Vec2::new(10.0, 10.0), now);
        // Move 20 px away — exceeds the 8.0 default drift threshold.
        let c2 = super::next_click_count(&mut state, &cfg, e, cell, Vec2::new(40.0, 10.0), now);
        assert_eq!(c2, 1);
    }

    #[test]
    fn click_count_resets_after_timeout() {
        use std::time::Duration;
        let mut state = MouseSelectionState::default();
        let cfg = mock_cfg();
        let now = Instant::now();
        let later = now + Duration::from_millis(500); // > 400ms default
        let e = Entity::from_bits(1);
        let cell = CellCoord { col: 5, row: 5 };
        let pos = Vec2::new(10.0, 10.0);
        let _ = super::next_click_count(&mut state, &cfg, e, cell, pos, now);
        let c2 = super::next_click_count(&mut state, &cfg, e, cell, pos, later);
        assert_eq!(c2, 1);
    }

    fn mock_cfg() -> ozmux_configs::mouse::MouseConfig {
        ozmux_configs::mouse::MouseConfig::default()
    }

    #[test]
    fn autoscroll_period_decreases_with_distance_past_edge() {
        let cfg = mock_cfg();
        // distance = 0 cells past edge → period = base (50ms).
        assert_eq!(super::autoscroll_period_ms(&cfg, 0), 50);
        // distance = 4 → 50 - 4*4 = 34ms (above min=16).
        assert_eq!(super::autoscroll_period_ms(&cfg, 4), 34);
        // distance = 100 → saturating_sub clamped to 0, then max → 16.
        assert_eq!(super::autoscroll_period_ms(&cfg, 100), 16);
    }

    #[test]
    fn try_click_to_focus_mutates_active_pane_and_returns_true() {
        use bevy::ecs::system::RunSystemOnce;
        use ozmux_configs::shortcuts::{Action, SplitDirection};

        let mut app = App::new();
        app.insert_resource(crate::multiplexer::Multiplexer::default());
        app.insert_resource(crate::ui::registry::ActivityEntityRegistry::default());

        let (sid, original_pane, original_activity, new_activity) = {
            let mux = &mut app
                .world_mut()
                .resource_mut::<crate::multiplexer::Multiplexer>();
            let (sid, original_pane, original_activity) = mux.create_session(Some("test".into()));
            let split_mutated = crate::multiplexer::commands::apply(
                Action::SplitPane {
                    direction: SplitDirection::Horizontal,
                },
                &mut mux.0,
                sid,
            );
            assert!(split_mutated, "split must succeed");
            let session = mux.sessions.get(&sid).unwrap();
            let new_pane = session.active_pane.clone();
            assert_ne!(
                new_pane, original_pane,
                "split must promote a fresh pane to active"
            );
            let new_activity = session.pane(&new_pane).unwrap().active_activity.clone();
            (sid, original_pane, original_activity, new_activity)
        };

        let original_entity = app.world_mut().spawn_empty().id();
        let new_entity = app.world_mut().spawn_empty().id();
        {
            let mut registry = app
                .world_mut()
                .resource_mut::<crate::ui::registry::ActivityEntityRegistry>();
            registry.insert_for_test(original_activity, original_entity);
            registry.insert_for_test(new_activity, new_entity);
        }

        let mutated = app
            .world_mut()
            .run_system_once(
                move |mut mux: ResMut<crate::multiplexer::Multiplexer>,
                      registry: Res<crate::ui::registry::ActivityEntityRegistry>|
                      -> bool {
                    try_click_to_focus(&mut mux, &registry, sid, original_entity)
                },
            )
            .unwrap();
        assert!(
            mutated,
            "click-to-focus must mutate when targeting a non-active pane"
        );
        assert_eq!(
            app.world()
                .resource::<crate::multiplexer::Multiplexer>()
                .sessions
                .get(&sid)
                .unwrap()
                .active_pane,
            original_pane,
            "active pane must now be the click target"
        );

        let mutated_again = app
            .world_mut()
            .run_system_once(
                move |mut mux: ResMut<crate::multiplexer::Multiplexer>,
                      registry: Res<crate::ui::registry::ActivityEntityRegistry>|
                      -> bool {
                    try_click_to_focus(&mut mux, &registry, sid, original_entity)
                },
            )
            .unwrap();
        assert!(
            !mutated_again,
            "second click on already-active pane returns false"
        );

        let stranger = app.world_mut().spawn_empty().id();
        let mutated_unknown = app
            .world_mut()
            .run_system_once(
                move |mut mux: ResMut<crate::multiplexer::Multiplexer>,
                      registry: Res<crate::ui::registry::ActivityEntityRegistry>|
                      -> bool {
                    try_click_to_focus(&mut mux, &registry, sid, stranger)
                },
            )
            .unwrap();
        assert!(!mutated_unknown, "unknown entity must not mutate focus");
    }

    #[test]
    fn dispatch_translates_left_press_to_simple_selection() {
        use bevy_terminal::{
            ButtonAction, ButtonConfig, ButtonEvent, ButtonEventKind, MouseButtonKind,
            ProtocolModifiers, Side, TermMode,
        };

        let evt = ButtonEvent {
            kind: ButtonEventKind::Press,
            button: MouseButtonKind::Left,
            cell: bevy_terminal::CellCoord { col: 5, row: 5 },
            side: Side::Left,
            click_count: 1,
        };
        let action = ButtonAction::route(
            TermMode::empty(),
            evt,
            ProtocolModifiers::default(),
            &ButtonConfig {
                max_protocol_events_per_frame: 8,
            },
        );
        assert!(matches!(action, ButtonAction::StartLocalSelection { .. }));
    }

    #[test]
    fn should_drop_stale_drag_returns_true_when_no_selection() {
        let opts = bevy_terminal::SpawnOptions {
            cols: 80,
            rows: 24,
            shell: std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into()),
            cwd: None,
            env: Vec::new(),
        };
        let bundle = bevy_terminal::TerminalBundle::spawn(opts).unwrap();
        // Fresh bundle has no selection.
        assert!(super::should_drop_stale_drag(&bundle.handle));
    }
}
