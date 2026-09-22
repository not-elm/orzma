//! Terminal font-size zoom: the factor ladder a zoom shortcut steps through
//! and the observer that applies one step.

use crate::configs::OrzmaConfigsResource;
use crate::surface::geometry::cell_pitch_phys;
use bevy::ecs::system::NonSendMarker;
use bevy::prelude::*;
use bevy::window::{Monitor, OnMonitor, PrimaryWindow, WindowMode};
use bevy::winit::WINIT_WINDOWS;
use bevy_orzma_tty_renderer::{TerminalFontSize, TerminalFonts, physical_font_size};

/// The zoom factors, in ascending order. `FACTORS[BASE]` is the unzoomed 1.0.
const FACTORS: [f32; 12] = [
    0.5, 0.67, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0,
];

/// The index of the unzoomed factor in [`FACTORS`].
const BASE: usize = 4;

/// Which way a zoom step moves along the factor ladder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ZoomDirection {
    /// Step to the next larger factor.
    Increase,
    /// Step to the next smaller factor.
    Decrease,
    /// Return to the unzoomed factor.
    Reset,
}

/// The host asks for one zoom step on the terminal font size.
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct FontZoomAction {
    /// Which way to step.
    pub direction: ZoomDirection,
}

/// The current zoom step, as an index into the factor ladder.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FontZoom {
    index: usize,
}

impl Default for FontZoom {
    fn default() -> Self {
        Self { index: BASE }
    }
}

impl FontZoom {
    /// The factor the current step multiplies the configured font size by.
    pub fn factor(&self) -> f32 {
        FACTORS.get(self.index).copied().unwrap_or(1.0)
    }

    /// The current step's position on the ladder.
    #[cfg(test)]
    pub fn index(&self) -> usize {
        self.index
    }

    /// Moves the current step to `index`.
    pub fn set_index(&mut self, index: usize) {
        self.index = index;
    }

    /// Returns the next index in `direction` whose cell pitch differs from
    /// `current_pitch`, paired with that index's pitch, or `None` when the
    /// ladder has no such index left.
    ///
    /// `base_size` is the configured `[font] size`; the logical size at a rung
    /// is `base_size * FACTORS[rung]`. `pitch_at` maps a logical size to the
    /// whole-physical-pixel cell pitch the renderer would paint at.
    /// `current_pitch` is the pitch at the current rung. A rung whose pitch
    /// matches it is skipped, so an accepted step always changes the grid.
    pub fn next_index(
        &self,
        direction: ZoomDirection,
        base_size: f32,
        current_pitch: (u16, u16),
        pitch_at: impl Fn(f32) -> (u16, u16),
    ) -> Option<(usize, (u16, u16))> {
        let step: isize = match direction {
            ZoomDirection::Reset => {
                if self.index == BASE {
                    return None;
                }
                let factor = *FACTORS.get(BASE)?;
                return Some((BASE, pitch_at(base_size * factor)));
            }
            ZoomDirection::Increase => 1,
            ZoomDirection::Decrease => -1,
        };
        let mut index = self.index;
        loop {
            index = index.checked_add_signed(step)?;
            let factor = *FACTORS.get(index)?;
            let pitch = pitch_at(base_size * factor);
            if pitch != current_pitch {
                return Some((index, pitch));
            }
        }
    }
}

/// Adds the font-size zoom pipeline.
pub(crate) struct FontZoomPlugin;

impl Plugin for FontZoomPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FontZoom>().add_observer(on_font_zoom);
    }
}

