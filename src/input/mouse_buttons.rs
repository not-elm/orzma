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

#[allow(dead_code)] // fields populated in subsequent tasks
struct ActiveDrag {
    entity: Entity,
    ty: SelectionType,
    anchor_cell: CellCoord,
    in_copy_mode: bool,
}

#[allow(dead_code)] // fields populated in subsequent tasks
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
#[allow(dead_code)] // wired into dispatch_mouse_buttons in subsequent tasks
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
#[allow(dead_code)] // wired into dispatch_mouse_buttons in subsequent tasks
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

/// Per-frame system entrypoint. Drains `MouseButtonInput`, hit-tests
/// against activity hosts, tracks click count, and dispatches every
/// press/release through `ButtonAction::route`. Click-to-focus
/// (Task 18) and drag-state tracking + autoscroll (Tasks 19-20) are
/// layered on later.
fn dispatch_mouse_buttons(
    mut state: ResMut<MouseSelectionState>,
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
    let cell_w_phys = metrics.metrics.advance_phys.max(1.0);
    let cell_h_phys = metrics.metrics.line_height_phys.max(1.0);

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

        // Click-to-focus (Task 18) and drag-state tracking (Task 19)
        // go here.

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
        apply_action(action, entity, &mut handles, &copy_mode_q);
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
}
