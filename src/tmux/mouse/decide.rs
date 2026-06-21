//! Pure decision functions for the tmux mouse gesture system.
//!
//! Each `decide_*` function takes the gesture state plus plain, pre-resolved
//! input data and returns the `TmuxMouseEffect`s to apply this frame, mutating
//! only the state machine. Per behavior-preservation invariant 8, the deciders
//! never write the send-confirmed bookkeeping fields (`begun` / `last_target`
//! on the begin path, `last_sent`, `resized`); those are committed by
//! `on_tmux_mouse_effects` only on send success.

use super::GestureState;
use super::effect::{MultiSelectKind, TmuxMouseEffect};
use crate::tmux::render::DividerPixelRect;
use bevy::prelude::*;
use ozmux_tmux::PaneId;
use std::time::Duration;
use tmux_control_parser::DividerAxis;

/// The hit-test outcome of a left press: either a divider grab (resolved to its
/// primary pane's near edge and current size) or a pane focus.
pub(super) enum PressHit {
    /// Press landed in a divider's grab zone whose primary pane has geometry.
    Divider {
        divider: DividerPixelRect,
        near: i32,
        last_sent: u32,
    },
    /// Press landed on a pane body (no resizable divider under the cursor).
    Pane {
        pane: Entity,
        pane_id: PaneId,
        origin_phys: Vec2,
        cursor_logical: Vec2,
    },
}

/// Pre-resolved inputs for `decide_release` (the released gesture is read from
/// the state itself, which the decider then replaces with `Idle`).
pub(super) struct ReleaseCtx {
    /// Whether the `Pressed` pane is in copy mode (multi-click promotion gate).
    pub(super) copy_mode: bool,
    /// Resolved click cell for a `click_count >= 2` release, when available.
    pub(super) multi_cell: Option<(u16, u16)>,
    /// Pane under the cursor for a divider-click focus fallback.
    pub(super) pane_under: Option<PaneId>,
}

/// Pre-resolved per-state inputs for `decide_continuation`. Only the fields
/// relevant to the active state are read; the rest stay neutral / `None`.
pub(super) struct ContinuationCtx {
    /// Whether the active gesture's pane entity is still alive.
    pub(super) pane_alive: bool,
    /// Live physical-pixel cursor position, when the window has one.
    pub(super) cursor_phys: Option<Vec2>,
    /// Drag-promotion threshold in physical pixels.
    pub(super) drag_threshold_phys: f32,
    /// Whether the `Pressed` pane is in copy mode (drag promotion gate).
    pub(super) copy_mode: bool,
    /// Anchor cell resolved from the press origin (`Pressed` → `Selecting`).
    pub(super) anchor_cell: Option<(u16, u16)>,
    /// Copy-mode snapshot cursor (`Selecting` / `PendingMultiSelect`).
    pub(super) snapshot_cursor: Option<(u16, u16)>,
    /// Cell resolved from the live cursor (`Selecting` extend).
    pub(super) selecting_cell: Option<(u16, u16)>,
    /// `floor(cursor.major / cell.major)` on the divider's major axis (`Resizing`).
    pub(super) resize_pointer_cell: Option<i32>,
    /// Whether a tmux client is connected this frame. Only the
    /// `PendingMultiSelect` arm reads it: it leaves the state and emits
    /// `MultiSelect` only with a client, so a snapshot-present-but-no-client
    /// frame stays `PendingMultiSelect` and retries (the observer would
    /// otherwise drop the multi-select+copy with no client).
    pub(super) client_present: bool,
}

/// Tracks consecutive-click count using a timeout + positional drift gate.
#[derive(Default)]
pub(super) struct ClickTracker {
    last: Option<(Duration, Vec2, u8)>,
}

impl ClickTracker {
    /// Registers a press at `now` / `pos` and returns the resulting click count
    /// (1 = single, 2 = double, 3 = triple, capped at 3). `cfg` is
    /// `(double_click_timeout, click_drift_px)` in logical units.
    pub(super) fn register(&mut self, now: Duration, pos: Vec2, cfg: (Duration, f32)) -> u8 {
        let (timeout, drift) = cfg;
        let count = match self.last {
            Some((t, p, c)) if now.saturating_sub(t) <= timeout && p.distance(pos) <= drift => {
                (c + 1).min(3)
            }
            _ => 1,
        };
        self.last = Some((now, pos, count));
        count
    }
}

