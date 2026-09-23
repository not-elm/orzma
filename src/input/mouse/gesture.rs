//! Click, wheel, and drag gesture primitives.

use bevy::input::mouse::MouseScrollUnit;
use bevy::prelude::*;
use orzma_tty::prelude::{CellCoord, PointerButton};
use std::time::Duration;

/// The terminal a held button locked the gesture to, and which buttons are
/// held. Every later press, motion, and release goes to `entity` until no
/// button is held, even when the pointer wanders onto another terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::input::mouse) struct HeldPointer {
    pub entity: Entity,
    buttons: [bool; 3],
}

impl HeldPointer {
    /// A lock on `entity` with no button held yet.
    pub fn new(entity: Entity) -> Self {
        Self {
            entity,
            buttons: [false; 3],
        }
    }

    /// Marks `button` held.
    pub fn press(&mut self, button: PointerButton) {
        self.buttons[slot(button)] = true;
    }

    /// Marks `button` released.
    pub fn release(&mut self, button: PointerButton) {
        self.buttons[slot(button)] = false;
    }

    /// Whether `button` is held.
    pub fn holds(&self, button: PointerButton) -> bool {
        self.buttons[slot(button)]
    }

    /// Whether no button is held.
    pub fn is_empty(&self) -> bool {
        !self.buttons.contains(&true)
    }
}

/// Tracks the current mouse gesture and consecutive-click count.
#[derive(Resource, Default)]
pub(in crate::input::mouse) struct OrzmaMouseGesture {
    /// Consecutive-click counter for multi-click detection.
    pub click: ClickTracker,
    /// The locked terminal and held buttons, or `None` when no button is
    /// held.
    pub held: Option<HeldPointer>,
    /// The terminal and cell the last pointer event named, so motion is
    /// sent only when either changes.
    pub last_target: Option<(Entity, CellCoord)>,
    /// Last observed physical cursor position, including out-of-bounds values
    /// carried by `CursorMoved` while a button is held. Lets a drag continue
    /// when `Window::cursor_position()` masks an off-window cursor; cleared on
    /// every gesture reset and on the last release.
    pub last_cursor_phys: Option<Vec2>,
}

impl OrzmaMouseGesture {
    /// Resets the in-progress gesture: drops the held lock, the last
    /// target, and the cached cursor position. The multi-click counter is
    /// preserved so a follow-up click can still chain.
    pub fn reset(&mut self) {
        self.held = None;
        self.last_target = None;
        self.last_cursor_phys = None;
    }
}

/// Consecutive-click counter using a timeout + positional-drift gate.
#[derive(Default)]
pub(in crate::input::mouse) struct ClickTracker {
    last: Option<(Duration, Vec2, u8)>,
}

