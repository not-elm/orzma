//! Terminal font-size zoom: the factor ladder a zoom shortcut steps through
//! and the observer that applies one step.

use crate::configs::OrzmaConfigsResource;
use bevy::prelude::*;
use bevy_orzma_tty_renderer::prelude::TerminalFontSize;
use orzma_configs::shortcuts::FontSizeStep;

/// The zoom factors, in ascending order. `FACTORS[BASE]` is the unzoomed 1.0.
const FACTORS: [f32; 12] = [
    0.5, 0.67, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0,
];

/// The index of the unzoomed factor in [`FACTORS`].
const BASE: usize = 4;

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

    /// Moves the current step to `index`.
    pub fn set_index(&mut self, index: usize) {
        self.index = index;
    }

    /// Returns the index one rung away in `direction`, or `None` when the
    /// ladder has no rung left that way.
    ///
    /// A `Reset` returns `BASE`, or `None` when the current step is already
    /// `BASE`.
    pub fn next_index(&self, direction: FontSizeStep) -> Option<usize> {
        let index = match direction {
            FontSizeStep::Reset => BASE,
            FontSizeStep::Increase => self.index.checked_add(1)?,
            FontSizeStep::Decrease => self.index.checked_sub(1)?,
        };
        (index != self.index && index < FACTORS.len()).then_some(index)
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
    configs: Res<OrzmaConfigsResource>,
) {
    let Some(index) = zoom.next_index(ev.direction) else {
        return;
    };
    zoom.set_index(index);
    // NOTE: every rung carries a distinct factor, so this write always changes
    // the value. Writing unconditionally here is what makes change detection
    // fire exactly on a zoom step.
    *font_size = TerminalFontSize(configs.font.size * zoom.factor());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts that an increase moves to the adjacent larger rung.
    ///
    /// Case: the user presses the zoom-in key once from the unzoomed state.
    #[test]
    fn an_increase_advances_one_rung() {
        let zoom = FontZoom::default();
        assert_eq!(zoom.next_index(FontSizeStep::Increase), Some(BASE + 1));
    }

    /// Asserts that a decrease moves to the adjacent smaller rung.
    ///
    /// Case: the user presses the zoom-out key once from the unzoomed state.
    #[test]
    fn a_decrease_retreats_one_rung() {
        let zoom = FontZoom::default();
        assert_eq!(zoom.next_index(FontSizeStep::Decrease), Some(BASE - 1));
    }

    /// Asserts that an increase at the top of the ladder is ignored rather
    /// than clamped to the top rung again.
    ///
    /// Case: the user holds the zoom-in key past the largest factor.
    #[test]
    fn an_increase_at_the_top_returns_none() {
        let mut zoom = FontZoom::default();
        zoom.set_index(FACTORS.len() - 1);
        assert_eq!(zoom.next_index(FontSizeStep::Increase), None);
    }

    /// Asserts that a decrease at the bottom of the ladder is ignored.
    ///
    /// Case: the user holds the zoom-out key past the smallest factor.
    #[test]
    fn a_decrease_at_the_bottom_returns_none() {
        let mut zoom = FontZoom::default();
        zoom.set_index(0);
        assert_eq!(zoom.next_index(FontSizeStep::Decrease), None);
    }

    /// Asserts that a reset returns the unzoomed rung regardless of how far
    /// the ladder was stepped.
    ///
    /// Case: the user zooms in several times and then presses the reset key.
    #[test]
    fn a_reset_returns_the_base_rung() {
        let mut zoom = FontZoom::default();
        zoom.set_index(FACTORS.len() - 1);
        assert_eq!(zoom.next_index(FontSizeStep::Reset), Some(BASE));
    }

    /// Asserts that a reset from the unzoomed rung is ignored rather than
    /// rewriting the same value.
    ///
    /// Case: the user presses the reset key without having zoomed.
    #[test]
    fn a_reset_at_the_base_rung_returns_none() {
        let zoom = FontZoom::default();
        assert_eq!(zoom.next_index(FontSizeStep::Reset), None);
    }

    use bevy::window::{PrimaryWindow, WindowResolution};

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