/// Resolves a left press into the gesture-state transition and effects.
///
/// A divider grab enters `Resizing` with no effect; a pane press focuses the
/// pane (`SelectPane`), records the click count, and enters `Pressed`.
pub(super) fn decide_press(
    state: &mut GestureState,
    click: &mut ClickTracker,
    hit: PressHit,
    now: Duration,
    dbl_click: (Duration, f32),
) -> Vec<TmuxMouseEffect> {
    match hit {
        PressHit::Divider {
            divider,
            near,
            last_sent,
        } => {
            *state = GestureState::Resizing {
                divider,
                near,
                last_sent,
                resized: false,
            };
            Vec::new()
        }
        PressHit::Pane {
            pane,
            pane_id,
            origin_phys,
            cursor_logical,
        } => {
            let count = click.register(now, cursor_logical, dbl_click);
            *state = GestureState::Pressed {
                pane,
                pane_id,
                origin_phys,
                click_count: count,
            };
            vec![TmuxMouseEffect::SelectPane(pane_id)]
        }
    }
}

/// Resolves a left release: takes the prior state to `Idle` (via `mem::replace`),
/// then decides the transition + effects from that prior state.
///
/// A begun `Selecting` copies the selection; a multi-click (>=2) in copy mode
/// with a resolved cell enters `PendingMultiSelect` (otherwise stays `Idle`); a
/// `Resizing` that never dragged focuses the pane under the cursor as a fallback
/// click.
pub(super) fn decide_release(state: &mut GestureState, ctx: ReleaseCtx) -> Vec<TmuxMouseEffect> {
    let prior = std::mem::replace(state, GestureState::Idle);
    match prior {
        GestureState::Selecting { pane_id, begun, .. } if begun => {
            vec![TmuxMouseEffect::CopySelection { pane: pane_id }]
        }
        GestureState::Pressed {
            pane,
            pane_id,
            click_count,
            ..
        } if click_count >= 2 && ctx.copy_mode => {
            let Some(cell) = ctx.multi_cell else {
                return Vec::new();
            };
            let kind = if click_count == 2 {
                MultiSelectKind::Word
            } else {
                MultiSelectKind::Line
            };
            *state = GestureState::PendingMultiSelect {
                pane,
                pane_id,
                cell,
                kind,
            };
            Vec::new()
        }
        GestureState::Resizing { resized, .. } if !resized => match ctx.pane_under {
            Some(pane_id) => vec![TmuxMouseEffect::SelectPane(pane_id)],
            None => Vec::new(),
        },
        _ => Vec::new(),
    }
}

