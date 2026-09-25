//! Terminal fonts: the faces the grid draws with and their cell metrics,
//! kept current with the font size and the window's scale factor.

use crate::system_set::MaterialStage;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

mod faces;
mod metrics;

pub use faces::{FontFace, TerminalFonts};
pub use metrics::{CellMetrics, TerminalCellMetricsResource};

/// The physical pixel font size the renderer rasterizes at for a logical
/// size under the given scale factor.
///
/// A product that rounds below one is raised to one, and a product that is
/// not a number reports one rather than a zero-pixel face.
pub fn physical_font_size(logical_px: f32, scale_factor: f32) -> u16 {
    // NOTE: `f32::clamp` propagates NaN and `as u16` then saturates it to 0,
    // which would hand the atlas a zero-pixel face. `max` returns the
    // non-NaN operand, so it must come first.
    (logical_px * scale_factor)
        .round()
        .max(1.0)
        .min(f32::from(u16::MAX)) as u16
}

/// Logical (CSS) pixel font size for the terminal grid. Multiplied by the
/// PrimaryWindow's `scale_factor` to obtain the physical pixel size fed to
/// `cell_metrics_px` and the glyph atlas.
///
/// Defaults to 12.0.
#[derive(Resource, Clone, Copy, Debug)]
pub struct TerminalFontSize(pub f32);

impl Default for TerminalFontSize {
    fn default() -> Self {
        Self(FONT_SIZE_PX)
    }
}

/// Label to order systems against the renderer's cell-metrics
/// initialization. A plugin that mutates `TerminalFonts` before metrics
/// are computed must run its Startup systems
/// `.before(TerminalFontInitSet::InitCellMetrics)`.
#[derive(SystemSet, Debug, Clone, Eq, PartialEq, Hash)]
pub enum TerminalFontInitSet {
    /// Holds the Startup system that computes the initial cell metrics
    /// from `TerminalFonts`.
    InitCellMetrics,
}

/// Provides the terminal fonts and keeps the shared cell metrics current
/// with the font size and the primary window's scale factor.
///
/// A `TerminalFonts` resource inserted before the plugin is built is kept;
/// otherwise the bundled fonts are inserted. The cell metrics are inserted
/// only if a single primary window exists at the app's first update;
/// otherwise the plugin never inserts them.
#[derive(Default)]
pub struct TerminalFontPlugin;

impl Plugin for TerminalFontPlugin {
    fn build(&self, app: &mut App) {
        if !app.world().contains_resource::<TerminalFonts>() {
            app.insert_resource(TerminalFonts::default());
        }
        app.init_resource::<TerminalFontSize>()
            .add_systems(
                Startup,
                init_cell_metrics_from_primary_window.in_set(TerminalFontInitSet::InitCellMetrics),
            )
            // NOTE: Both runs are needed. The PreUpdate run lets `Update`
            // systems size the grid from the new metrics in the update that
            // delivers a scale factor change, and the PostUpdate run applies a
            // font size that `Update` changed before that update's upload.
            .add_systems(
                PreUpdate,
                update_cell_metrics.run_if(resource_exists::<TerminalCellMetricsResource>),
            )
            .add_systems(
                PostUpdate,
                update_cell_metrics
                    .in_set(MaterialStage::Metrics)
                    .run_if(resource_exists::<TerminalCellMetricsResource>),
            );
    }
}

const FONT_SIZE_PX: f32 = 12.0;

/// Inserts `TerminalCellMetricsResource` from the PrimaryWindow's
/// scale_factor and `TerminalFontSize`. The very first metrics already
/// carry the OS-reported scale factor, not a DPR of 1.0.
///
/// The system runs only while exactly one primary window exists; without
/// one (under `MinimalPlugins`, say) it is skipped.
fn init_cell_metrics_from_primary_window(
    mut commands: Commands,
    fonts: Res<TerminalFonts>,
    font_size: Res<TerminalFontSize>,
    window: Single<&Window, With<PrimaryWindow>>,
) {
    let phys_font_size = physical_font_size(font_size.0, window.scale_factor());
    commands.insert_resource(TerminalCellMetricsResource::new(&fonts, phys_font_size));
}

