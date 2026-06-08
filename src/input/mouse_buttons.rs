//! Bevy plugin that drives mouse-button selection. Reads
//! `MouseButtonInput` and `CursorMoved` events, hit-tests against
//! surface hosts, builds `ButtonEvent`s, dispatches them through
//! `bevy_terminal::ButtonAction::route`, and applies the result.
//!
//! State is owned by the `MouseSelectionState` resource — see spec
//! §6.

#[cfg(not(feature = "thin-client"))]
use crate::configs::OzmuxConfigsResource;
use crate::input::InputPhase;
use crate::input::current_modifiers;
use crate::input::hyperlink::{HyperlinkClick, hyperlink_click, link_modifier_held, try_open_uri};
use crate::ui::Slotted;
use crate::ui::copy_mode::CopyModeState;
#[cfg(not(feature = "thin-client"))]
use bevy::input::ButtonState;
#[cfg(not(feature = "thin-client"))]
use bevy::input::mouse::{MouseButton, MouseButtonInput};
use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};
#[cfg(not(feature = "thin-client"))]
use bevy::window::{CursorMoved, PrimaryWindow};
use bevy_terminal::{ButtonAction, CellCoord, SelectionType, Side};
#[cfg(not(feature = "thin-client"))]
use bevy_terminal::{Column, Line, Point};
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
        app.init_resource::<MouseSelectionState>();
        app.add_systems(Update, dispatch_mouse_buttons.in_set(InputPhase::Dispatch));
    }
}