/// Resolves the per-frame continuation of an in-progress gesture into a state
/// transition and at most one arm's effects.
///
/// Per behavior-preservation invariant 8, this never writes the send-confirmed
/// fields (`begun` / `last_target` on the begin path, `last_sent`, `resized`);
/// the apply observer commits those on send success. The `PendingMultiSelect`
/// arm additionally requires `ctx.client_present`: with no client it stays
/// `PendingMultiSelect` and emits nothing, since the observer would drop the
/// multi-select+copy with no client (the `Selecting` / `Resizing` arms are
/// already observer-committed-on-success, so they retry without a client).
pub(super) fn decide_continuation(
    state: &mut GestureState,
    ctx: ContinuationCtx,
) -> Vec<TmuxMouseEffect> {
    match &mut *state {
        GestureState::Pressed {
            pane,
            pane_id,
            origin_phys,
            ..
        } => {
            let pane = *pane;
            let pane_id = *pane_id;
            let origin_phys = *origin_phys;
            let Some(cursor_phys) = ctx.cursor_phys else {
                return Vec::new();
            };
            if cursor_phys.distance(origin_phys) <= ctx.drag_threshold_phys {
                return Vec::new();
            }
            if !ctx.pane_alive {
                *state = GestureState::Idle;
                return Vec::new();
            }
            if ctx.copy_mode {
                let Some(anchor) = ctx.anchor_cell else {
                    return Vec::new();
                };
                *state = GestureState::Selecting {
                    pane,
                    pane_id,
                    anchor,
                    begun: false,
                    last_target: None,
                };
            } else {
                *state = GestureState::Idle;
            }
            Vec::new()
        }
        GestureState::Selecting {
            pane_id,
            anchor,
            begun,
            last_target,
            ..
        } => {
            let pane_id = *pane_id;
            let anchor = *anchor;
            let begun = *begun;
            let last_target = *last_target;
            if !ctx.pane_alive {
                *state = GestureState::Idle;
                return Vec::new();
            }
            let Some(snapshot_cursor) = ctx.snapshot_cursor else {
                return Vec::new();
            };
            let Some(cell) = ctx.selecting_cell else {
                return Vec::new();
            };
            if !begun {
                vec![TmuxMouseEffect::BeginCopyDrag {
                    pane: pane_id,
                    snapshot_cursor,
                    anchor,
                }]
            } else if Some(cell) != last_target {
                vec![TmuxMouseEffect::ExtendCopyDrag {
                    pane: pane_id,
                    snapshot_cursor,
                    cell,
                }]
            } else {
                Vec::new()
            }
        }
        GestureState::PendingMultiSelect {
            pane_id,
            cell,
            kind,
            ..
        } => {
            let pane_id = *pane_id;
            let cell = *cell;
            let kind = *kind;
            if !ctx.pane_alive {
                *state = GestureState::Idle;
                return Vec::new();
            }
            let Some(snapshot_cursor) = ctx.snapshot_cursor else {
                return Vec::new();
            };
            if !ctx.client_present {
                return Vec::new();
            }
            *state = GestureState::Idle;
            vec![TmuxMouseEffect::MultiSelect {
                pane: pane_id,
                kind,
                snapshot_cursor,
                cell,
            }]
        }
        GestureState::Resizing {
            divider,
            near,
            last_sent,
            ..
        } => {
            let divider = *divider;
            let near = *near;
            let last_sent = *last_sent;
            let Some(pointer_cell) = ctx.resize_pointer_cell else {
                return Vec::new();
            };
            let target = resize_target_size(near, pointer_cell);
            if target == last_sent {
                return Vec::new();
            }
            vec![TmuxMouseEffect::ResizePane {
                axis: divider.axis,
                primary: divider.primary,
                size: target,
            }]
        }
        GestureState::Idle => Vec::new(),
    }
}

/// Returns the divider whose grab zone contains `cursor` (logical px), given a
/// tolerance in logical px. The grab zone is the 1px gap at `[pos_px, pos_px+1)`
/// on the major axis expanded by `tol` on each side, intersected with the
/// divider's span on the perpendicular axis.
pub(crate) fn divider_at(
    dividers: &[DividerPixelRect],
    cursor: Vec2,
    tol: f32,
) -> Option<DividerPixelRect> {
    dividers.iter().copied().find(|d| match d.axis {
        DividerAxis::Vertical => {
            cursor.x >= d.pos_px - tol
                && cursor.x <= d.pos_px + 1.0 + tol
                && cursor.y >= d.span_start_px
                && cursor.y < d.span_end_px
        }
        DividerAxis::Horizontal => {
            cursor.y >= d.pos_px - tol
                && cursor.y <= d.pos_px + 1.0 + tol
                && cursor.x >= d.span_start_px
                && cursor.x < d.span_end_px
        }
    })
}