fn on_font_zoom(
    ev: On<FontZoomAction>,
    mut zoom: ResMut<FontZoom>,
    mut font_size: ResMut<TerminalFontSize>,
    mut windows: Query<(Entity, &mut Window, Option<&OnMonitor>), With<PrimaryWindow>>,
    configs: Res<OrzmaConfigsResource>,
    fonts: Res<TerminalFonts>,
    monitors: Query<&Monitor>,
    _non_send_marker: NonSendMarker,
) {
    let Ok((entity, mut window, on_monitor)) = windows.single_mut() else {
        return;
    };
    let base = configs.font.size;
    let dpr = window.scale_factor();
    let pitch_at = |logical: f32| {
        let (w, h) = cell_pitch_phys(&fonts.cell_metrics_px(physical_font_size(logical, dpr)));
        (w as u16, h as u16)
    };
    let old_pitch = pitch_at(base * zoom.factor());
    let Some((index, new_pitch)) = zoom.next_index(ev.direction, base, old_pitch, pitch_at) else {
        return;
    };
    zoom.set_index(index);
    // NOTE: every rung carries a distinct factor, so this write always changes
    // the value. Writing unconditionally here is what makes change detection
    // fire exactly on a zoom step.
    *font_size = TerminalFontSize(base * zoom.factor());

    if !configs.font.zoom_resizes_window || !may_resize(entity, &window) {
        return;
    }
    let current = UVec2::new(
        window.resolution.physical_width(),
        window.resolution.physical_height(),
    );
    let monitor = on_monitor
        .and_then(|on| monitors.get(on.0).ok())
        .map(|m| UVec2::new(m.physical_width, m.physical_height));
    let Some(want) = requested_window_size(current, old_pitch, new_pitch, monitor) else {
        return;
    };
    window.resolution.set_physical_resolution(want.x, want.y);
}

/// Whether the window may be resized to preserve the grid.
///
/// A fullscreen or maximized window is left alone. A window winit does not
/// know about yet is treated as resizable.
fn may_resize(entity: Entity, window: &Window) -> bool {
    if window.mode != WindowMode::Windowed {
        return false;
    }
    // NOTE: a maximized window must be left alone. winit's Windows backend
    // clears the maximized flag instead of refusing the resize, so dropping
    // this check turns a zoom keypress into an un-maximize.
    WINIT_WINDOWS.with_borrow(|winit_windows| {
        winit_windows
            .get_window(entity)
            .is_none_or(|w| !w.is_maximized())
    })
}

