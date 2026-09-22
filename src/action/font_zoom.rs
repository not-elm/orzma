//! Terminal font-size zoom: the factor ladder a zoom shortcut steps through
//! and the observer that applies one step.

use crate::configs::OrzmaConfigsResource;
use crate::surface::geometry::cell_pitch_phys;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy_orzma_tty_renderer::{TerminalFontSize, TerminalFonts, physical_font_size};
use orzma_configs::shortcuts::FontSizeStep;

/// The zoom factors, in ascending order. `FACTORS[BASE]` is the unzoomed 1.0.
const FACTORS: [f32; 12] = [
    0.5, 0.67, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0,
];

/// The index of the unzoomed factor in [`FACTORS`].
const BASE: usize = 4;

/// The unzoomed factor.
const BASE_FACTOR: f32 = FACTORS[BASE];

/// The host asks for one zoom step on the terminal font size.
#[derive(Event, Debug, Clone, Copy)]
pub(crate) struct FontZoomAction {
    /// Which way to step.
    pub direction: FontSizeStep,
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

    /// Returns the next index in `direction` paired with that index's cell
    /// pitch, or `None` when the ladder has no such index left.
    ///
    /// `base_size` is the configured `[font] size`; the logical size at a rung
    /// is `base_size * FACTORS[rung]`. `pitch_at` maps a logical size to the
    /// whole-physical-pixel cell pitch the renderer would paint at.
    /// `current_pitch` is the pitch at the current rung. An `Increase` or
    /// `Decrease` skips any rung whose pitch matches it; a `Reset` returns
    /// `BASE` without comparing pitches.
    pub fn next_index(
        &self,
        direction: FontSizeStep,
        base_size: f32,
        current_pitch: (u16, u16),
        pitch_at: impl Fn(f32) -> (u16, u16),
    ) -> Option<(usize, (u16, u16))> {
        let step: isize = match direction {
            FontSizeStep::Reset => {
                if self.index == BASE {
                    return None;
                }
                return Some((BASE, pitch_at(base_size * BASE_FACTOR)));
            }
            FontSizeStep::Increase => 1,
            FontSizeStep::Decrease => -1,
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
    windows: Query<&Window, With<PrimaryWindow>>,
    configs: Res<OrzmaConfigsResource>,
    fonts: Res<TerminalFonts>,
) {
    let Ok(window) = windows.single() else {
        return;
    };
    let base = configs.font.size;
    let dpr = window.scale_factor();
    let pitch_at = |logical: f32| {
        let (w, h) = cell_pitch_phys(&fonts.cell_metrics_px(physical_font_size(logical, dpr)));
        (w as u16, h as u16)
    };
    let old_pitch = pitch_at(font_size.0);
    let Some((index, _)) = zoom.next_index(ev.direction, base, old_pitch, pitch_at) else {
        return;
    };
    zoom.set_index(index);
    // NOTE: every rung carries a distinct factor, so this write always changes
    // the value. Writing unconditionally here is what makes change detection
    // fire exactly on a zoom step.
    *font_size = TerminalFontSize(base * zoom.factor());
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
            zoom.next_index(FontSizeStep::Increase, 11.25, current, distinct_pitch),
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
            zoom.next_index(FontSizeStep::Decrease, 11.25, current, distinct_pitch),
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
            zoom.next_index(FontSizeStep::Increase, 11.25, (7, 15), constant_pitch),
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
            zoom.next_index(FontSizeStep::Increase, 11.25, current, distinct_pitch),
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
            zoom.next_index(FontSizeStep::Decrease, 11.25, current, distinct_pitch),
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
            zoom.next_index(FontSizeStep::Reset, 11.25, current, distinct_pitch),
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
            zoom.next_index(FontSizeStep::Reset, 11.25, current, distinct_pitch),
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
            zoom.next_index(FontSizeStep::Increase, 11.25, current, pitch_at)
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

    use bevy::window::WindowResolution;

    /// Builds an app with the zoom observer and one primary window of the
    /// given physical size. `TerminalFontSize` starts at the configured
    /// `[font] size`, matching what the startup font bridge leaves it at in
    /// the real app.
    fn zoom_app(width: u32, height: u32) -> (App, Entity) {
        let mut app = App::new();
        let configs = OrzmaConfigsResource::default();
        let base_size = configs.font.size;
        app.add_plugins(MinimalPlugins)
            .add_plugins(FontZoomPlugin)
            .init_resource::<TerminalFonts>()
            .insert_resource(TerminalFontSize(base_size))
            .insert_resource(configs);
        let mut window = Window {
            resolution: WindowResolution::new(width, height),
            ..default()
        };
        window.resolution.set_scale_factor(1.0);
        let entity = app.world_mut().spawn((window, PrimaryWindow)).id();
        (app, entity)
    }

    /// Triggers one zoom-in step.
    fn zoom_in(app: &mut App) {
        app.world_mut().trigger(FontZoomAction {
            direction: FontSizeStep::Increase,
        });
        app.update();
    }

    /// Asserts that a zoom step raises the terminal font size above the
    /// configured base.
    ///
    /// Case: the user presses the zoom-in key in a normal window.
    #[test]
    fn a_zoom_step_raises_the_terminal_font_size() {
        let (mut app, _) = zoom_app(1600, 1000);
        let before = app.world().resource::<TerminalFontSize>().0;

        zoom_in(&mut app);

        assert!(app.world().resource::<TerminalFontSize>().0 > before);
    }

    /// Asserts that a zoom step leaves the window size untouched, so the
    /// column and row counts change instead.
    ///
    /// Case: the user presses the zoom-in key in a floating window.
    #[test]
    fn a_zoom_step_leaves_the_window_size_untouched() {
        let (mut app, entity) = zoom_app(1600, 1000);

        zoom_in(&mut app);

        let window = app
            .world()
            .get::<Window>(entity)
            .expect("the primary window");
        assert_eq!(
            UVec2::new(
                window.resolution.physical_width(),
                window.resolution.physical_height(),
            ),
            UVec2::new(1600, 1000)
        );
    }
}