impl ClickTracker {
    /// Registers a press at `now` / logical `pos`, returning the click count
    /// (1..=3). `cfg` is `(timeout, drift_px)`.
    pub(in crate::input::mouse) fn register(
        &mut self,
        now: Duration,
        pos: Vec2,
        cfg: (Duration, f32),
    ) -> u8 {
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

/// Carries the sub-notch wheel remainder across frames, per axis, scoped to the
/// last terminal the wheel targeted.
#[derive(Resource, Default)]
pub(in crate::input::mouse) struct WheelAccumulator {
    pub residual_cells: f32,
    pub residual_cells_h: f32,
    last_target: Option<Entity>,
}

impl WheelAccumulator {
    /// Resets both residuals when the wheel target changes, so a sub-notch
    /// fraction accumulated over one terminal cannot bleed into the next.
    pub(in crate::input::mouse) fn retarget(&mut self, entity: Entity) {
        if self.last_target != Some(entity) {
            self.residual_cells = 0.0;
            self.residual_cells_h = 0.0;
            self.last_target = Some(entity);
        }
    }
}

/// Cells of scroll for one wheel event: `Line` units count directly, `Pixel`
/// units divide by the cell height. Positive = wheel-up (toward older lines).
pub(in crate::input::mouse) fn wheel_delta_cells(
    unit: MouseScrollUnit,
    y: f32,
    cell_h: f32,
) -> f32 {
    match unit {
        MouseScrollUnit::Line => y,
        MouseScrollUnit::Pixel => y / cell_h.max(1.0),
    }
}

/// Adds `delta_cells` to `residual` and returns whole notches to emit
/// (positive = up/older for the vertical axis, right for the horizontal axis),
/// carrying the remainder. Resets `residual` on a sign flip, then processes the
/// new delta at full magnitude. A zero / negative-zero delta has no direction
/// and must not trip the sign-flip reset.
pub(in crate::input::mouse) fn accumulate_notches(
    residual: &mut f32,
    delta_cells: f32,
    cells_per_notch: f32,
) -> i32 {
    if *residual != 0.0 && delta_cells != 0.0 && (*residual).signum() != delta_cells.signum() {
        *residual = 0.0;
    }
    let threshold = cells_per_notch.max(f32::EPSILON);
    *residual += delta_cells;
    let notches = (*residual / threshold).trunc() as i32;
    if notches != 0 {
        *residual -= notches as f32 * threshold;
    }
    notches
}

/// Dominant-axis lock for a single frame's `(vertical, horizontal)` cell
/// delta: keeps the axis the gesture is travelling along and zeros the
/// other, so trackpad jitter on the off-axis cannot leak a stray notch.
///
/// `ratio` is the share of the gesture's magnitude the horizontal component must
/// reach to count as horizontal: horizontal survives only when
/// `|h| / hypot(v, h) >= ratio` (with `ratio = 0.9` that is `|h| >= ~2.06·|v|`),
/// biasing toward vertical. Meaningful range is `0.0..=1.0`: `0.0` (or any
/// negative) disables the lock and `1.0` is strictest (only a pure-horizontal
/// gesture, `v == 0`, scrolls horizontally). The comparison is `>=`, not `>`, so
/// `1.0` does not silently kill all horizontal scroll. A zero horizontal delta
/// (the common pure-vertical frame) short-circuits before the `hypot`.
pub(in crate::input::mouse) fn lock_dominant_axis(
    vertical: f32,
    horizontal: f32,
    ratio: f32,
) -> (f32, f32) {
    if ratio <= 0.0 || horizontal == 0.0 {
        return (vertical, horizontal);
    }
    if horizontal.abs() / vertical.hypot(horizontal) >= ratio {
        (0.0, horizontal)
    } else {
        (vertical, 0.0)
    }
}

fn slot(button: PointerButton) -> usize {
    match button {
        PointerButton::Left => 0,
        PointerButton::Middle => 1,
        PointerButton::Right => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn click_tracker_counts_within_timeout_and_drift() {
        let mut t = ClickTracker::default();
        let cfg = (std::time::Duration::from_millis(400), 8.0f32);
        assert_eq!(
            t.register(
                std::time::Duration::from_millis(0),
                Vec2::new(10.0, 10.0),
                cfg
            ),
            1
        );
        assert_eq!(
            t.register(
                std::time::Duration::from_millis(200),
                Vec2::new(11.0, 11.0),
                cfg
            ),
            2
        );
        assert_eq!(
            t.register(
                std::time::Duration::from_millis(350),
                Vec2::new(12.0, 10.0),
                cfg
            ),
            3
        );
        assert_eq!(
            t.register(
                std::time::Duration::from_millis(900),
                Vec2::new(12.0, 10.0),
                cfg
            ),
            1
        );
    }

    /// Asserts that a freshly initialized gesture resource holds no button
    /// and no target, while its click tracker still counts a first click.
    ///
    /// Case: the plugin's resource has just been inserted at startup,
    /// before the first mouse event of the session arrives.
    #[test]
    fn mouse_gesture_resource_default_is_idle() {
        let g = OrzmaMouseGesture::default();
        assert!(g.held.is_none() && g.last_target.is_none());
        let mut t = g.click;
        let cfg = (Duration::from_millis(400), 8.0f32);
        assert_eq!(t.register(Duration::from_millis(0), Vec2::ZERO, cfg), 1);
    }

    /// Asserts that a held-pointer lock tracks each button and reports
    /// empty only once every held button is released.
    ///
    /// Case: the user presses the left button, adds the right one, and
    /// lets go of them one at a time.
    #[test]
    fn held_pointer_tracks_each_button_until_all_are_released() {
        let mut held = HeldPointer::new(Entity::PLACEHOLDER);
        held.press(PointerButton::Left);
        held.press(PointerButton::Right);
        assert!(held.holds(PointerButton::Left) && held.holds(PointerButton::Right));
        assert!(!held.holds(PointerButton::Middle));
        held.release(PointerButton::Left);
        assert!(!held.is_empty());
        held.release(PointerButton::Right);
        assert!(held.is_empty());
    }

    #[test]
    fn line_delta_is_direct_pixel_divides_by_cell_height() {
        assert_eq!(wheel_delta_cells(MouseScrollUnit::Line, 2.0, 16.0), 2.0);
        assert_eq!(wheel_delta_cells(MouseScrollUnit::Pixel, 32.0, 16.0), 2.0);
    }

    #[test]
    fn accumulator_emits_on_threshold_and_carries_remainder() {
        let mut acc = WheelAccumulator::default();
        assert_eq!(accumulate_notches(&mut acc.residual_cells, 0.3, 0.5), 0);
        assert_eq!(accumulate_notches(&mut acc.residual_cells, 0.3, 0.5), 1);
        assert_eq!(accumulate_notches(&mut acc.residual_cells, -1.0, 0.5), -2);
    }

    #[test]
    fn wheel_accumulator_resets_residual_on_target_change() {
        let mut world = World::new();
        let a = world.spawn_empty().id();
        let b = world.spawn_empty().id();
        let mut acc = WheelAccumulator::default();
        acc.retarget(a);
        assert_eq!(accumulate_notches(&mut acc.residual_cells, 0.3, 0.5), 0);
        acc.retarget(a);
        assert_eq!(
            accumulate_notches(&mut acc.residual_cells, 0.3, 0.5),
            1,
            "0.3 + 0.3 = 0.6 → one notch on the same target"
        );
        acc.retarget(b);
        assert_eq!(
            accumulate_notches(&mut acc.residual_cells, 0.3, 0.5),
            0,
            "switching target clears the carried residual"
        );
        assert_eq!(accumulate_notches(&mut acc.residual_cells_h, 0.3, 0.5), 0);
        acc.retarget(a);
        assert_eq!(
            accumulate_notches(&mut acc.residual_cells_h, 0.3, 0.5),
            0,
            "a target change must clear the carried horizontal residual too"
        );
    }

    #[test]
    fn accumulator_zero_delta_does_not_reset_residual() {
        let mut acc = WheelAccumulator::default();
        assert_eq!(accumulate_notches(&mut acc.residual_cells, 0.3, 0.5), 0);
        assert_eq!(accumulate_notches(&mut acc.residual_cells, -0.0, 0.5), 0);
        assert_eq!(accumulate_notches(&mut acc.residual_cells, 0.3, 0.5), 1);
    }

    #[test]
    fn axis_lock_keeps_dominant_axis_and_zeros_the_other() {
        assert_eq!(lock_dominant_axis(1.0, 0.2, 0.9), (1.0, 0.0));
        assert_eq!(lock_dominant_axis(0.2, 1.0, 0.9), (0.0, 1.0));
        assert_eq!(lock_dominant_axis(1.0, 0.0, 0.9), (1.0, 0.0));
        assert_eq!(lock_dominant_axis(0.0, 1.0, 0.9), (0.0, 1.0));
    }

    #[test]
    fn axis_lock_diagonal_45_degrees_resolves_to_vertical() {
        // |h|/hypot = 0.707 < 0.9, so a 45° gesture collapses to vertical.
        let (v, h) = lock_dominant_axis(1.0, 1.0, 0.9);
        assert_eq!((v, h), (1.0, 0.0));
    }

    #[test]
    fn axis_lock_passes_zero_zero_through() {
        assert_eq!(lock_dominant_axis(0.0, 0.0, 0.9), (0.0, 0.0));
    }

    #[test]
    fn axis_lock_ratio_zero_disables_the_lock() {
        // A diagonal that would otherwise collapse to vertical keeps both axes.
        assert_eq!(lock_dominant_axis(1.0, 0.5, 0.0), (1.0, 0.5));
    }

    #[test]
    fn axis_lock_ratio_one_still_allows_pure_horizontal() {
        // Strictest setting: a pure-horizontal gesture must still scroll
        // horizontally (|h|/hypot == 1.0 >= 1.0), and any vertical component
        // collapses to vertical.
        assert_eq!(lock_dominant_axis(0.0, 1.0, 1.0), (0.0, 1.0));
        assert_eq!(lock_dominant_axis(0.01, 1.0, 1.0), (0.01, 0.0));
    }
}