/// The physical window size that keeps the cell count `current` shows at
/// `old_pitch` once the pitch becomes `new_pitch`, clamped to `monitor`.
///
/// Both pitches are whole physical pixels; a zero on either axis is treated as
/// one, so the division never divides by zero. `monitor` is the display's
/// physical size, and `None` leaves the request unclamped.
///
/// Returns `None` when `current` holds no whole cell on either axis, since
/// there is no cell count to preserve.
fn requested_window_size(
    current: UVec2,
    old_pitch: (u16, u16),
    new_pitch: (u16, u16),
    monitor: Option<UVec2>,
) -> Option<UVec2> {
    let old = UVec2::new(u32::from(old_pitch.0).max(1), u32::from(old_pitch.1).max(1));
    let new = UVec2::new(u32::from(new_pitch.0).max(1), u32::from(new_pitch.1).max(1));
    let cells = current / old;
    if cells.x == 0 || cells.y == 0 {
        return None;
    }
    let want = cells * new;
    Some(match monitor {
        // NOTE: neither bevy_window::Monitor nor winit 0.30 exposes the work
        // area, so the taskbar and dock are not excluded. The margin keeps a
        // maximal request off the very edge of the display.
        Some(monitor) => want.min(monitor * 95 / 100),
        None => want,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `pitch_at` stand-in that reports a cell pitch proportional to the
    /// logical size, so every rung of the ladder resolves to a distinct pitch.
    fn distinct_pitch(logical: f32) -> (u16, u16) {
        let w = (logical * 10.0) as u16;
        (w, w * 2)
    }

    /// A `pitch_at` stand-in that reports one pitch for every logical size, so
    /// no rung ever changes the grid.
    fn constant_pitch(_logical: f32) -> (u16, u16) {
        (7, 15)
    }

    /// Asserts that an increase moves to the adjacent larger rung, and reports
    /// that rung's cell pitch, when the rung changes the pitch.
    ///
    /// Case: the user presses the zoom-in key once from the unzoomed state.
    #[test]
    fn an_increase_advances_one_rung() {
        let zoom = FontZoom::default();
        let current = distinct_pitch(11.25);
        assert_eq!(
            zoom.next_index(ZoomDirection::Increase, 11.25, current, distinct_pitch),
            Some((5, distinct_pitch(11.25 * 1.1)))
        );
    }

    /// Asserts that a decrease moves to the adjacent smaller rung, and reports
    /// that rung's cell pitch.
    ///
    /// Case: the user presses the zoom-out key once from the unzoomed state.
    #[test]
    fn a_decrease_retreats_one_rung() {
        let zoom = FontZoom::default();
        let current = distinct_pitch(11.25);
        assert_eq!(
            zoom.next_index(ZoomDirection::Decrease, 11.25, current, distinct_pitch),
            Some((3, distinct_pitch(11.25 * 0.9)))
        );
    }

    /// Asserts that rungs whose cell pitch matches the current one are skipped
    /// rather than accepted, so a keypress never leaves the grid unchanged.
    ///
    /// Case: at a small configured font size on a low-DPI display, adjacent
    /// factors round to the same whole-pixel cell width.
    #[test]
    fn a_rung_with_an_unchanged_cell_pitch_is_skipped() {
        let zoom = FontZoom::default();
        assert_eq!(
            zoom.next_index(ZoomDirection::Increase, 11.25, (7, 15), constant_pitch),
            None,
            "every rung reports the same pitch, so the ladder runs out"
        );
    }

    /// Asserts that an increase at the top of the ladder is ignored rather
    /// than clamped to the top rung again.
    ///
    /// Case: the user holds the zoom-in key past the largest factor.
    #[test]
    fn an_increase_at_the_top_returns_none() {
        let mut zoom = FontZoom::default();
        zoom.set_index(FACTORS.len() - 1);
        let current = distinct_pitch(11.25 * zoom.factor());
        assert_eq!(
            zoom.next_index(ZoomDirection::Increase, 11.25, current, distinct_pitch),
            None
        );
    }

    /// Asserts that a decrease at the bottom of the ladder is ignored.
    ///
    /// Case: the user holds the zoom-out key past the smallest factor.
    #[test]
    fn a_decrease_at_the_bottom_returns_none() {
        let mut zoom = FontZoom::default();
        zoom.set_index(0);
        let current = distinct_pitch(11.25 * zoom.factor());
        assert_eq!(
            zoom.next_index(ZoomDirection::Decrease, 11.25, current, distinct_pitch),
            None
        );
    }

    /// Asserts that a reset returns the unzoomed rung regardless of how far
    /// the ladder was stepped.
    ///
    /// Case: the user zooms in several times and then presses the reset key.
    #[test]
    fn a_reset_returns_the_base_rung() {
        let mut zoom = FontZoom::default();
        zoom.set_index(FACTORS.len() - 1);
        let current = distinct_pitch(11.25 * zoom.factor());
        assert_eq!(
            zoom.next_index(ZoomDirection::Reset, 11.25, current, distinct_pitch),
            Some((BASE, distinct_pitch(11.25)))
        );
    }

    /// Asserts that a reset from the unzoomed rung is ignored rather than
    /// rewriting the same value.
    ///
    /// Case: the user presses the reset key without having zoomed.
    #[test]
    fn a_reset_at_the_base_rung_returns_none() {
        let zoom = FontZoom::default();
        let current = distinct_pitch(11.25);
        assert_eq!(
            zoom.next_index(ZoomDirection::Reset, 11.25, current, distinct_pitch),
            None
        );
    }

    /// Asserts that walking the shipped ladder upward with the bundled font
    /// takes at least one step and never reports a narrower cell than the rung
    /// before it.
    ///
    /// Case: a user on a 2x display with the default configuration holds the
    /// zoom-in key.
    #[test]
    fn the_shipped_ladder_widens_monotonically() {
        let fonts = TerminalFonts::default();
        let pitch_at = |logical: f32| {
            let (w, h) = cell_pitch_phys(&fonts.cell_metrics_px(physical_font_size(logical, 2.0)));
            (w as u16, h as u16)
        };

        let mut zoom = FontZoom::default();
        let mut current = pitch_at(11.25 * zoom.factor());
        let mut steps = 0;
        while let Some((index, pitch)) =
            zoom.next_index(ZoomDirection::Increase, 11.25, current, pitch_at)
        {
            assert!(
                pitch.0 >= current.0 && pitch.1 >= current.1,
                "rung {index} reports a narrower cell than the rung before it"
            );
            assert_ne!(pitch, current, "an accepted rung must change the pitch");
            zoom.set_index(index);
            current = pitch;
            steps += 1;
        }
        assert!(steps > 0, "the ladder must allow at least one zoom-in step");
        assert!(zoom.index() > BASE);
    }

    /// Asserts that the requested window size keeps the cell count the window
    /// currently shows, so the grid does not shrink under the larger pitch.
    ///
    /// Case: a 1600x1000 window at a 13x30 cell pitch zooms to a 16x36 pitch.
    #[test]
    fn the_requested_size_preserves_the_cell_count() {
        let want = requested_window_size(
            UVec2::new(1600, 1000),
            (13, 30),
            (16, 36),
            Some(UVec2::new(4000, 3000)),
        );

        // 1600 / 13 = 123 columns; 1000 / 30 = 33 rows.
        assert_eq!(want, Some(UVec2::new(123 * 16, 33 * 36)));
    }

    /// Asserts that a request larger than the monitor is clamped, so the
    /// window never asks for a size the display cannot show.
    ///
    /// Case: the user zooms in on a window that already fills most of a
    /// 1920x1080 display.
    #[test]
    fn the_requested_size_is_clamped_to_the_monitor() {
        let want = requested_window_size(
            UVec2::new(1600, 1000),
            (13, 30),
            (16, 36),
            Some(UVec2::new(1920, 1080)),
        );

        assert_eq!(want, Some(UVec2::new(1920 * 95 / 100, 1080 * 95 / 100)));
    }

    /// Asserts that an unknown monitor leaves the request unclamped rather
    /// than falling back to a guess.
    ///
    /// Case: the window has just been created and has not been assigned to a
    /// monitor yet.
    #[test]
    fn an_unknown_monitor_leaves_the_request_unclamped() {
        let want = requested_window_size(UVec2::new(800, 600), (10, 20), (20, 40), None);

        assert_eq!(want, Some(UVec2::new(80 * 20, 30 * 40)));
    }

    /// Asserts that a degenerate pitch is treated as one pixel rather than
    /// dividing by zero.
    ///
    /// Case: a font whose metrics floor to zero on one axis.
    #[test]
    fn a_zero_pitch_is_treated_as_one_pixel() {
        let want = requested_window_size(UVec2::new(100, 100), (0, 0), (2, 2), None);

        assert_eq!(want, Some(UVec2::new(200, 200)));
    }

    /// Asserts that a window holding no whole cell asks for nothing rather
    /// than collapsing to a one-pixel request.
    ///
    /// Case: the user minimizes the orzma window on Windows, which reports a
    /// 0x0 client area, and then presses a zoom key.
    #[test]
    fn a_window_with_no_cells_requests_nothing() {
        assert_eq!(
            requested_window_size(UVec2::ZERO, (13, 30), (16, 36), None),
            None
        );
        assert_eq!(
            requested_window_size(UVec2::new(1600, 10), (13, 30), (16, 36), None),
            None,
            "one axis short of a whole cell is enough to decline"
        );
    }

    use bevy::window::{MonitorSelection, WindowResolution};
    use orzma_configs::OrzmaConfigs;
    use orzma_configs::font::FontConfig;

    /// Builds an app with the zoom observer, one primary window of the given
    /// physical size, and a config whose `zoom_resizes_window` is `resizes`.
    fn zoom_app(resizes: bool, width: u32, height: u32) -> (App, Entity) {
        let mut app = App::new();
        // `OrzmaConfigsResource` derives `Deref` but not `DerefMut`, so the
        // inner config is built up front rather than mutated in place.
        let configs = OrzmaConfigsResource(OrzmaConfigs {
            font: FontConfig {
                zoom_resizes_window: resizes,
                ..FontConfig::default()
            },
            ..OrzmaConfigs::default()
        });
        app.add_plugins(MinimalPlugins)
            .add_plugins(FontZoomPlugin)
            .init_resource::<TerminalFonts>()
            .init_resource::<TerminalFontSize>()
            .insert_resource(configs);
        let mut window = Window {
            resolution: WindowResolution::new(width, height),
            ..default()
        };
        window.resolution.set_scale_factor(1.0);
        let entity = app.world_mut().spawn((window, PrimaryWindow)).id();
        (app, entity)
    }

    /// Asserts that a zoom step raises the terminal font size above the
    /// configured base.
    ///
    /// Case: the user presses the zoom-in key in a normal window.
    #[test]
    fn a_zoom_step_raises_the_terminal_font_size() {
        let (mut app, _) = zoom_app(false, 1600, 1000);
        let before = app.world().resource::<TerminalFontSize>().0;

        app.world_mut().trigger(FontZoomAction {
            direction: ZoomDirection::Increase,
        });
        app.update();

        assert!(app.world().resource::<TerminalFontSize>().0 > before);
    }

    /// Asserts that the window is left untouched when the config disables the
    /// resize, so the cell count changes instead.
    ///
    /// Case: a tiling window manager user sets `zoom_resizes_window = false`.
    #[test]
    fn the_window_is_untouched_when_the_config_disables_the_resize() {
        let (mut app, entity) = zoom_app(false, 1600, 1000);

        app.world_mut().trigger(FontZoomAction {
            direction: ZoomDirection::Increase,
        });
        app.update();

        let window = app
            .world()
            .get::<Window>(entity)
            .expect("the primary window");
        assert_eq!(window.resolution.physical_width(), 1600);
        assert_eq!(window.resolution.physical_height(), 1000);
    }

    /// Asserts that a zoom step grows the window when the config allows it,
    /// so the grid keeps its cell count.
    ///
    /// Case: the user presses the zoom-in key in a normal floating window.
    #[test]
    fn a_zoom_step_grows_the_window_when_enabled() {
        let (mut app, entity) = zoom_app(true, 1600, 1000);

        app.world_mut().trigger(FontZoomAction {
            direction: ZoomDirection::Increase,
        });
        app.update();

        let window = app
            .world()
            .get::<Window>(entity)
            .expect("the primary window");
        assert!(window.resolution.physical_width() > 1600);
        assert!(window.resolution.physical_height() > 1000);
    }

    /// Asserts that a fullscreen window is left untouched.
    ///
    /// Case: the user zooms while orzma is in borderless fullscreen.
    #[test]
    fn a_fullscreen_window_is_untouched() {
        let (mut app, entity) = zoom_app(true, 1600, 1000);
        app.world_mut()
            .get_mut::<Window>(entity)
            .expect("the primary window")
            .mode = WindowMode::BorderlessFullscreen(MonitorSelection::Current);

        app.world_mut().trigger(FontZoomAction {
            direction: ZoomDirection::Increase,
        });
        app.update();

        let window = app
            .world()
            .get::<Window>(entity)
            .expect("the primary window");
        assert_eq!(window.resolution.physical_width(), 1600);
    }

    /// Asserts that a window reporting no client area is left untouched
    /// rather than being asked for a collapsed size.
    ///
    /// Case: the user minimizes the orzma window on Windows and presses a
    /// zoom key.
    #[test]
    fn a_minimized_window_is_untouched() {
        let (mut app, entity) = zoom_app(true, 0, 0);

        app.world_mut().trigger(FontZoomAction {
            direction: ZoomDirection::Increase,
        });
        app.update();

        let window = app
            .world()
            .get::<Window>(entity)
            .expect("the primary window");
        assert_eq!(window.resolution.physical_width(), 0);
        assert_eq!(window.resolution.physical_height(), 0);
    }
}