/// Hit-tests `cursor_phys_px` against the **slotted** (`Slotted`) Surface
/// entities and returns `(entity, local_phys_px)` for the first pane that
/// contains the cursor. `local_phys_px` is in pane-local pixels with
/// origin at the top-left corner of the node (i.e., `(0, 0)` is the
/// top-left, `(size.x, size.y)` is the bottom-right).
///
/// Only the active (slotted) surface is hit-tested. Parked surfaces sit
/// outside layout and keep stale, often window-sized `ComputedNode` geometry;
/// including them lets a click resolve to a parked surface of an
/// already-active pane, so focus never moves (see `Slotted`).
///
/// `cursor_phys_px` is in physical (DPR-scaled) pixels — the caller
/// must convert from `Window::cursor_position()` (logical) by
/// multiplying by `Window::scale_factor()` first.
pub(crate) fn resolve_pane_at_phys(
    hosts: &Query<
        (Entity, &ComputedNode, &UiGlobalTransform),
        (With<ozmux_multiplexer::SurfaceMarker>, With<Slotted>),
    >,
    cursor_phys_px: Vec2,
) -> Option<(Entity, Vec2)> {
    for (entity, node, transform) in hosts.iter() {
        if !node.contains_point(*transform, cursor_phys_px) {
            continue;
        }
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

/// Pre-route helper. The hit entity *is* the Surface, so its owning Pane is
/// one `SurfaceOf` read (`pane_of_surface`). If that pane is not the
/// currently active pane in the attached workspace, updates
/// `Workspace::ActivePane`. No-op when:
///   - The surface has no owning pane.
///   - The pane belongs to a different (non-attached) workspace.
///   - The pane is already active.
///   - The workspace isn't found.
///
/// Returns `true` when a focus change actually happened.
///
/// Cross-workspace clicks (where the surface belongs to a pane in a
/// different workspace) are rejected per the spec §10 edge cases.
#[cfg(not(feature = "thin-client"))]
pub(crate) fn try_click_to_focus(
    mux: &mut ozmux_multiplexer::MultiplexerCommands,
    attached_workspace: Entity,
    target_surface: Entity,
) -> bool {
    let Some(target_pane) = mux.pane_of_surface(target_surface) else {
        return false;
    };

    if mux.workspace_of_pane(target_pane) != Some(attached_workspace) {
        return false;
    }

    if mux.workspaces_active_pane(attached_workspace) == Some(target_pane) {
        return false;
    }

    if let Err(err) = mux.set_active_pane(attached_workspace, target_pane) {
        tracing::warn!(target: "ozmux_gui::input", ?err, "try_click_to_focus: set_active_pane failed");
        return false;
    }
    true
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
/// reset). See spec §7 and `term/mod.rs:682, 733, 1803, 1847`.
///
/// Only fires for `DragPhase::Active` drags — an `Armed` drag legitimately
/// has no `Term::selection` because the selection has not been
/// materialized yet, and the absence is not a wipe signal.
#[cfg(not(feature = "thin-client"))]
fn should_drop_stale_drag(drag: &ActiveDrag, handle: &bevy_terminal::TerminalHandle) -> bool {
    drag.is_active() && handle.selection_type().is_none()
}

/// Runs a single autoscroll tick if conditions are met. Called once per
/// frame from the end-of-frame guard section. Updates `next_autoscroll_at`
/// and performs the scroll+selection-update.
#[cfg(not(feature = "thin-client"))]
fn run_autoscroll_tick(
    state: &mut MouseSelectionState,
    drag: &ActiveDrag,
    cursor_phys: Vec2,
    now: Instant,
    node: ComputedNode,
    transform: UiGlobalTransform,
    cell_h_phys: f32,
    cell_w_phys: f32,
    configs: &ozmux_configs::mouse::MouseConfig,
    handles: &mut Query<(
        &mut bevy_terminal::TerminalHandle,
        &mut bevy_terminal::PtyHandle,
    )>,
    copy_modes: &Query<(), With<CopyModeState>>,
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
    let edge_local = Vec2::new(edge_local_x, edge_local_y);

    let Ok((mut handle, _pty)) = handles.get_mut(drag.entity) else {
        return;
    };
    let (cols, rows, _) = handle.read_geometry();
    let (col, row, side) = cell_at_local(edge_local, cell_w_phys, cell_h_phys, cols, rows);
    let pt = to_viewport_point(CellCoord { col, row });

    let in_copy_mode = copy_modes.get(drag.entity).is_ok();
    let scroll_delta: i32 = if above { 1 } else { -1 };

    if in_copy_mode {
        // NOTE: vi_goto must run BEFORE scroll_display in copy mode.
        // scroll_display calls vi_mode_recompute_selection, which sets
        // selection.end = vi_cursor.point. Without the pre-scroll
        // vi_goto, the selection end snaps back to the stale vi cursor
        // before we overwrite it via selection_update_to.
        handle.vi_goto(pt);
        handle.scroll(scroll_delta);
        handle.selection_update_to(pt, side);
    } else {
        handle.scroll(scroll_delta);
        handle.selection_update_to(pt, side);
    }

    state.next_autoscroll_at = Some(now + period);
}

/// Decides whether to synthesize a `Drag` event this frame. Returns
/// `Some((cell, side))` when the cursor has moved to a different cell
/// since the last synthesized Drag event for the active drag; `None`
/// otherwise. Also mutates `state.drag.last_drag_cell` to the new cell
/// on `Some`. Returns `None` if no drag is in flight or the drag pane's
/// geometry is unresolvable.
///
/// The drag stays anchored to the original pane — out-of-pane cursor
/// positions clamp to the pane edge so a cursor that wanders into
/// another pane still extends the original selection.
fn synthesize_drag_cell(
    state: &mut MouseSelectionState,
    cursor_phys: Vec2,
    cell_w_phys: f32,
    cell_h_phys: f32,
    pane_geometry: Option<(ComputedNode, UiGlobalTransform, u16, u16)>,
) -> Option<(CellCoord, Side)> {
    let drag = state.drag.as_ref()?;
    let (node, transform, cols, rows) = pane_geometry?;
    // NOTE: UiGlobalTransform.translation is the node CENTER, not the
    // top-left corner — every hit-test in this file relies on the
    // `translation ± half * size` form.
    let pane_top_left = transform.translation - node.size * 0.5;
    let local = (cursor_phys - pane_top_left).clamp(Vec2::ZERO, node.size);
    let (col, row, side) = cell_at_local(local, cell_w_phys, cell_h_phys, cols, rows);
    let cell = CellCoord { col, row };
    let last = drag.last_drag_cell.unwrap_or(drag.anchor_cell);
    if last == cell {
        return None;
    }
    // Record the new last_drag_cell BEFORE the caller routes the event,
    // so re-entrancy can't loop.
    if let Some(drag) = state.drag.as_mut() {
        drag.last_drag_cell = Some(cell);
    }
    Some((cell, side))
}

/// Per-frame system entrypoint. Drains `MouseButtonInput`, hit-tests
/// against surface hosts, tracks click count, dispatches every
/// press/release through `ButtonAction::route`, and pre-routes
/// click-to-focus per spec §6 step 4. Drag-state tracking + autoscroll
/// (Tasks 19-20) are layered on later.
#[cfg(not(feature = "thin-client"))]
fn dispatch_mouse_buttons(
    mut state: ResMut<MouseSelectionState>,
    mut mux: ozmux_multiplexer::MultiplexerCommands,
    mut buttons_msg: MessageReader<MouseButtonInput>,
    mut cursor_msg: MessageReader<CursorMoved>,
    mut handles: Query<(
        &mut bevy_terminal::TerminalHandle,
        &mut bevy_terminal::PtyHandle,
    )>,
    keys: Res<ButtonInput<KeyCode>>,
    configs: Res<OzmuxConfigsResource>,
    hosts: Query<
        (Entity, &ComputedNode, &UiGlobalTransform),
        (With<ozmux_multiplexer::SurfaceMarker>, With<Slotted>),
    >,
    grids: Query<&bevy_terminal_renderer::schema::TerminalGrid>,
    copy_modes: Query<(), With<CopyModeState>>,
    primary_window: Query<&Window, With<PrimaryWindow>>,
    metrics: Res<bevy_terminal_renderer::TerminalCellMetricsResource>,
    time: Res<Time<Real>>,
    attached_workspace: Query<
        Entity,
        (
            With<ozmux_multiplexer::WorkspaceMarker>,
            With<ozmux_multiplexer::AttachedWorkspace>,
        ),
    >,
) {
    let Ok(window) = primary_window.single() else {
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

    let mods = current_modifiers(&keys);
    let proto_mods = bevy_terminal::ProtocolModifiers {
        shift: mods.shift,
        ctrl: mods.ctrl,
        alt: mods.alt,
        meta: mods.meta,
    };
    let cfg = bevy_terminal::ButtonConfig {
        max_protocol_events_per_frame: configs.mouse.max_protocol_events_per_frame,
    };

    // We don't need per-event CursorMoved details — the drag-event
    // synthesizer below reads `cursor_phys` directly. Drain so the
    // reader's queue doesn't grow unbounded.
    cursor_msg.clear();

    for ev in buttons_msg.read() {
        let bevy_button = match ev.button {
            MouseButton::Left => bevy_terminal::MouseButtonKind::Left,
            MouseButton::Middle => bevy_terminal::MouseButtonKind::Middle,
            MouseButton::Right => bevy_terminal::MouseButtonKind::Right,
            _ => continue,
        };
        let Some((entity, local)) = resolve_pane_at_phys(&hosts, cursor_phys) else {
            continue;
        };

        // Click-to-focus runs for EVERY pane kind (terminal, extension, browser)
        // and MUST happen before the terminal-handle lookup below: that lookup
        // `continue`s past panes with no `TerminalHandle` (extension / browser
        // webviews), so focusing only afterwards would never switch to a webview
        // pane — keystrokes would keep routing to the previously-active terminal.
        if matches!(ev.state, ButtonState::Pressed)
            && let Ok(attached_workspace) = attached_workspace.single()
        {
            try_click_to_focus(&mut mux, attached_workspace, entity);
        }

        let (cols, rows) = match handles.get(entity) {
            Ok((h, _)) => {
                let (c, r, _) = h.read_geometry();
                (c, r)
            }
            Err(_) => continue,
        };
        let (col, row, side) = cell_at_local(local, cell_w_phys, cell_h_phys, cols, rows);
        let cell = bevy_terminal::CellCoord { col, row };

        let kind = match ev.state {
            ButtonState::Pressed => bevy_terminal::ButtonEventKind::Press,
            ButtonState::Released => bevy_terminal::ButtonEventKind::Release,
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

        // NOTE: OSC 8 hyperlink interception — Cmd+Left (or Ctrl+Left) on a
        //       linked cell. Press opens the URI and skips PTY routing; a
        //       modifier-held release also skips so the PTY never sees a
        //       release without a matching press. The try_click_to_focus call
        //       above is preserved so the pane still focuses.
        let modifier_held = link_modifier_held(&mods);
        if modifier_held && let Ok(grid) = grids.get(entity) {
            match hyperlink_click(
                grid,
                row.saturating_sub(1) as u16,
                col.saturating_sub(1) as u16,
                bevy_button,
                kind,
                modifier_held,
            ) {
                HyperlinkClick::Open(uri) => {
                    try_open_uri(uri.as_str());
                    continue;
                }
                HyperlinkClick::Suppress => continue,
                HyperlinkClick::Pass => {}
            }
        }

        let evt = bevy_terminal::ButtonEvent {
            kind,
            button: bevy_button,
            cell,
            side,
            click_count,
        };
        let modes = match handles.get(entity) {
            Ok((h, _)) => h.current_modes(),
            Err(_) => continue,
        };
        let action = bevy_terminal::ButtonAction::route(modes, evt, proto_mods, &cfg);

        apply_action(
            &mut state,
            kind,
            bevy_button,
            action,
            entity,
            &mut handles,
            &copy_modes,
        );
    }

    // Drag-event synthesis (spec §4). While `state.drag.is_some()`, turn
    // cursor motion into `ButtonEventKind::Drag` events anchored to the
    // drag pane. De-duplicated by cell (spec §4.3): only one Drag event
    // per cell crossing per frame.
    if let Some(drag_entity) = state.drag.as_ref().map(|d| d.entity) {
        let pane_geometry = hosts.get(drag_entity).ok().and_then(|(_, node, xf)| {
            handles.get(drag_entity).ok().map(|(h, _)| {
                let (cols, rows, _) = h.read_geometry();
                (*node, *xf, cols, rows)
            })
        });
        if let Some((cell, side)) = synthesize_drag_cell(
            &mut state,
            cursor_phys,
            cell_w_phys,
            cell_h_phys,
            pane_geometry,
        ) {
            let modes = match handles.get(drag_entity) {
                Ok((h, _)) => h.current_modes(),
                Err(_) => return,
            };
            let evt = bevy_terminal::ButtonEvent {
                kind: bevy_terminal::ButtonEventKind::Drag,
                button: bevy_terminal::MouseButtonKind::Left,
                cell,
                side,
                click_count: 1,
            };
            let action = bevy_terminal::ButtonAction::route(modes, evt, proto_mods, &cfg);
            apply_action(
                &mut state,
                evt.kind,
                evt.button,
                action,
                drag_entity,
                &mut handles,
                &copy_modes,
            );
        }
    }

    // Drag-scroll loop. Runs only for Active drags (Armed drags have no
    // selection to extend yet) and only while the cursor is past the
    // pane's vertical rect.
    let now = time.last_update().unwrap_or_else(Instant::now);
    if let Some(drag) = state.drag.as_ref().filter(|d| d.is_active()).cloned() {
        if let Ok((_, node_ref, transform_ref)) = hosts.get(drag.entity) {
            let node = *node_ref;
            let transform = *transform_ref;
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
                &copy_modes,
            );
        }
    } else {
        state.next_autoscroll_at = None;
    }

    // End-of-frame guards (run for both Armed and Active phases).

    // 1. Drop stale drag when alacritty wiped Term::selection (e.g.
    //    alt-screen swap, screen reset). Without this, the next drag
    //    tick would re-arm a phantom anchor. Gated by drag.is_active()
    //    inside should_drop_stale_drag so an armed-but-unstarted drag
    //    is not falsely flagged stale.
    if let Some(drag) = state.drag.as_ref() {
        match handles.get(drag.entity) {
            Ok((handle, _)) if should_drop_stale_drag(drag, handle) => {
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
        && let Ok((handle, _)) = handles.get(drag.entity)
    {
        let (cols, rows, _) = handle.read_geometry();
        drag.anchor_cell.col = drag.anchor_cell.col.min(cols as u32).max(1);
        drag.anchor_cell.row = drag.anchor_cell.row.min(rows as u32).max(1);
    }
}

#[cfg(feature = "thin-client")]
fn dispatch_mouse_buttons(
    mut conn: bevy::ecs::system::NonSendMut<crate::thin_client::ThinClientConn>,
    mut state: ResMut<MouseSelectionState>,
    mut buttons_msg: MessageReader<bevy::input::mouse::MouseButtonInput>,
    query: ozmux_multiplexer::MultiplexerQuery,
    keys: Res<ButtonInput<KeyCode>>,
    configs: Res<crate::configs::OzmuxConfigsResource>,
    hosts: Query<
        (Entity, &ComputedNode, &UiGlobalTransform),
        (With<ozmux_multiplexer::SurfaceMarker>, With<Slotted>),
    >,
    grids: Query<&bevy_terminal_renderer::prelude::TerminalGrid>,
    surface_ids: Query<&ozmux_multiplexer::MuxSurfaceId>,
    pane_ids: Query<&ozmux_multiplexer::MuxPaneId>,
    workspace_ids: Query<&ozmux_multiplexer::MuxWorkspaceId>,
    copy_modes: Query<(), With<CopyModeState>>,
    primary_window: Query<&Window, With<bevy::window::PrimaryWindow>>,
    metrics: Res<bevy_terminal_renderer::TerminalCellMetricsResource>,
    time: Res<Time<Real>>,
    attached_workspace: Query<
        Entity,
        (
            With<ozmux_multiplexer::WorkspaceMarker>,
            With<ozmux_multiplexer::AttachedWorkspace>,
        ),
    >,
) {
    let Ok(window) = primary_window.single() else {
        buttons_msg.clear();
        return;
    };
    let scale = window.scale_factor();
    let Some(cursor_logical) = window.cursor_position() else {
        buttons_msg.clear();
        return;
    };
    let cursor_phys = cursor_logical * scale;
    let cell_w_phys = metrics.metrics.advance_phys.floor().max(1.0);
    let cell_h_phys = metrics.metrics.line_height_phys.floor().max(1.0);

    let mods = current_modifiers(&keys);
    let proto_mods = bevy_terminal::ProtocolModifiers {
        shift: mods.shift,
        ctrl: mods.ctrl,
        alt: mods.alt,
        meta: mods.meta,
    };
    let cfg = bevy_terminal::ButtonConfig {
        max_protocol_events_per_frame: configs.mouse.max_protocol_events_per_frame,
    };

    for ev in buttons_msg.read() {
        let bevy_button = match ev.button {
            bevy::input::mouse::MouseButton::Left => bevy_terminal::MouseButtonKind::Left,
            bevy::input::mouse::MouseButton::Middle => bevy_terminal::MouseButtonKind::Middle,
            bevy::input::mouse::MouseButton::Right => bevy_terminal::MouseButtonKind::Right,
            _ => continue,
        };
        let Some((entity, local)) = resolve_pane_at_phys(&hosts, cursor_phys) else {
            continue;
        };

        // Click-to-focus: read-only mirror of try_click_to_focus's reject
        // logic (cross-workspace + already-active). Send SetActivePane when
        // the press lands on a non-active pane in the attached workspace.
        if matches!(ev.state, bevy::input::ButtonState::Pressed)
            && let Ok(attached) = attached_workspace.single()
            && let Some(target_pane) = query.pane_of_surface(entity)
            && query.workspace_of_pane(target_pane) == Some(attached)
            && query.workspaces_active_pane(attached) != Some(target_pane)
            && let Ok(workspace) = workspace_ids.get(attached).map(|c| c.0)
            && let Ok(pane) = pane_ids.get(target_pane).map(|c| c.0)
        {
            crate::thin_client::send_cmd(
                &mut conn,
                ozmux_proto::ClientMessage::SetActivePane { workspace, pane },
            );
        }

        // Selection / mouse-protocol routing requires both a grid (cell dims
        // + modes) and a wire surface id; webview panes have neither.
        let Ok(grid) = grids.get(entity) else {
            continue;
        };
        let Ok(surface) = surface_ids.get(entity).map(|c| c.0) else {
            continue;
        };
        let (col, row, side) = cell_at_local(local, cell_w_phys, cell_h_phys, grid.cols, grid.rows);
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

        // NOTE: OSC 8 hyperlink interception — mirrors the local arm. Cmd+Left
        //       (or Ctrl+Left) on a linked cell: Press opens the URI and skips
        //       wire routing; a modifier-held release skips so the daemon never
        //       sees a release without a matching press. The SetActivePane
        //       click-to-focus above still stands.
        let modifier_held = link_modifier_held(&mods);
        if modifier_held {
            match hyperlink_click(
                grid,
                row.saturating_sub(1) as u16,
                col.saturating_sub(1) as u16,
                bevy_button,
                kind,
                modifier_held,
            ) {
                HyperlinkClick::Open(uri) => {
                    try_open_uri(uri.as_str());
                    continue;
                }
                HyperlinkClick::Suppress => continue,
                HyperlinkClick::Pass => {}
            }
        }

        let evt = bevy_terminal::ButtonEvent {
            kind,
            button: bevy_button,
            cell,
            side,
            click_count,
        };
        let modes = bevy_terminal::modes_from_names(&grid.modes);
        let action = bevy_terminal::ButtonAction::route(modes, evt, proto_mods, &cfg);
        let in_copy_mode = copy_modes.get(entity).is_ok();
        let selection_active = grid.selection.is_some();
        apply_action(
            &mut conn,
            &mut state,
            kind,
            bevy_button,
            action,
            entity,
            surface,
            selection_active,
            in_copy_mode,
        );
    }

    // Drag-event synthesis: while a drag is in flight, turn cursor motion into
    // `Drag` events anchored to the drag pane, de-duplicated by cell.
    if let Some(drag_entity) = state.drag.as_ref().map(|d| d.entity) {
        let pane_geometry = hosts.get(drag_entity).ok().and_then(|(_, node, xf)| {
            grids
                .get(drag_entity)
                .ok()
                .map(|g| (*node, *xf, g.cols, g.rows))
        });
        if let Some((cell, side)) = synthesize_drag_cell(
            &mut state,
            cursor_phys,
            cell_w_phys,
            cell_h_phys,
            pane_geometry,
        ) && let Ok(grid) = grids.get(drag_entity)
            && let Ok(surface) = surface_ids.get(drag_entity).map(|c| c.0)
        {
            let modes = bevy_terminal::modes_from_names(&grid.modes);
            let evt = bevy_terminal::ButtonEvent {
                kind: bevy_terminal::ButtonEventKind::Drag,
                button: bevy_terminal::MouseButtonKind::Left,
                cell,
                side,
                click_count: 1,
            };
            let action = bevy_terminal::ButtonAction::route(modes, evt, proto_mods, &cfg);
            let in_copy_mode = copy_modes.get(drag_entity).is_ok();
            let selection_active = grid.selection.is_some();
            apply_action(
                &mut conn,
                &mut state,
                evt.kind,
                evt.button,
                action,
                drag_entity,
                surface,
                selection_active,
                in_copy_mode,
            );
        }
    }

    // Drag-scroll loop. Runs only for Active drags while the cursor is past the
    // pane's vertical rect.
    let now = time.last_update().unwrap_or_else(Instant::now);
    if let Some(drag) = state.drag.as_ref().filter(|d| d.is_active()).cloned() {
        if let Ok((_, node_ref, transform_ref)) = hosts.get(drag.entity)
            && let Ok(grid) = grids.get(drag.entity)
            && let Ok(surface) = surface_ids.get(drag.entity).map(|c| c.0)
        {
            let node = *node_ref;
            let transform = *transform_ref;
            let in_copy_mode = copy_modes.get(drag.entity).is_ok();
            run_autoscroll_tick(
                &mut conn,
                &mut state,
                cursor_phys,
                now,
                node,
                transform,
                cell_h_phys,
                cell_w_phys,
                &configs.mouse,
                surface,
                grid.cols,
                grid.rows,
                in_copy_mode,
            );
        }
    } else {
        state.next_autoscroll_at = None;
    }

    // End-of-frame guards (run for both Armed and Active phases).

    // 1. Drop the drag when its entity is gone (pane closed mid-drag). This is
    //    unambiguous and frame-latency-independent, unlike the selection-wipe
    //    check below.
    //
    // TODO: port the local `should_drop_stale_drag` selection-wipe check
    //    (Active drag + selection gone => alt-screen swap / screen reset).
    //    Over the wire, `grid.selection` lags the optimistic local `Active`
    //    transition by >=1 frame (the daemon's SelectionStartAt round-trip
    //    arrives via the next pump, which runs before Input), so reading
    //    `grid.selection.is_none()` here would falsely drop a freshly-started
    //    drag every time. A faithful port needs a "selection acknowledged"
    //    signal (e.g. only arm the wipe-check once a frame with our selection
    //    has been observed) — deferred to keep T5 scoped to the core flow.
    if let Some(drag) = state.drag.as_ref()
        && grids.get(drag.entity).is_err()
    {
        state.drag = None;
        state.next_autoscroll_at = None;
    }

    // 2. Resize clamp: clamp anchor_cell to current geometry so a mid-drag
    //    pane resize doesn't leave us pointing past the new bottom-right.
    if let Some(drag) = state.drag.as_mut()
        && let Ok(grid) = grids.get(drag.entity)
    {
        drag.anchor_cell.col = drag.anchor_cell.col.min(grid.cols as u32).max(1);
        drag.anchor_cell.row = drag.anchor_cell.row.min(grid.rows as u32).max(1);
    }
}

/// Converts a 1-indexed `CellCoord` (the wire format from
/// `ButtonEvent.cell` / `cell_at_local`) to a 0-indexed alacritty
/// `Point` suitable for `selection_start_at` / `selection_update_to` /
/// `vi_goto`. Both callers (apply_action's local-selection branches
/// and run_autoscroll_tick) must use the same conversion.
#[cfg(not(feature = "thin-client"))]
fn to_viewport_point(cell: CellCoord) -> Point {
    Point::new(Line((cell.row as i32) - 1), Column((cell.col as usize) - 1))
}

/// Dispatches the router's `ButtonAction` against the focused entity's
/// `TerminalHandle`. In copy mode, `vi_goto` is issued before any
/// selection mutation so the vi cursor tracks the moving end of the
/// selection (see `bevy_terminal::TerminalHandle::vi_goto` docs).
#[cfg(not(feature = "thin-client"))]
fn apply_action(
    state: &mut MouseSelectionState,
    event_kind: bevy_terminal::ButtonEventKind,
    event_button: bevy_terminal::MouseButtonKind,
    action: bevy_terminal::ButtonAction,
    entity: Entity,
    handles: &mut Query<(
        &mut bevy_terminal::TerminalHandle,
        &mut bevy_terminal::PtyHandle,
    )>,
    copy_modes: &Query<(), With<CopyModeState>>,
) {
    let Ok((mut handle, mut pty)) = handles.get_mut(entity) else {
        return;
    };

    // Left-release ALWAYS clears the drag state, regardless of which
    // action the router emitted — covers the Shift-release-mid-drag
    // corner where the router flips from local-route to PTY-forward
    // between press and release.
    if matches!(
        (event_kind, event_button),
        (
            bevy_terminal::ButtonEventKind::Release,
            bevy_terminal::MouseButtonKind::Left,
        ),
    ) {
        state.drag = None;
        state.next_autoscroll_at = None;
    }

    let in_copy_mode = copy_modes.get(entity).is_ok();

    match action {
        ButtonAction::Noop => {}
        ButtonAction::WriteToPty(bytes) => {
            if let Err(e) = handle.write(&mut pty, &bytes) {
                tracing::warn!(?e, ?entity, "mouse-button PTY write failed");
            }
        }
        ButtonAction::ClearAndWriteToPty(bytes) => {
            handle.selection_clear();
            if let Err(e) = handle.write(&mut pty, &bytes) {
                tracing::warn!(?e, ?entity, "mouse-button forwarded press PTY write failed");
            }
        }
        ButtonAction::ArmDrag { ty, cell, side } => {
            // Clear any prior selection on the focused pane so the
            // click does not leave a stale highlight visible (spec §2,
            // brainstorm Q5).
            handle.selection_clear();
            state.drag = Some(ActiveDrag {
                entity,
                anchor_cell: cell,
                last_drag_cell: None,
                phase: DragPhase::Armed {
                    ty,
                    anchor_side: side,
                },
            });
        }
        ButtonAction::StartLocalSelection { ty, cell, side } => {
            let pt = to_viewport_point(cell);
            if in_copy_mode {
                handle.vi_goto(pt);
            }
            handle.selection_start_at(pt, side, ty);
            // Immediate-selection types (Semantic, Lines, Block) are born
            // already-started — the press itself materialized the
            // selection.
            state.drag = Some(ActiveDrag {
                entity,
                anchor_cell: cell,
                last_drag_cell: None,
                phase: DragPhase::Active,
            });
        }
        ButtonAction::UpdateLocalSelection { cell, side } => {
            let pt = to_viewport_point(cell);
            if let Some(drag) = state.drag.as_mut().filter(|d| d.entity == entity) {
                // Materialize the selection now if the drag is still
                // Armed (first inter-cell move). Anchor at the press
                // cell, then extend to the current cell.
                if let DragPhase::Armed { ty, anchor_side } = drag.phase {
                    if drag.anchor_cell == cell {
                        // Still in the press cell — wait for the next
                        // Drag event. Drag-event synthesis dedupes
                        // same-cell motion, so this branch is
                        // defensive — should not normally be hit.
                        return;
                    }
                    let anchor_pt = to_viewport_point(drag.anchor_cell);
                    if in_copy_mode {
                        handle.vi_goto(anchor_pt);
                    }
                    handle.selection_start_at(anchor_pt, anchor_side, ty);
                    drag.phase = DragPhase::Active;
                }
                if in_copy_mode {
                    handle.vi_goto(pt);
                }
                handle.selection_update_to(pt, side);
            }
            // No active or armed drag → silently ignore. Drag events
            // should only be synthesized when state.drag.is_some();
            // reaching this arm without a drag would be a logic bug
            // elsewhere.
        }
        ButtonAction::ClearLocalSelection => {
            handle.selection_clear();
        }
    }
}

/// Converts a 1-indexed `CellCoord` to a 0-indexed proto `ViewportPoint`
/// (the wire analog of `to_viewport_point`). Both apply_action's selection
/// arms and the thin autoscroll tick must use the same conversion.
#[cfg(feature = "thin-client")]
fn cell_to_viewport_point(cell: CellCoord) -> ozmux_proto::ViewportPoint {
    ozmux_proto::ViewportPoint {
        line: (cell.row as i32) - 1,
        col: (cell.col as usize) - 1,
    }
}

/// Converts a `bevy_terminal::Side` to its proto mirror `ozmux_proto::CellSide`.
#[cfg(feature = "thin-client")]
fn cell_side_to_proto(side: Side) -> ozmux_proto::CellSide {
    match side {
        Side::Left => ozmux_proto::CellSide::Left,
        Side::Right => ozmux_proto::CellSide::Right,
    }
}

/// Thin-client sink for the router's `ButtonAction`. Mirrors the local
/// `apply_action`'s gesture-state mutation but sends each selection mutation
/// over the wire as a `CopyModeOp` (the daemon drives selection + frame
/// rendering). In copy mode, `ViGoto` is sent before any selection mutation so
/// the daemon's vi cursor tracks the moving end of the selection.
///
/// `pub(crate)` so the real-daemon thin integration test in
/// `crate::thin_client` can drive the `ButtonAction` → `CopyModeOp` mapping
/// end-to-end (the wire-level mouse test exercises raw ops, not this mapping).
#[cfg(feature = "thin-client")]
pub(crate) fn apply_action(
    conn: &mut crate::thin_client::ThinClientConn,
    state: &mut MouseSelectionState,
    event_kind: bevy_terminal::ButtonEventKind,
    event_button: bevy_terminal::MouseButtonKind,
    action: ButtonAction,
    entity: Entity,
    surface: ozmux_proto::SurfaceId,
    selection_active: bool,
    in_copy_mode: bool,
) {
    // Left-release ALWAYS clears the drag state, regardless of which action the
    // router emitted — covers the Shift-release-mid-drag corner where the
    // router flips from local-route to PTY-forward between press and release.
    if matches!(
        (event_kind, event_button),
        (
            bevy_terminal::ButtonEventKind::Release,
            bevy_terminal::MouseButtonKind::Left,
        ),
    ) {
        state.drag = None;
        state.next_autoscroll_at = None;
    }

    match action {
        ButtonAction::Noop => {}
        ButtonAction::WriteToPty(bytes) => {
            crate::thin_client::send_cmd(
                conn,
                ozmux_proto::ClientMessage::Input { surface, bytes },
            );
        }
        ButtonAction::ClearAndWriteToPty(bytes) => {
            if selection_active {
                crate::thin_client::send_copy_op(
                    conn,
                    surface,
                    ozmux_proto::CopyModeOp::SelectionClear,
                );
            }
            crate::thin_client::send_cmd(
                conn,
                ozmux_proto::ClientMessage::Input { surface, bytes },
            );
        }
        ButtonAction::ArmDrag { ty, cell, side } => {
            crate::thin_client::send_copy_op(
                conn,
                surface,
                ozmux_proto::CopyModeOp::SelectionClear,
            );
            state.drag = Some(ActiveDrag {
                entity,
                anchor_cell: cell,
                last_drag_cell: None,
                phase: DragPhase::Armed {
                    ty,
                    anchor_side: side,
                },
            });
        }
        ButtonAction::StartLocalSelection { ty, cell, side } => {
            let point = cell_to_viewport_point(cell);
            if in_copy_mode {
                crate::thin_client::send_copy_op(
                    conn,
                    surface,
                    ozmux_proto::CopyModeOp::ViGoto { point },
                );
            }
            crate::thin_client::send_copy_op(
                conn,
                surface,
                ozmux_proto::CopyModeOp::SelectionStartAt {
                    point,
                    side: cell_side_to_proto(side),
                    ty: crate::thin_client::selection_type_to_kind(ty),
                },
            );
            state.drag = Some(ActiveDrag {
                entity,
                anchor_cell: cell,
                last_drag_cell: None,
                phase: DragPhase::Active,
            });
        }
        ButtonAction::UpdateLocalSelection { cell, side } => {
            let Some(drag) = state.drag.as_mut().filter(|d| d.entity == entity) else {
                return;
            };
            // Materialize the selection now if the drag is still Armed (first
            // inter-cell move). Anchor at the press cell, then extend.
            if let DragPhase::Armed { ty, anchor_side } = drag.phase {
                if drag.anchor_cell == cell {
                    return;
                }
                let anchor_point = cell_to_viewport_point(drag.anchor_cell);
                if in_copy_mode {
                    crate::thin_client::send_copy_op(
                        conn,
                        surface,
                        ozmux_proto::CopyModeOp::ViGoto {
                            point: anchor_point,
                        },
                    );
                }
                crate::thin_client::send_copy_op(
                    conn,
                    surface,
                    ozmux_proto::CopyModeOp::SelectionStartAt {
                        point: anchor_point,
                        side: cell_side_to_proto(anchor_side),
                        ty: crate::thin_client::selection_type_to_kind(ty),
                    },
                );
                drag.phase = DragPhase::Active;
            }
            let point = cell_to_viewport_point(cell);
            if in_copy_mode {
                crate::thin_client::send_copy_op(
                    conn,
                    surface,
                    ozmux_proto::CopyModeOp::ViGoto { point },
                );
            }
            crate::thin_client::send_copy_op(
                conn,
                surface,
                ozmux_proto::CopyModeOp::SelectionUpdateTo {
                    point,
                    side: cell_side_to_proto(side),
                },
            );
        }
        ButtonAction::ClearLocalSelection => {
            crate::thin_client::send_copy_op(
                conn,
                surface,
                ozmux_proto::CopyModeOp::SelectionClear,
            );
        }
    }
}

/// Thin-client autoscroll tick. Mirrors `run_autoscroll_tick`'s timer /
/// geometry / edge-cell logic (all pure, GUI-side, reads grid dims), but sends
/// the scroll + selection extension over the wire: `Scroll` then
/// `SelectionUpdateTo` (and `ViGoto` first in copy mode).
#[cfg(feature = "thin-client")]
fn run_autoscroll_tick(
    conn: &mut crate::thin_client::ThinClientConn,
    state: &mut MouseSelectionState,
    cursor_phys: Vec2,
    now: Instant,
    node: ComputedNode,
    transform: UiGlobalTransform,
    cell_h_phys: f32,
    cell_w_phys: f32,
    configs: &ozmux_configs::mouse::MouseConfig,
    surface: ozmux_proto::SurfaceId,
    cols: u16,
    rows: u16,
    in_copy_mode: bool,
) {
    // NOTE: UiGlobalTransform.translation is the node CENTER, not the
    // top-left corner — every hit-test in this file relies on the
    // `translation ± half * size` form.
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

    let edge_local_y = if above { 0.0 } else { node.size.y };
    let edge_local_x = (cursor_phys.x - (translation.x - half.x)).clamp(0.0, node.size.x);
    let edge_local = Vec2::new(edge_local_x, edge_local_y);
    let (col, row, side) = cell_at_local(edge_local, cell_w_phys, cell_h_phys, cols, rows);
    let point = cell_to_viewport_point(CellCoord { col, row });

    let scroll_delta: i32 = if above { 1 } else { -1 };

    if in_copy_mode {
        // NOTE: vi_goto must be sent BEFORE Scroll in copy mode. The daemon's
        // scroll recomputes the selection end from the vi cursor; without the
        // pre-scroll ViGoto, the end snaps back to the stale vi cursor before
        // SelectionUpdateTo overwrites it.
        crate::thin_client::send_copy_op(conn, surface, ozmux_proto::CopyModeOp::ViGoto { point });
    }
    crate::thin_client::send_cmd(
        conn,
        ozmux_proto::ClientMessage::Scroll {
            surface,
            delta: scroll_delta,
        },
    );
    crate::thin_client::send_copy_op(
        conn,
        surface,
        ozmux_proto::CopyModeOp::SelectionUpdateTo {
            point,
            side: cell_side_to_proto(side),
        },
    );

    state.next_autoscroll_at = Some(now + period);
}

#[cfg(all(test, not(feature = "thin-client")))]
mod tests {
    use super::*;

    #[test]
    fn resolve_pane_at_phys_ignores_parked_surfaces() {
        use bevy::ecs::system::RunSystemOnce;
        use ozmux_multiplexer::SurfaceMarker;

        // Both surfaces' geometry covers (400, 300), but only one is the active
        // (`Slotted`) surface. The parked surface retains stale, oversized
        // `ComputedNode` geometry after being unslotted; the hit-test must
        // ignore it and return the slotted surface. Regression: a
        // terminal-surface click resolving to a parked surface of an
        // already-active pane left keyboard focus stuck on a webview.
        let mut app = App::new();
        app.world_mut().spawn((
            SurfaceMarker,
            ComputedNode {
                size: Vec2::new(800.0, 600.0),
                ..ComputedNode::DEFAULT
            },
            UiGlobalTransform::from_xy(400.0, 300.0),
        ));
        let visible = app
            .world_mut()
            .spawn((
                SurfaceMarker,
                Slotted,
                ComputedNode {
                    size: Vec2::new(800.0, 600.0),
                    ..ComputedNode::DEFAULT
                },
                UiGlobalTransform::from_xy(400.0, 300.0),
            ))
            .id();

        let resolved = app
            .world_mut()
            .run_system_once(
                |hosts: Query<
                    (Entity, &ComputedNode, &UiGlobalTransform),
                    (With<SurfaceMarker>, With<Slotted>),
                >| {
                    resolve_pane_at_phys(&hosts, Vec2::new(400.0, 300.0)).map(|(e, _)| e)
                },
            )
            .unwrap();

        assert_eq!(
            resolved,
            Some(visible),
            "the hit-test must ignore parked (non-Slotted) surfaces",
        );
    }

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
        use ozmux_multiplexer::{
            ActivePane, MultiplexerCommands, MultiplexerPlugin, SplitOrientation,
        };

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(MultiplexerPlugin);
        app.add_plugins(crate::action::split_pane::SplitPaneActionPlugin);

        let (workspace, original_pane, original_surface) = app
            .world_mut()
            .run_system_once(|mut mux: MultiplexerCommands| {
                let o = mux.create_workspace(Some("test".into()));
                (o.workspace, o.pane, o.surface)
            })
            .unwrap();
        app.world_mut().flush();

        app.world_mut()
            .run_system_once(move |mut commands: Commands| {
                commands.trigger(crate::action::split_pane::SplitPaneActionEvent {
                    workspace,
                    orientation: SplitOrientation::Horizontal,
                });
            })
            .unwrap();
        app.world_mut().flush();
        let new_pane = app
            .world_mut()
            .run_system_once(move |mux: MultiplexerCommands| mux.workspaces_active_pane(workspace))
            .unwrap()
            .expect("active pane after split");
        assert_ne!(new_pane, original_pane, "split must promote fresh pane");

        // The Surface entity IS its own host: the click target is the surface
        // itself; `try_click_to_focus` resolves its owning pane via `SurfaceOf`.
        let mutated = app
            .world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| -> bool {
                try_click_to_focus(&mut mux, workspace, original_surface)
            })
            .unwrap();
        assert!(
            mutated,
            "click-to-focus must mutate when targeting a non-active pane"
        );
        assert_eq!(
            app.world().get::<ActivePane>(workspace).map(|a| a.0),
            Some(original_pane),
            "active pane must now be the click target"
        );

        let mutated_again = app
            .world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| -> bool {
                try_click_to_focus(&mut mux, workspace, original_surface)
            })
            .unwrap();
        assert!(
            !mutated_again,
            "second click on already-active pane returns false"
        );

        let stranger = app.world_mut().spawn_empty().id();
        let mutated_unknown = app
            .world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| -> bool {
                try_click_to_focus(&mut mux, workspace, stranger)
            })
            .unwrap();
        assert!(!mutated_unknown, "unknown entity must not mutate focus");
    }

    #[test]
    fn dispatch_translates_left_press_to_arm_drag() {
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
        assert!(matches!(action, ButtonAction::ArmDrag { .. }));
    }

    #[test]
    fn should_drop_stale_drag_returns_false_for_armed_drag() {
        let opts = bevy_terminal::SpawnOptions {
            cols: 80,
            rows: 24,
            shell: std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into()),
            cwd: None,
            env: Vec::new(),
        };
        let bundle = bevy_terminal::TerminalBundle::spawn(opts).unwrap();
        // Fresh bundle has no selection. An Armed drag should NOT be
        // considered stale (the absence of selection is expected
        // pre-materialization).
        let armed = ActiveDrag {
            entity: Entity::from_bits(1),
            anchor_cell: CellCoord { col: 1, row: 1 },
            last_drag_cell: None,
            phase: DragPhase::Armed {
                ty: bevy_terminal::SelectionType::Simple,
                anchor_side: bevy_terminal::Side::Left,
            },
        };
        assert!(!super::should_drop_stale_drag(&armed, &bundle.handle));
    }

    #[test]
    fn should_drop_stale_drag_returns_true_for_active_drag_with_no_selection() {
        let opts = bevy_terminal::SpawnOptions {
            cols: 80,
            rows: 24,
            shell: std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into()),
            cwd: None,
            env: Vec::new(),
        };
        let bundle = bevy_terminal::TerminalBundle::spawn(opts).unwrap();
        // Active drag + no Term::selection = alacritty wiped it out
        // from under us (alt-screen swap, screen reset).
        let active = ActiveDrag {
            entity: Entity::from_bits(1),
            anchor_cell: CellCoord { col: 1, row: 1 },
            last_drag_cell: None,
            phase: DragPhase::Active,
        };
        assert!(super::should_drop_stale_drag(&active, &bundle.handle));
    }

    #[test]
    fn apply_action_arm_drag_clears_prior_selection_and_arms_state() {
        use bevy::ecs::system::RunSystemOnce;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<MouseSelectionState>();

        let opts = bevy_terminal::SpawnOptions {
            cols: 80,
            rows: 24,
            shell: std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into()),
            cwd: None,
            env: Vec::new(),
        };
        let bundle = bevy_terminal::TerminalBundle::spawn(opts).unwrap();
        let entity = app.world_mut().spawn(bundle).id();

        // Seed a prior selection so ArmDrag's selection_clear is observable.
        app.world_mut()
            .run_system_once(
                move |mut handles: Query<&mut bevy_terminal::TerminalHandle>| {
                    let mut handle = handles.get_mut(entity).unwrap();
                    let anchor =
                        bevy_terminal::Point::new(bevy_terminal::Line(2), bevy_terminal::Column(3));
                    handle.selection_start_at(
                        anchor,
                        bevy_terminal::Side::Left,
                        bevy_terminal::SelectionType::Simple,
                    );
                    assert!(
                        handle.selection_type().is_some(),
                        "fixture pre-condition: seed selection must exist before ArmDrag"
                    );
                },
            )
            .unwrap();

        app.world_mut()
            .run_system_once(
                move |mut state: ResMut<MouseSelectionState>,
                      mut handles: Query<(
                    &mut bevy_terminal::TerminalHandle,
                    &mut bevy_terminal::PtyHandle,
                )>,
                      copy_modes: Query<(), With<crate::ui::copy_mode::CopyModeState>>| {
                    super::apply_action(
                        &mut state,
                        bevy_terminal::ButtonEventKind::Press,
                        bevy_terminal::MouseButtonKind::Left,
                        bevy_terminal::ButtonAction::ArmDrag {
                            ty: bevy_terminal::SelectionType::Simple,
                            cell: CellCoord { col: 5, row: 7 },
                            side: bevy_terminal::Side::Right,
                        },
                        entity,
                        &mut handles,
                        &copy_modes,
                    );
                },
            )
            .unwrap();

        let state = app.world().resource::<MouseSelectionState>();
        let drag = state.drag.as_ref().expect("ArmDrag must arm state.drag");
        assert_eq!(drag.entity, entity);
        assert_eq!(drag.anchor_cell, CellCoord { col: 5, row: 7 });
        match &drag.phase {
            DragPhase::Armed { ty, anchor_side } => {
                assert!(matches!(ty, bevy_terminal::SelectionType::Simple));
                assert!(matches!(anchor_side, bevy_terminal::Side::Right));
            }
            DragPhase::Active => panic!("expected Armed phase, got Active"),
        }

        // ArmDrag also clears the prior selection on the focused pane.
        let handle = app
            .world()
            .get::<bevy_terminal::TerminalHandle>(entity)
            .unwrap();
        assert!(
            handle.selection_type().is_none(),
            "ArmDrag must drop any prior selection on the focused pane"
        );
    }

    #[test]
    fn apply_action_update_local_selection_materializes_armed_drag() {
        use bevy::ecs::system::RunSystemOnce;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<MouseSelectionState>();

        let opts = bevy_terminal::SpawnOptions {
            cols: 80,
            rows: 24,
            shell: std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into()),
            cwd: None,
            env: Vec::new(),
        };
        let bundle = bevy_terminal::TerminalBundle::spawn(opts).unwrap();
        let entity = app.world_mut().spawn(bundle).id();

        // Pre-arm a drag at (5, 7) of type Simple.
        {
            let mut state = app.world_mut().resource_mut::<MouseSelectionState>();
            state.drag = Some(ActiveDrag {
                entity,
                anchor_cell: CellCoord { col: 5, row: 7 },
                last_drag_cell: None,
                phase: DragPhase::Armed {
                    ty: bevy_terminal::SelectionType::Simple,
                    anchor_side: bevy_terminal::Side::Left,
                },
            });
        }

        // UpdateLocalSelection at a different cell triggers the transition.
        app.world_mut()
            .run_system_once(
                move |mut state: ResMut<MouseSelectionState>,
                      mut handles: Query<(
                    &mut bevy_terminal::TerminalHandle,
                    &mut bevy_terminal::PtyHandle,
                )>,
                      copy_modes: Query<(), With<crate::ui::copy_mode::CopyModeState>>| {
                    super::apply_action(
                        &mut state,
                        bevy_terminal::ButtonEventKind::Drag,
                        bevy_terminal::MouseButtonKind::Left,
                        bevy_terminal::ButtonAction::UpdateLocalSelection {
                            cell: CellCoord { col: 8, row: 7 },
                            side: bevy_terminal::Side::Right,
                        },
                        entity,
                        &mut handles,
                        &copy_modes,
                    );
                },
            )
            .unwrap();

        let state = app.world().resource::<MouseSelectionState>();
        let drag = state.drag.as_ref().expect("drag must remain set");
        assert!(
            drag.is_active(),
            "phase must be Active after first inter-cell update"
        );
    }

    #[test]
    fn synthesize_drag_cell_emits_once_per_cell_crossing() {
        // Fixture: an Active drag anchored at (1, 1) in a pane sized
        // 800×480 physical px, centered at (400, 240) — so its
        // top-left corner is (0, 0) and bottom-right is (800, 480).
        // Cell pitch is 10×20 physical px → 80 cols, 24 rows.
        let mut state = MouseSelectionState {
            drag: Some(ActiveDrag {
                entity: Entity::from_bits(1),
                anchor_cell: CellCoord { col: 1, row: 1 },
                last_drag_cell: None,
                phase: DragPhase::Active,
            }),
            ..Default::default()
        };

        let node = bevy::ui::ComputedNode {
            size: Vec2::new(800.0, 480.0),
            ..Default::default()
        };
        let transform = bevy::ui::UiGlobalTransform::from_xy(400.0, 240.0);

        // Cursor at (155, 25) → pane-local (155, 25). With 10×20-px
        // cells, that's col floor(155/10)+1 = 16, row floor(25/20)+1
        // = 2. frac_x = 0.5 → Side::Right.
        let result = super::synthesize_drag_cell(
            &mut state,
            Vec2::new(155.0, 25.0),
            10.0,
            20.0,
            Some((node, transform, 80, 24)),
        );
        assert_eq!(
            result,
            Some((CellCoord { col: 16, row: 2 }, Side::Right)),
            "first call must emit a Drag event at the new cell",
        );
        assert_eq!(
            state.drag.as_ref().unwrap().last_drag_cell,
            Some(CellCoord { col: 16, row: 2 }),
            "last_drag_cell must track the most recent emission",
        );

        // Second call with the same cursor → de-duplicated.
        let dup = super::synthesize_drag_cell(
            &mut state,
            Vec2::new(155.0, 25.0),
            10.0,
            20.0,
            Some((node, transform, 80, 24)),
        );
        assert!(dup.is_none(), "second call with same cell must return None");
    }

    #[test]
    fn synthesize_drag_cell_returns_none_without_drag() {
        let mut state = MouseSelectionState::default();
        let node = bevy::ui::ComputedNode {
            size: Vec2::new(800.0, 480.0),
            ..Default::default()
        };
        let transform = bevy::ui::UiGlobalTransform::from_xy(400.0, 240.0);
        let result = super::synthesize_drag_cell(
            &mut state,
            Vec2::new(155.0, 25.0),
            10.0,
            20.0,
            Some((node, transform, 80, 24)),
        );
        assert!(result.is_none());
    }

    #[test]
    fn synthesize_drag_cell_clamps_out_of_pane_cursor_to_edge() {
        // Drag anchored at (1, 1); pane top-left at (0, 0), size
        // 800×480. Cursor at (-50, 500) is outside the pane on both
        // axes → clamps to (0, 480). With 10×20 cells, that's col 1,
        // row 24.
        let mut state = MouseSelectionState {
            drag: Some(ActiveDrag {
                entity: Entity::from_bits(1),
                anchor_cell: CellCoord { col: 1, row: 1 },
                last_drag_cell: None,
                phase: DragPhase::Active,
            }),
            ..Default::default()
        };
        let node = bevy::ui::ComputedNode {
            size: Vec2::new(800.0, 480.0),
            ..Default::default()
        };
        let transform = bevy::ui::UiGlobalTransform::from_xy(400.0, 240.0);
        let result = super::synthesize_drag_cell(
            &mut state,
            Vec2::new(-50.0, 500.0),
            10.0,
            20.0,
            Some((node, transform, 80, 24)),
        );
        let (cell, _side) = result.expect("clamped cursor must still emit");
        assert_eq!(cell, CellCoord { col: 1, row: 24 });
    }

    #[test]
    fn apply_action_left_release_clears_state_drag_even_on_pty_forward_path() {
        // Regression: Shift+click → ArmDrag; user releases Shift before
        // mouse-release. The mouse-release now routes to PTY-forward
        // (WriteToPty), not Noop. The release-clear of state.drag must
        // still fire.
        use bevy::ecs::system::RunSystemOnce;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<MouseSelectionState>();

        let opts = bevy_terminal::SpawnOptions {
            cols: 80,
            rows: 24,
            shell: std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into()),
            cwd: None,
            env: Vec::new(),
        };
        let bundle = bevy_terminal::TerminalBundle::spawn(opts).unwrap();
        let entity = app.world_mut().spawn(bundle).id();

        // Pre-arm the drag (simulating the Shift+click → ArmDrag press).
        {
            let mut state = app.world_mut().resource_mut::<MouseSelectionState>();
            state.drag = Some(ActiveDrag {
                entity,
                anchor_cell: CellCoord { col: 1, row: 1 },
                last_drag_cell: None,
                phase: DragPhase::Armed {
                    ty: bevy_terminal::SelectionType::Simple,
                    anchor_side: bevy_terminal::Side::Left,
                },
            });
        }

        // Apply a left-release with WriteToPty (captured-mouse release).
        app.world_mut()
            .run_system_once(
                move |mut state: ResMut<MouseSelectionState>,
                      mut handles: Query<(
                    &mut bevy_terminal::TerminalHandle,
                    &mut bevy_terminal::PtyHandle,
                )>,
                      copy_modes: Query<(), With<crate::ui::copy_mode::CopyModeState>>| {
                    super::apply_action(
                        &mut state,
                        bevy_terminal::ButtonEventKind::Release,
                        bevy_terminal::MouseButtonKind::Left,
                        bevy_terminal::ButtonAction::WriteToPty(b"\x1b[<0;1;1m".to_vec()),
                        entity,
                        &mut handles,
                        &copy_modes,
                    );
                },
            )
            .unwrap();

        let state = app.world().resource::<MouseSelectionState>();
        assert!(
            state.drag.is_none(),
            "left-release must clear state.drag regardless of the action variant",
        );
    }

    #[test]
    fn end_of_frame_guard_drops_drag_when_armed_pane_closes() {
        // Regression: the end-of-frame guards must run for Armed drags.
        // Previously the autoscroll early-return skipped past them.
        // This test verifies the guard fires when the drag entity is gone.
        use bevy::ecs::system::RunSystemOnce;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<MouseSelectionState>();

        let dead_entity = Entity::from_bits(99999);

        // Set state.drag to an Armed drag pointing at a non-existent entity.
        // (Simulating a pane that closed mid-arm.)
        {
            let mut state = app.world_mut().resource_mut::<MouseSelectionState>();
            state.drag = Some(ActiveDrag {
                entity: dead_entity,
                anchor_cell: CellCoord { col: 5, row: 5 },
                last_drag_cell: None,
                phase: DragPhase::Armed {
                    ty: bevy_terminal::SelectionType::Simple,
                    anchor_side: bevy_terminal::Side::Left,
                },
            });
        }

        // Run the end-of-frame guards inline by mimicking the structure
        // in dispatch_mouse_buttons. The entity is not in the query, so
        // the Err(_) arm should drop state.drag.
        app.world_mut()
            .run_system_once(
                |mut state: ResMut<MouseSelectionState>,
                 handles: Query<(
                    &mut bevy_terminal::TerminalHandle,
                    &mut bevy_terminal::PtyHandle,
                )>| {
                    if let Some(drag) = state.drag.as_ref()
                        && handles.get(drag.entity).is_err()
                    {
                        state.drag = None;
                        state.next_autoscroll_at = None;
                    }
                },
            )
            .unwrap();

        let state = app.world().resource::<MouseSelectionState>();
        assert!(
            state.drag.is_none(),
            "end-of-frame guard must drop state.drag when entity is gone (Armed phase)",
        );
    }

    #[test]
    fn click_focuses_pane_whose_host_has_no_terminal_handle() {
        // Regression: clicking an extension/browser pane — a Surface entity
        // with NO `TerminalHandle` — must still move focus.
        // dispatch_mouse_buttons previously `continue`d at the terminal-handle
        // lookup *before* `try_click_to_focus` ran, so focus stayed on the
        // terminal and keystrokes kept routing to it.
        use bevy::ecs::message::Messages;
        use bevy::ecs::system::RunSystemOnce;
        use bevy::math::DVec2;
        use bevy::window::WindowResolution;
        use ozmux_multiplexer::{
            ActivePane, AttachedWorkspace, MultiplexerCommands, MultiplexerPlugin, Side,
            SplitOrientation,
        };

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(MultiplexerPlugin);
        app.init_resource::<MouseSelectionState>();
        app.insert_resource(ButtonInput::<KeyCode>::default());
        app.insert_resource(OzmuxConfigsResource(ozmux_configs::OzmuxConfigs::default()));
        app.insert_resource(bevy_terminal_renderer::TerminalCellMetricsResource {
            metrics: bevy_terminal_renderer::CellMetrics {
                advance_phys: 8.0,
                line_height_phys: 16.0,
                ascent_phys: 12.0,
                descent_phys: 4.0,
                underline_position_phys: -2.0,
                underline_thickness_phys: 1.0,
                max_overflow_phys: 0.0,
            },
            phys_font_size: 12,
        });
        app.add_message::<MouseButtonInput>();
        app.add_message::<CursorMoved>();
        app.add_systems(Update, dispatch_mouse_buttons);
        // NOTE: run once so Startup (materialize_mux_snapshot) fires before any
        // MultiplexerCommands calls; otherwise a later app.update() would re-materialize
        // the Mux-seeded workspace, duplicating ECS entities and corrupting reverse maps.
        app.update();

        // Two-pane workspace: original (terminal) is re-focused after the split,
        // so the test starts with the terminal pane active.
        // create_workspace / split / focus-reset each queue deferred Commands, so
        // flush between steps — split_pane reads the pane via a query and would
        // PaneNotFound it before the create flush.
        let (workspace, original_pane) = app
            .world_mut()
            .run_system_once(|mut mux: MultiplexerCommands| {
                let o = mux.create_workspace(Some("t".into()));
                (o.workspace, o.pane)
            })
            .unwrap();
        app.world_mut().flush();

        let ext_pane = app
            .world_mut()
            .run_system_once(move |mut mux: MultiplexerCommands| {
                let ext_pane = mux
                    .split_pane(original_pane, Side::After, SplitOrientation::Horizontal)
                    .expect("split_pane");
                // Re-focus the original (terminal) pane so the test starts there.
                mux.set_active_pane(workspace, original_pane)
                    .expect("set_active_pane");
                ext_pane
            })
            .unwrap();
        app.world_mut().flush();

        // The new pane's ActiveSurface is wired via deferred Commands, so read
        // it only after the flush above.
        let ext_surface = app
            .world_mut()
            .run_system_once(move |mux: MultiplexerCommands| mux.panes_active_surface(ext_pane))
            .unwrap()
            .expect("new pane has an active surface");
        app.world_mut()
            .entity_mut(workspace)
            .insert(AttachedWorkspace);

        // The clicked surface IS its own host: decorate the ext pane's active
        // Surface with a laid-out node under the cursor, but NO TerminalHandle
        // (a webview surface). `Slotted` marks it as the active (slotted)
        // surface so the hit-test considers it.
        app.world_mut().entity_mut(ext_surface).insert((
            Slotted,
            ComputedNode {
                size: Vec2::new(800.0, 600.0),
                ..ComputedNode::DEFAULT
            },
            UiGlobalTransform::from_xy(400.0, 300.0),
        ));

        let window = app
            .world_mut()
            .spawn((
                Window {
                    resolution: WindowResolution::new(800, 600),
                    ..default()
                },
                PrimaryWindow,
            ))
            .id();
        app.world_mut()
            .get_mut::<Window>(window)
            .unwrap()
            .set_physical_cursor_position(Some(DVec2::new(400.0, 300.0)));

        assert_eq!(
            app.world().get::<ActivePane>(workspace).map(|a| a.0),
            Some(original_pane),
            "precondition: the terminal pane is focused before the click",
        );

        app.world_mut()
            .resource_mut::<Messages<MouseButtonInput>>()
            .write(MouseButtonInput {
                button: MouseButton::Left,
                state: ButtonState::Pressed,
                window,
            });
        app.update();

        assert_eq!(
            app.world().get::<ActivePane>(workspace).map(|a| a.0),
            Some(ext_pane),
            "clicking a pane whose host has no TerminalHandle must still move focus to it",
        );
    }
}