/// New absolute size (cells) for a divider's primary pane given the pointer's
/// cell coordinate on the major axis. The pane's near edge stays fixed; its far
/// edge follows the pointer. Clamped to at least 1.
fn resize_target_size(near: i32, pointer_cell: i32) -> u32 {
    (pointer_cell - near).max(1) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel_vdiv(pos: f32, span_start: f32, span_end: f32) -> DividerPixelRect {
        DividerPixelRect {
            axis: DividerAxis::Vertical,
            primary: PaneId(1),
            pos_px: pos,
            span_start_px: span_start,
            span_end_px: span_end,
        }
    }

    fn base_ctx() -> ContinuationCtx {
        ContinuationCtx {
            pane_alive: true,
            cursor_phys: None,
            drag_threshold_phys: 0.0,
            copy_mode: false,
            anchor_cell: None,
            snapshot_cursor: None,
            selecting_cell: None,
            resize_pointer_cell: None,
            client_present: true,
        }
    }

    #[test]
    fn pixel_hit_test_within_tolerance() {
        let d = pixel_vdiv(320.0, 0.0, 384.0);
        let hit = divider_at(&[d], Vec2::new(322.0, 100.0), 4.0);
        assert!(hit.is_some());
        assert_eq!(hit.unwrap().primary, PaneId(1));
    }

    #[test]
    fn pixel_hit_test_outside_tolerance() {
        let d = pixel_vdiv(320.0, 0.0, 384.0);
        assert!(divider_at(&[d], Vec2::new(330.0, 100.0), 4.0).is_none());
    }

    #[test]
    fn pixel_hit_test_outside_span() {
        let d = pixel_vdiv(320.0, 0.0, 192.0);
        assert!(divider_at(&[d], Vec2::new(320.0, 200.0), 4.0).is_none());
    }

    #[test]
    fn pixel_hit_test_far_side_of_tolerance() {
        let d = pixel_vdiv(320.0, 0.0, 384.0);
        assert!(divider_at(&[d], Vec2::new(324.0, 100.0), 4.0).is_some());
    }

    #[test]
    fn click_count_increments_within_timeout_and_drift() {
        let mut t = ClickTracker::default();
        let cfg = (Duration::from_millis(400), 8.0f32);
        assert_eq!(
            t.register(Duration::from_millis(0), Vec2::new(10.0, 10.0), cfg),
            1
        );
        assert_eq!(
            t.register(Duration::from_millis(200), Vec2::new(11.0, 11.0), cfg),
            2
        );
        assert_eq!(
            t.register(Duration::from_millis(350), Vec2::new(12.0, 10.0), cfg),
            3
        );
    }

    #[test]
    fn click_count_resets_after_timeout() {
        let mut t = ClickTracker::default();
        let cfg = (Duration::from_millis(400), 8.0f32);
        assert_eq!(
            t.register(Duration::from_millis(0), Vec2::new(10.0, 10.0), cfg),
            1
        );
        assert_eq!(
            t.register(Duration::from_millis(500), Vec2::new(10.0, 10.0), cfg),
            1
        );
    }

    #[test]
    fn click_count_resets_after_drift() {
        let mut t = ClickTracker::default();
        let cfg = (Duration::from_millis(400), 8.0f32);
        assert_eq!(
            t.register(Duration::from_millis(0), Vec2::new(10.0, 10.0), cfg),
            1
        );
        assert_eq!(
            t.register(Duration::from_millis(100), Vec2::new(40.0, 40.0), cfg),
            1
        );
    }

    #[test]
    fn resize_target_size_follows_pointer() {
        assert_eq!(resize_target_size(0, 50), 50);
        assert_eq!(resize_target_size(10, 25), 15);
        assert_eq!(resize_target_size(0, 0), 1);
    }

    #[test]
    fn press_on_pane_focuses_and_enters_pressed() {
        let mut st = GestureState::Idle;
        let mut click = ClickTracker::default();
        let fx = decide_press(
            &mut st,
            &mut click,
            PressHit::Pane {
                pane: Entity::from_bits(1),
                pane_id: PaneId(7),
                origin_phys: Vec2::ZERO,
                cursor_logical: Vec2::ZERO,
            },
            Duration::ZERO,
            (Duration::from_millis(400), 8.0),
        );
        assert_eq!(fx, vec![TmuxMouseEffect::SelectPane(PaneId(7))]);
        assert!(matches!(st, GestureState::Pressed { click_count: 1, .. }));
    }

    #[test]
    fn press_on_divider_enters_resizing_without_effect() {
        let mut st = GestureState::Idle;
        let mut click = ClickTracker::default();
        let div = pixel_vdiv(320.0, 0.0, 384.0);
        let fx = decide_press(
            &mut st,
            &mut click,
            PressHit::Divider {
                divider: div,
                near: 5,
                last_sent: 42,
            },
            Duration::ZERO,
            (Duration::from_millis(400), 8.0),
        );
        assert!(fx.is_empty());
        assert!(matches!(
            st,
            GestureState::Resizing {
                near: 5,
                last_sent: 42,
                resized: false,
                ..
            }
        ));
    }

    #[test]
    fn release_from_begun_selecting_copies() {
        let mut st = GestureState::Selecting {
            pane: Entity::from_bits(1),
            pane_id: PaneId(7),
            anchor: (3, 4),
            begun: true,
            last_target: Some((5, 4)),
        };
        let fx = decide_release(
            &mut st,
            ReleaseCtx {
                copy_mode: false,
                multi_cell: None,
                pane_under: None,
            },
        );
        assert_eq!(fx, vec![TmuxMouseEffect::CopySelection { pane: PaneId(7) }]);
        assert!(matches!(st, GestureState::Idle));
    }

    #[test]
    fn release_from_unbegun_selecting_does_not_copy() {
        let mut st = GestureState::Selecting {
            pane: Entity::from_bits(1),
            pane_id: PaneId(7),
            anchor: (3, 4),
            begun: false,
            last_target: None,
        };
        let fx = decide_release(
            &mut st,
            ReleaseCtx {
                copy_mode: false,
                multi_cell: None,
                pane_under: None,
            },
        );
        assert!(fx.is_empty());
        assert!(matches!(st, GestureState::Idle));
    }

    #[test]
    fn release_double_click_in_copy_mode_enters_pending_word() {
        let mut st = GestureState::Pressed {
            pane: Entity::from_bits(1),
            pane_id: PaneId(7),
            origin_phys: Vec2::ZERO,
            click_count: 2,
        };
        let fx = decide_release(
            &mut st,
            ReleaseCtx {
                copy_mode: true,
                multi_cell: Some((6, 2)),
                pane_under: None,
            },
        );
        assert!(fx.is_empty());
        assert!(matches!(
            st,
            GestureState::PendingMultiSelect {
                cell: (6, 2),
                kind: MultiSelectKind::Word,
                ..
            }
        ));
    }

    #[test]
    fn release_triple_click_in_copy_mode_enters_pending_line() {
        let mut st = GestureState::Pressed {
            pane: Entity::from_bits(1),
            pane_id: PaneId(7),
            origin_phys: Vec2::ZERO,
            click_count: 3,
        };
        let fx = decide_release(
            &mut st,
            ReleaseCtx {
                copy_mode: true,
                multi_cell: Some((6, 2)),
                pane_under: None,
            },
        );
        assert!(fx.is_empty());
        assert!(matches!(
            st,
            GestureState::PendingMultiSelect {
                kind: MultiSelectKind::Line,
                ..
            }
        ));
    }

    #[test]
    fn release_double_click_not_in_copy_mode_goes_idle() {
        let mut st = GestureState::Pressed {
            pane: Entity::from_bits(1),
            pane_id: PaneId(7),
            origin_phys: Vec2::ZERO,
            click_count: 2,
        };
        let fx = decide_release(
            &mut st,
            ReleaseCtx {
                copy_mode: false,
                multi_cell: Some((6, 2)),
                pane_under: None,
            },
        );
        assert!(fx.is_empty());
        assert!(matches!(st, GestureState::Idle));
    }

    #[test]
    fn release_double_click_without_cell_goes_idle() {
        let mut st = GestureState::Pressed {
            pane: Entity::from_bits(1),
            pane_id: PaneId(7),
            origin_phys: Vec2::ZERO,
            click_count: 2,
        };
        let fx = decide_release(
            &mut st,
            ReleaseCtx {
                copy_mode: true,
                multi_cell: None,
                pane_under: None,
            },
        );
        assert!(fx.is_empty());
        assert!(matches!(st, GestureState::Idle));
    }

    #[test]
    fn release_resizing_click_focuses_pane_under() {
        let div = pixel_vdiv(320.0, 0.0, 384.0);
        let mut st = GestureState::Resizing {
            divider: div,
            near: 0,
            last_sent: 10,
            resized: false,
        };
        let fx = decide_release(
            &mut st,
            ReleaseCtx {
                copy_mode: false,
                multi_cell: None,
                pane_under: Some(PaneId(9)),
            },
        );
        assert_eq!(fx, vec![TmuxMouseEffect::SelectPane(PaneId(9))]);
        assert!(matches!(st, GestureState::Idle));
    }

    #[test]
    fn release_resizing_that_dragged_does_not_focus() {
        let div = pixel_vdiv(320.0, 0.0, 384.0);
        let mut st = GestureState::Resizing {
            divider: div,
            near: 0,
            last_sent: 10,
            resized: true,
        };
        let fx = decide_release(
            &mut st,
            ReleaseCtx {
                copy_mode: false,
                multi_cell: None,
                pane_under: Some(PaneId(9)),
            },
        );
        assert!(fx.is_empty());
        assert!(matches!(st, GestureState::Idle));
    }

    #[test]
    fn continuation_pressed_below_threshold_stays_pressed() {
        let mut st = GestureState::Pressed {
            pane: Entity::from_bits(1),
            pane_id: PaneId(7),
            origin_phys: Vec2::new(100.0, 100.0),
            click_count: 1,
        };
        let fx = decide_continuation(
            &mut st,
            ContinuationCtx {
                cursor_phys: Some(Vec2::new(101.0, 101.0)),
                drag_threshold_phys: 4.0,
                ..base_ctx()
            },
        );
        assert!(fx.is_empty());
        assert!(matches!(st, GestureState::Pressed { .. }));
    }

    #[test]
    fn continuation_pressed_drag_in_copy_mode_enters_selecting() {
        let mut st = GestureState::Pressed {
            pane: Entity::from_bits(1),
            pane_id: PaneId(7),
            origin_phys: Vec2::new(100.0, 100.0),
            click_count: 1,
        };
        let fx = decide_continuation(
            &mut st,
            ContinuationCtx {
                cursor_phys: Some(Vec2::new(200.0, 200.0)),
                drag_threshold_phys: 4.0,
                copy_mode: true,
                anchor_cell: Some((3, 4)),
                ..base_ctx()
            },
        );
        assert!(fx.is_empty());
        assert!(matches!(
            st,
            GestureState::Selecting {
                anchor: (3, 4),
                begun: false,
                last_target: None,
                ..
            }
        ));
    }

    #[test]
    fn continuation_pressed_drag_not_copy_mode_goes_idle() {
        let mut st = GestureState::Pressed {
            pane: Entity::from_bits(1),
            pane_id: PaneId(7),
            origin_phys: Vec2::new(100.0, 100.0),
            click_count: 1,
        };
        let fx = decide_continuation(
            &mut st,
            ContinuationCtx {
                cursor_phys: Some(Vec2::new(200.0, 200.0)),
                drag_threshold_phys: 4.0,
                copy_mode: false,
                ..base_ctx()
            },
        );
        assert!(fx.is_empty());
        assert!(matches!(st, GestureState::Idle));
    }

    #[test]
    fn continuation_pressed_drag_copy_mode_without_anchor_stays() {
        let mut st = GestureState::Pressed {
            pane: Entity::from_bits(1),
            pane_id: PaneId(7),
            origin_phys: Vec2::new(100.0, 100.0),
            click_count: 1,
        };
        let fx = decide_continuation(
            &mut st,
            ContinuationCtx {
                cursor_phys: Some(Vec2::new(200.0, 200.0)),
                drag_threshold_phys: 4.0,
                copy_mode: true,
                anchor_cell: None,
                ..base_ctx()
            },
        );
        assert!(fx.is_empty());
        assert!(matches!(st, GestureState::Pressed { .. }));
    }

    #[test]
    fn continuation_pressed_dead_pane_goes_idle() {
        let mut st = GestureState::Pressed {
            pane: Entity::from_bits(1),
            pane_id: PaneId(7),
            origin_phys: Vec2::new(100.0, 100.0),
            click_count: 1,
        };
        let fx = decide_continuation(
            &mut st,
            ContinuationCtx {
                pane_alive: false,
                cursor_phys: Some(Vec2::new(200.0, 200.0)),
                drag_threshold_phys: 4.0,
                ..base_ctx()
            },
        );
        assert!(fx.is_empty());
        assert!(matches!(st, GestureState::Idle));
    }

    #[test]
    fn continuation_selecting_first_frame_emits_begin_not_extend() {
        let mut st = GestureState::Selecting {
            pane: Entity::from_bits(1),
            pane_id: PaneId(7),
            anchor: (3, 4),
            begun: false,
            last_target: None,
        };
        let ctx = ContinuationCtx {
            pane_alive: true,
            cursor_phys: Some(Vec2::splat(99.0)),
            drag_threshold_phys: 4.0,
            copy_mode: true,
            anchor_cell: None,
            snapshot_cursor: Some((0, 0)),
            selecting_cell: Some((5, 4)),
            resize_pointer_cell: None,
            client_present: true,
        };
        let fx = decide_continuation(&mut st, ctx);
        assert_eq!(
            fx,
            vec![TmuxMouseEffect::BeginCopyDrag {
                pane: PaneId(7),
                snapshot_cursor: (0, 0),
                anchor: (3, 4)
            }]
        );
        assert!(matches!(st, GestureState::Selecting { begun: false, .. }));
    }

    #[test]
    fn continuation_selecting_begun_extends_on_new_cell() {
        let mut st = GestureState::Selecting {
            pane: Entity::from_bits(1),
            pane_id: PaneId(7),
            anchor: (3, 4),
            begun: true,
            last_target: Some((3, 4)),
        };
        let fx = decide_continuation(
            &mut st,
            ContinuationCtx {
                snapshot_cursor: Some((0, 0)),
                selecting_cell: Some((5, 4)),
                ..base_ctx()
            },
        );
        assert_eq!(
            fx,
            vec![TmuxMouseEffect::ExtendCopyDrag {
                pane: PaneId(7),
                snapshot_cursor: (0, 0),
                cell: (5, 4)
            }]
        );
        assert!(matches!(
            st,
            GestureState::Selecting {
                begun: true,
                last_target: Some((3, 4)),
                ..
            }
        ));
    }

    #[test]
    fn continuation_selecting_begun_same_cell_no_effect() {
        let mut st = GestureState::Selecting {
            pane: Entity::from_bits(1),
            pane_id: PaneId(7),
            anchor: (3, 4),
            begun: true,
            last_target: Some((5, 4)),
        };
        let fx = decide_continuation(
            &mut st,
            ContinuationCtx {
                snapshot_cursor: Some((0, 0)),
                selecting_cell: Some((5, 4)),
                ..base_ctx()
            },
        );
        assert!(fx.is_empty());
        assert!(matches!(st, GestureState::Selecting { begun: true, .. }));
    }

    #[test]
    fn continuation_selecting_without_snapshot_stays() {
        let mut st = GestureState::Selecting {
            pane: Entity::from_bits(1),
            pane_id: PaneId(7),
            anchor: (3, 4),
            begun: false,
            last_target: None,
        };
        let fx = decide_continuation(
            &mut st,
            ContinuationCtx {
                snapshot_cursor: None,
                selecting_cell: Some((5, 4)),
                ..base_ctx()
            },
        );
        assert!(fx.is_empty());
        assert!(matches!(st, GestureState::Selecting { begun: false, .. }));
    }

    #[test]
    fn continuation_selecting_dead_pane_goes_idle() {
        let mut st = GestureState::Selecting {
            pane: Entity::from_bits(1),
            pane_id: PaneId(7),
            anchor: (3, 4),
            begun: false,
            last_target: None,
        };
        let fx = decide_continuation(
            &mut st,
            ContinuationCtx {
                pane_alive: false,
                snapshot_cursor: Some((0, 0)),
                selecting_cell: Some((5, 4)),
                ..base_ctx()
            },
        );
        assert!(fx.is_empty());
        assert!(matches!(st, GestureState::Idle));
    }

    #[test]
    fn continuation_pending_multi_select_emits_then_idle() {
        let mut st = GestureState::PendingMultiSelect {
            pane: Entity::from_bits(1),
            pane_id: PaneId(7),
            cell: (6, 2),
            kind: MultiSelectKind::Word,
        };
        let fx = decide_continuation(
            &mut st,
            ContinuationCtx {
                snapshot_cursor: Some((0, 0)),
                ..base_ctx()
            },
        );
        assert_eq!(
            fx,
            vec![TmuxMouseEffect::MultiSelect {
                pane: PaneId(7),
                kind: MultiSelectKind::Word,
                snapshot_cursor: (0, 0),
                cell: (6, 2)
            }]
        );
        assert!(matches!(st, GestureState::Idle));
    }

    #[test]
    fn continuation_pending_multi_select_without_snapshot_stays() {
        let mut st = GestureState::PendingMultiSelect {
            pane: Entity::from_bits(1),
            pane_id: PaneId(7),
            cell: (6, 2),
            kind: MultiSelectKind::Word,
        };
        let fx = decide_continuation(
            &mut st,
            ContinuationCtx {
                snapshot_cursor: None,
                ..base_ctx()
            },
        );
        assert!(fx.is_empty());
        assert!(matches!(st, GestureState::PendingMultiSelect { .. }));
    }

    #[test]
    fn continuation_pending_multi_select_without_client_stays_then_retries() {
        let mut st = GestureState::PendingMultiSelect {
            pane: Entity::from_bits(1),
            pane_id: PaneId(7),
            cell: (6, 2),
            kind: MultiSelectKind::Word,
        };
        let fx = decide_continuation(
            &mut st,
            ContinuationCtx {
                snapshot_cursor: Some((0, 0)),
                client_present: false,
                ..base_ctx()
            },
        );
        assert!(
            fx.is_empty(),
            "a snapshot-present-but-no-client frame must emit nothing"
        );
        assert!(
            matches!(st, GestureState::PendingMultiSelect { .. }),
            "with no client the gesture must stay PendingMultiSelect to retry next frame"
        );

        let fx = decide_continuation(
            &mut st,
            ContinuationCtx {
                snapshot_cursor: Some((0, 0)),
                client_present: true,
                ..base_ctx()
            },
        );
        assert_eq!(
            fx,
            vec![TmuxMouseEffect::MultiSelect {
                pane: PaneId(7),
                kind: MultiSelectKind::Word,
                snapshot_cursor: (0, 0),
                cell: (6, 2)
            }],
            "once a client is present the retry emits the multi-select"
        );
        assert!(matches!(st, GestureState::Idle));
    }

    #[test]
    fn continuation_pending_multi_select_dead_pane_goes_idle() {
        let mut st = GestureState::PendingMultiSelect {
            pane: Entity::from_bits(1),
            pane_id: PaneId(7),
            cell: (6, 2),
            kind: MultiSelectKind::Word,
        };
        let fx = decide_continuation(
            &mut st,
            ContinuationCtx {
                pane_alive: false,
                snapshot_cursor: Some((0, 0)),
                ..base_ctx()
            },
        );
        assert!(fx.is_empty());
        assert!(matches!(st, GestureState::Idle));
    }

    #[test]
    fn continuation_resizing_emits_only_on_target_change() {
        let div = pixel_vdiv(0.0, 0.0, 384.0);
        let mut st = GestureState::Resizing {
            divider: DividerPixelRect {
                primary: PaneId(2),
                ..div
            },
            near: 0,
            last_sent: 10,
            resized: false,
        };
        let same = decide_continuation(
            &mut st,
            ContinuationCtx {
                resize_pointer_cell: Some(10),
                ..base_ctx()
            },
        );
        assert!(same.is_empty());
        let changed = decide_continuation(
            &mut st,
            ContinuationCtx {
                resize_pointer_cell: Some(12),
                ..base_ctx()
            },
        );
        assert_eq!(
            changed,
            vec![TmuxMouseEffect::ResizePane {
                axis: DividerAxis::Vertical,
                primary: PaneId(2),
                size: 12
            }]
        );
        assert!(matches!(
            st,
            GestureState::Resizing {
                last_sent: 10,
                resized: false,
                ..
            }
        ));
    }

    #[test]
    fn continuation_resizing_without_pointer_cell_no_effect() {
        let div = pixel_vdiv(0.0, 0.0, 384.0);
        let mut st = GestureState::Resizing {
            divider: div,
            near: 0,
            last_sent: 10,
            resized: false,
        };
        let fx = decide_continuation(
            &mut st,
            ContinuationCtx {
                resize_pointer_cell: None,
                ..base_ctx()
            },
        );
        assert!(fx.is_empty());
        assert!(matches!(st, GestureState::Resizing { .. }));
    }

    #[test]
    fn continuation_idle_no_effect() {
        let mut st = GestureState::Idle;
        let fx = decide_continuation(&mut st, base_ctx());
        assert!(fx.is_empty());
        assert!(matches!(st, GestureState::Idle));
    }
}