/// Rewrites `TerminalCellMetricsResource` when the physical font size that
/// `TerminalFontSize` and the primary window's scale factor give differs
/// from the one the metrics were measured at.
fn update_cell_metrics(
    mut metrics: ResMut<TerminalCellMetricsResource>,
    fonts: Res<TerminalFonts>,
    font_size: Res<TerminalFontSize>,
    window: Single<&Window, With<PrimaryWindow>>,
) {
    let phys_font_size = physical_font_size(font_size.0, window.scale_factor());
    if metrics.phys_font_size != phys_font_size {
        *metrics = TerminalCellMetricsResource::new(&fonts, phys_font_size);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ab_glyph::Font as _;

    /// Asserts that `TerminalFontPlugin::build` keeps an already-present
    /// `TerminalFonts` resource rather than overwriting it.
    ///
    /// Case: the app inserts a config-driven font override before adding
    /// `TerminalFontPlugin`.
    #[test]
    fn terminal_font_plugin_preserves_pre_inserted_terminal_fonts() {
        use bevy::window::{PrimaryWindow, Window, WindowResolution};

        // Build a non-default TerminalFonts via from_bytes — same TTF for
        // all four faces (legal for a smoke test; the labels are advisory).
        let bytes: Vec<u8> = crate::bundled::REGULAR.to_vec();
        let custom = TerminalFonts::from_bytes(bytes.clone(), bytes.clone(), bytes.clone(), bytes)
            .expect("from_bytes accepts JBM regular for all four faces");

        // Use a sentinel: pre-insert THIS specific instance, then check
        // that the bytes pointer hasn't changed after Plugin::build.
        let pre_inserted_bytes_ptr = custom.regular.font_data().as_ptr();

        let mut app = App::new();
        let mut window = Window {
            resolution: WindowResolution::new(800, 600),
            ..default()
        };
        window.resolution.set_scale_factor(1.0);
        app.world_mut().spawn((window, PrimaryWindow));

        app.insert_resource(custom);
        app.add_plugins(TerminalFontPlugin);
        app.update();

        let fonts = app.world().resource::<TerminalFonts>();
        assert_eq!(
            fonts.regular.font_data().as_ptr(),
            pre_inserted_bytes_ptr,
            "TerminalFonts was overwritten by Plugin::build, but the resource was \
             already present at add_plugins time — the pre-insert should have been preserved"
        );
    }

    #[test]
    fn init_cell_metrics_honors_terminal_font_size_resource() {
        use bevy::window::{PrimaryWindow, Window, WindowResolution};

        let mut app = App::new();
        let mut window = Window {
            resolution: WindowResolution::new(800, 600),
            ..default()
        };
        window.resolution.set_scale_factor(1.0);
        app.world_mut().spawn((window, PrimaryWindow));
        app.insert_resource(TerminalFontSize(10.0));
        app.add_plugins(TerminalFontPlugin);
        app.update();

        let res = app
            .world()
            .get_resource::<TerminalCellMetricsResource>()
            .expect("Startup system should insert TerminalCellMetricsResource");
        assert_eq!(
            res.phys_font_size, 10,
            "phys_font_size must follow TerminalFontSize (10.0) at DPR 1.0"
        );
    }

    /// Asserts that the inserted `TerminalCellMetricsResource` scales both
    /// `phys_font_size` and the metrics derived from it by the
    /// PrimaryWindow's scale_factor.
    ///
    /// Case: the app starts on a Retina display reporting a scale factor
    /// of 2.
    #[test]
    fn init_cell_metrics_from_primary_window_uses_window_scale_factor() {
        use bevy::window::{PrimaryWindow, Window, WindowResolution};

        let mut app = App::new();
        // NOTE: PrimaryWindow must be spawned BEFORE `app.update()` — the
        // Startup system uses `Single<&Window, With<PrimaryWindow>>` which
        // skips the system when zero entities match. If we spawned after
        // update, the resource would never be inserted and the assertion
        // below would panic with "should have inserted" — a vacuous pass
        // disguised as a failure-mode test.
        let mut window = Window {
            resolution: WindowResolution::new(800, 600),
            ..default()
        };
        window.resolution.set_scale_factor(2.0);
        app.world_mut().spawn((window, PrimaryWindow));

        app.add_plugins(TerminalFontPlugin);
        app.update();

        let res = app
            .world()
            .get_resource::<TerminalCellMetricsResource>()
            .expect("Startup system should have inserted TerminalCellMetricsResource");

        // (a) phys_font_size reflects scale_factor.
        assert_eq!(
            res.phys_font_size, 24,
            "phys_font_size should be FONT_SIZE_PX * scale_factor (12 * 2.0 = 24)"
        );

        // (b) Derived metrics are ALSO scaled to DPR=2 — catches a bug
        // where phys_font_size is right but the wrong size is fed to
        // cell_metrics_px. Compares against DPR=1 baseline rather than
        // hardcoding a font-specific advance value that would break on
        // font updates.
        let baseline = TerminalFonts::default();
        let m12 = baseline.cell_metrics_px(12);
        assert!(
            (res.metrics.advance_phys - m12.advance_phys * 2.0).abs() < 0.5,
            "advance_phys at DPR=2 ({:.3}) should be ~2x DPR=1's ({:.3})",
            res.metrics.advance_phys,
            m12.advance_phys * 2.0,
        );
    }

    /// Asserts that the logical size is multiplied by the scale factor and
    /// rounded to the nearest whole physical pixel.
    ///
    /// Case: the default 11.25 px configured font on a 2x Retina display.
    #[test]
    fn physical_font_size_scales_and_rounds() {
        assert_eq!(physical_font_size(11.25, 2.0), 23);
    }

    /// Asserts that a product rounding below one is raised to one rather than
    /// yielding a zero-pixel face.
    ///
    /// Case: a very small configured font size on a low-DPI display.
    #[test]
    fn physical_font_size_never_returns_zero() {
        assert_eq!(physical_font_size(0.4, 1.0), 1);
    }

    /// Asserts that a product that is not a number reports one pixel rather
    /// than saturating to a zero-pixel face.
    ///
    /// Case: a window reports a scale factor of zero while the configured
    /// size is infinite, so the product is NaN.
    #[test]
    fn physical_font_size_maps_a_nan_product_to_one() {
        assert_eq!(physical_font_size(f32::INFINITY, 0.0), 1);
    }

    /// Asserts that a product past the `u16` ceiling is clamped instead of
    /// wrapping.
    ///
    /// Case: a very large configured font size on a high-DPI display.
    #[test]
    fn physical_font_size_clamps_to_the_u16_ceiling() {
        assert_eq!(physical_font_size(100_000.0, 4.0), u16::MAX);
    }

    /// Asserts that the cell metrics follow a change of the primary
    /// window's scale factor, and that an update which changes nothing
    /// leaves the resource unmarked.
    ///
    /// Case: the user drags the window from a standard display to a Retina
    /// display, and the terminal then sits idle.
    #[test]
    fn cell_metrics_follow_the_scale_factor_and_stay_unmarked_otherwise() {
        use bevy::window::{PrimaryWindow, Window, WindowResolution};

        let mut app = App::new();
        let mut window = Window {
            resolution: WindowResolution::new(800, 600),
            ..default()
        };
        window.resolution.set_scale_factor(1.0);
        let window = app.world_mut().spawn((window, PrimaryWindow)).id();
        app.add_plugins(TerminalFontPlugin);
        app.update();
        assert_eq!(
            app.world()
                .resource::<TerminalCellMetricsResource>()
                .phys_font_size,
            12
        );

        app.world_mut()
            .get_mut::<Window>(window)
            .expect("the primary window")
            .resolution
            .set_scale_factor(2.0);
        app.update();
        let doubled = *app.world().resource::<TerminalCellMetricsResource>();
        assert_eq!(doubled.phys_font_size, 24);
        let expected = TerminalFonts::default().cell_metrics_px(24);
        assert!((doubled.metrics.advance_phys - expected.advance_phys).abs() < 0.001);

        let marked = app
            .world()
            .resource_ref::<TerminalCellMetricsResource>()
            .last_changed();
        app.update();
        assert_eq!(
            app.world()
                .resource_ref::<TerminalCellMetricsResource>()
                .last_changed(),
            marked
        );
    }

    /// Asserts that a system in `Update` reads the metrics of a new scale
    /// factor in the update that delivers the change, not one update later.
    ///
    /// Case: the user drags the window onto a display with twice the scale
    /// factor, and the window's grid size is recomputed in that update.
    #[test]
    fn update_reads_the_metrics_of_a_new_scale_factor_in_the_same_update() {
        use bevy::window::{PrimaryWindow, Window, WindowResolution};

        #[derive(Resource, Default)]
        struct SizeReadInUpdate(Option<u16>);

        fn record_size(
            mut read: ResMut<SizeReadInUpdate>,
            metrics: Res<TerminalCellMetricsResource>,
        ) {
            read.0 = Some(metrics.phys_font_size);
        }

        let mut app = App::new();
        let mut window = Window {
            resolution: WindowResolution::new(800, 600),
            ..default()
        };
        window.resolution.set_scale_factor(1.0);
        let window = app.world_mut().spawn((window, PrimaryWindow)).id();
        app.add_plugins(TerminalFontPlugin)
            .init_resource::<SizeReadInUpdate>()
            .add_systems(Update, record_size);
        app.update();
        assert_eq!(app.world().resource::<SizeReadInUpdate>().0, Some(12));

        app.world_mut()
            .get_mut::<Window>(window)
            .expect("the primary window")
            .resolution
            .set_scale_factor(2.0);
        app.update();
        assert_eq!(app.world().resource::<SizeReadInUpdate>().0, Some(24));
    }
}
