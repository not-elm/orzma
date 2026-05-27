use ab_glyph::{Font, FontArc, ScaleFont};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use ttf_parser::Face as TtfFace;

/// Error returned by `TerminalFonts::from_bytes` when one of the supplied
/// per-face byte buffers fails `ab_glyph::FontArc::try_from_vec`.
#[derive(Debug, thiserror::Error)]
pub enum FontLoadError {
    /// `FontArc::try_from_vec` rejected the bytes for this face.
    #[error("ab_glyph rejected {face:?} face: {source}")]
    ParseFailed {
        /// Which face's bytes were invalid.
        face: FontFace,
        /// Underlying ab_glyph error.
        #[source]
        source: ab_glyph::InvalidFont,
    },
}

/// Font size in CSS pixels; multiplied by the PrimaryWindow's
/// `scale_factor` to obtain the physical pixel size fed to
/// `cell_metrics_px`.
pub const FONT_SIZE_PX: f32 = 12.0;

/// Public `SystemSet` label used to order systems against the renderer's
/// cell-metrics initialization. App-level plugins that need to mutate
/// `TerminalFonts` before metrics are computed should run their Startup
/// systems `.before(TerminalFontInitSet::InitCellMetrics)`.
#[derive(SystemSet, Debug, Clone, Eq, PartialEq, Hash)]
pub enum TerminalFontInitSet {
    /// `init_cell_metrics_from_primary_window` lives in this set. With
    /// `bridge_font_config` (or any other override) running `.before` it,
    /// the metrics are computed from the final `TerminalFonts`.
    InitCellMetrics,
}

#[derive(Default)]
pub struct TerminalFontPlugin;

impl Plugin for TerminalFontPlugin {
    fn build(&self, app: &mut App) {
        if !app.world().contains_resource::<TerminalFonts>() {
            app.insert_resource(TerminalFonts::default());
        }
        app.add_systems(
            Startup,
            init_cell_metrics_from_primary_window
                .in_set(TerminalFontInitSet::InitCellMetrics),
        );
    }
}

/// Inserts `TerminalCellMetricsResource` at Startup based on the
/// PrimaryWindow's current scale_factor. Bevy 0.18's winit runner writes
/// the OS-reported scale_factor into the Window during `create_windows()`
/// (in `resumed()`), which runs before the first `App::update()` — so this
/// Startup system sees the correct DPR on its very first invocation,
/// eliminating the 1-frame Retina jitter where the resource would
/// otherwise hold DPR=1.0 values.
///
/// `Single<&Window, With<PrimaryWindow>>` refuses to run the system unless
/// exactly one matching entity exists; under `MinimalPlugins` (no Window)
/// the system is silently skipped, and consumers' test helpers continue
/// to insert `TerminalCellMetricsResource` manually.
fn init_cell_metrics_from_primary_window(
    mut commands: Commands,
    fonts: Res<TerminalFonts>,
    window: Single<&Window, With<PrimaryWindow>>,
) {
    let dpr = window.scale_factor();
    let phys_font_size = (FONT_SIZE_PX * dpr).round() as u16;
    let metrics = fonts.cell_metrics_px(phys_font_size);
    commands.insert_resource(TerminalCellMetricsResource {
        metrics,
        phys_font_size,
    });
}

/// Pixel metrics for the regular face at the given physical pixel size.
///
/// Data sources differ by field because no single library exposes all
/// the OpenType metrics we need:
/// - `advance_phys` / `ascent_phys` / `descent_phys` come from
///   `ab_glyph::ScaleFont`.
/// - `line_height_phys` comes from the `hhea` table via `ttf-parser`
///   (`ab_glyph::PxScale` maps the em-square exactly to the requested
///   pixel size, so its `line_gap()` is always 0 — using it would
///   collapse rows to the em-height and lose the typographic gap).
/// - `underline_position_phys` / `underline_thickness_phys` come from the
///   OpenType `post` table via `ttf-parser` (`ab_glyph` exposes no
///   underline API).
#[derive(Clone, Copy, Debug)]
pub struct CellMetrics {
    /// Horizontal advance of glyph `'0'` in physical pixels (Alacritty parity).
    pub advance_phys: f32,
    /// Ascent + |descent| + line_gap in physical pixels.
    pub line_height_phys: f32,
    /// Distance from baseline to top of em-box in physical pixels (positive).
    pub ascent_phys: f32,
    /// Distance from baseline to bottom of em-box in physical pixels (positive).
    pub descent_phys: f32,
    /// Offset from baseline to underline-stroke CENTER in physical pixels.
    /// Negative because the underline sits below the baseline. (OpenType
    /// `post.underlinePosition` convention.)
    pub underline_position_phys: f32,
    /// Underline stroke thickness in physical pixels.
    pub underline_thickness_phys: f32,
    /// Worst-case rightward overflow in physical px across all four faces
    /// (Regular/Italic/Bold/BoldItalic) over ASCII printable codepoints,
    /// measured as `max(0, outline_glyph(...).px_bounds().max.x - cell_w_phys_floor)`.
    /// Used by the shader to paint the rightmost column's overflow pixels
    /// inside the bg_padding strip; used by `resize_terminals_to_node` to
    /// reserve that strip from the available node width.
    pub max_overflow_phys: f32,
}

/// Cross-crate public Resource exposing the current `CellMetrics` for
/// `ozmux-gui::resize_terminals_to_node` and any other consumer that needs
/// the canonical cell pitch / advance values.
///
/// Inserted at `Startup` by `init_cell_metrics_from_primary_window` based
/// on the OS-reported scale_factor of the PrimaryWindow; subsequently
/// rewritten by `update_terminal_material` whenever DPR or font size
/// changes (e.g. window moved to a different-DPR display).
#[derive(Resource, Clone, Copy, Debug)]
pub struct TerminalCellMetricsResource {
    /// Current cell pitch and typographic measurements in physical pixels.
    pub metrics: CellMetrics,
    /// Physical font size (in pixels) that `metrics` was computed at.
    pub phys_font_size: u16,
}

#[derive(Resource, Clone)]
pub struct TerminalFonts {
    /// Regular weight, upright style.
    pub regular: FontArc,
    /// Bold weight, upright style.
    pub bold: FontArc,
    /// Regular weight, italic style.
    pub italic: FontArc,
    /// Bold weight, italic style.
    pub bold_italic: FontArc,
}

/// Computes the worst-case rightward overflow (in physical px) over ASCII
/// printable codepoints for a single scaled face. Uses the same
/// `outline_glyph(...).px_bounds()` path as the atlas rasterizer, so the
/// value matches what the shader actually samples.
///
/// `cell_w_phys_floor` is the floored advance the renderer uses as cell
/// pitch. The overflow is how far past that floor the rasterized bitmap
/// reaches.
fn max_ascii_overflow_for_face(face: &FontArc, px_scale: f32, cell_w_phys_floor: f32) -> f32 {
    let scaled = face.as_scaled(ab_glyph::PxScale::from(px_scale));
    let mut worst = 0.0_f32;
    for codepoint in 0x20u8..=0x7Eu8 {
        let ch = codepoint as char;
        let gid = scaled.glyph_id(ch);
        if gid.0 == 0 {
            continue;
        }
        let outlined = match scaled.outline_glyph(gid.with_scale(px_scale)) {
            Some(o) => o,
            None => continue,
        };
        let overflow = outlined.px_bounds().max.x - cell_w_phys_floor;
        if overflow > worst {
            worst = overflow;
        }
    }
    worst.max(0.0)
}

impl TerminalFonts {
    /// Constructs a `TerminalFonts` from four owned TTF byte buffers, one
    /// per face. `Vec<u8>` is required by `ab_glyph::FontArc::try_from_vec`;
    /// callers responsible for runtime font loading (e.g., the Bevy
    /// font-bridge plugin) read bytes from disk, build `Vec<u8>`, and call
    /// this. On per-face parse failure, returns `FontLoadError::ParseFailed`
    /// naming the offending face — callers may then substitute bundled
    /// bytes for that face and retry.
    pub fn from_bytes(
        regular: Vec<u8>,
        bold: Vec<u8>,
        italic: Vec<u8>,
        bold_italic: Vec<u8>,
    ) -> Result<Self, FontLoadError> {
        let regular = FontArc::try_from_vec(regular).map_err(|source| {
            FontLoadError::ParseFailed {
                face: FontFace::Regular,
                source,
            }
        })?;
        let bold =
            FontArc::try_from_vec(bold).map_err(|source| FontLoadError::ParseFailed {
                face: FontFace::Bold,
                source,
            })?;
        let italic =
            FontArc::try_from_vec(italic).map_err(|source| FontLoadError::ParseFailed {
                face: FontFace::Italic,
                source,
            })?;
        let bold_italic = FontArc::try_from_vec(bold_italic).map_err(|source| {
            FontLoadError::ParseFailed {
                face: FontFace::BoldItalic,
                source,
            }
        })?;
        Ok(Self {
            regular,
            bold,
            italic,
            bold_italic,
        })
    }

    pub fn choice(&self, face: &FontFace) -> &FontArc {
        match face {
            FontFace::Regular => &self.regular,
            FontFace::Bold => &self.bold,
            FontFace::Italic => &self.italic,
            FontFace::BoldItalic => &self.bold_italic,
        }
    }

    /// Returns full pixel metrics for the regular face at the requested
    /// physical pixel size. See [`CellMetrics`] for individual field semantics.
    pub fn cell_metrics_px(&self, phys_size_px: u16) -> CellMetrics {
        let face = TtfFace::parse(self.regular.font_data(), 0)
            .expect(
                "regular face: ttf-parser parse failed (bundled or user-supplied via FontBridgePlugin); \
                 if a user override is in effect this means the override file passed ab_glyph but \
                 ttf-parser rejected it — check the most recent FontBridgePlugin warning",
            );
        // NOTE: cast to i32 before subtraction. ascender() / descender() return
        // i16, and (asc − desc) can exceed i16::MAX for fonts where the
        // typographic envelope is unusually tall. The bundled font is safe;
        // a user-provided font might not be.
        let asc = i32::from(face.ascender());
        let desc = i32::from(face.descender());
        let upem = f32::from(face.units_per_em());
        let px_scale_value = self.px_scale_value(phys_size_px);
        let phys_size_px_f = f32::from(phys_size_px);

        let scaled = self
            .regular
            .as_scaled(ab_glyph::PxScale::from(px_scale_value));
        let advance_phys = scaled.h_advance(scaled.glyph_id('0'));
        let ascent_phys = scaled.ascent();
        let descent_phys = scaled.descent().abs();

        // NOTE: scale for ttf-parser font-unit values: phys_size_px is the
        // em-square (1 em = upem font units = phys_size_px physical pixels).
        // px_scale_value is already inflated for ab_glyph's PxScale convention
        // and must not be used here — that would double-count the em_scale factor.
        let scale = phys_size_px_f / upem;

        // NOTE: ab_glyph's PxScale maps em-square exactly to px so
        // ascent+descent==px_size and line_gap()==0. Use the hhea typographic
        // values from ttf-parser instead so the real typographic line gap is
        // preserved (drives correct line spacing).
        let line_height_phys = (asc - desc + i32::from(face.line_gap())) as f32 * scale;

        let (underline_position_phys, underline_thickness_phys) =
            if let Some(u) = face.underline_metrics() {
                (
                    f32::from(u.position) * scale,
                    (f32::from(u.thickness) * scale).max(1.0),
                )
            } else {
                (-ascent_phys * 0.07, (ascent_phys / 14.0).max(1.0))
            };

        let cell_w_phys_floor = advance_phys.floor().max(1.0);
        let max_overflow_phys = [&self.regular, &self.italic, &self.bold, &self.bold_italic]
            .iter()
            .map(|face| max_ascii_overflow_for_face(face, px_scale_value, cell_w_phys_floor))
            .fold(0.0_f32, f32::max);

        CellMetrics {
            advance_phys,
            line_height_phys,
            ascent_phys,
            descent_phys,
            underline_position_phys,
            underline_thickness_phys,
            max_overflow_phys,
        }
    }

    /// Returns the actual `PxScale` value fed to `ab_glyph` for the given
    /// CSS-pixel font size. Multiplies by `em_scale = (ascender − descender)
    /// / units_per_em` (derived from the Regular face's hhea/head tables)
    /// so the input size is interpreted as the em-square in physical pixels
    /// (CSS / Terminal.app convention), not as `ab_glyph::PxScale`'s
    /// native "ascent + |descent|" interpretation.
    ///
    /// Used by both `cell_metrics_px` (for advance / ascent / descent
    /// derivation) and `glyph/atlas.rs` (for glyph rasterization), so both
    /// agree on the actual rendering scale.
    pub(crate) fn px_scale_value(&self, phys_size_px: u16) -> f32 {
        let face = TtfFace::parse(self.regular.font_data(), 0)
            .expect(
                "regular face: ttf-parser parse failed (bundled or user-supplied via FontBridgePlugin); \
                 if a user override is in effect this means the override file passed ab_glyph but \
                 ttf-parser rejected it — check the most recent FontBridgePlugin warning",
            );
        let asc = i32::from(face.ascender());
        let desc = i32::from(face.descender());
        let upem = f32::from(face.units_per_em());
        let em_scale = (asc - desc) as f32 / upem;
        f32::from(phys_size_px) * em_scale
    }
}

impl Default for TerminalFonts {
    fn default() -> Self {
        // Bytes come from crate::bundled so the Bevy app's FontBridgePlugin
        // can reference the same static slices instead of re-embedding
        // identical copies (the linker cannot dedup include_bytes! across
        // crate boundaries without LTO; without this single source of
        // truth the binary carries the font data twice).
        Self {
            regular: FontArc::try_from_slice(crate::bundled::REGULAR)
                .expect("JetBrainsMonoNerdFontMono-Regular load"),
            bold: FontArc::try_from_slice(crate::bundled::BOLD)
                .expect("JetBrainsMonoNerdFontMono-Bold load"),
            italic: FontArc::try_from_slice(crate::bundled::ITALIC)
                .expect("JetBrainsMonoNerdFontMono-Italic load"),
            bold_italic: FontArc::try_from_slice(crate::bundled::BOLD_ITALIC)
                .expect("JetBrainsMonoNerdFontMono-BoldItalic load"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GlyphKey {
    pub face: FontFace,
    pub codepoint: u32,
    pub size_px: u16,
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum FontFace {
    Regular,
    Bold,
    Italic,
    BoldItalic,
}

impl FontFace {
    pub fn from_style(style: u16) -> Self {
        const BOLD: u16 = 1;
        const ITALIC: u16 = 2;
        let bold = (style & BOLD) != 0;
        let italic = (style & ITALIC) != 0;
        match (bold, italic) {
            (false, false) => Self::Regular,
            (true, false) => Self::Bold,
            (false, true) => Self::Italic,
            (true, true) => Self::BoldItalic,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `cell_metrics_px(12)` returns sensible values for JetBrains Mono
    /// Nerd Font Mono Regular at 12px. Empirical ranges were measured
    /// against the bundled TTF; structural invariants (positive ascent,
    /// negative underline position) are the load-bearing assertions.
    #[test]
    fn jetbrains_mono_12px_metrics_are_sensible() {
        let fonts = TerminalFonts::default();
        let m = fonts.cell_metrics_px(12);
        // Empirical ranges below were measured against the bundled
        // JetBrainsMonoNerdFontMono-Regular.ttf. Update if the font is
        // re-vendored.
        assert!(
            m.advance_phys > 6.9 && m.advance_phys < 7.5,
            "advance_phys = {} (JBM Mono 12px range)",
            m.advance_phys
        );
        assert!(
            m.line_height_phys > 15.5 && m.line_height_phys < 16.2,
            "line_height_phys = {}",
            m.line_height_phys
        );
        assert!(
            m.ascent_phys > 11.9 && m.ascent_phys < 12.5,
            "ascent_phys = {}",
            m.ascent_phys
        );
        assert!(
            m.descent_phys > 3.3 && m.descent_phys < 3.9,
            "descent_phys = {}",
            m.descent_phys
        );
        assert!(
            m.underline_position_phys < 0.0,
            "underline_position_phys = {} should be below baseline (negative)",
            m.underline_position_phys
        );
        assert!(
            m.underline_thickness_phys >= 1.0,
            "underline_thickness_phys = {}",
            m.underline_thickness_phys
        );
    }

    /// JBM Mono at 12 px must report a non-zero `max_overflow_phys`
    /// because glyphs like `W` rasterize past the floored advance.
    #[test]
    fn cell_metrics_px_reports_nonzero_max_overflow() {
        let fonts = TerminalFonts::default();
        let m = fonts.cell_metrics_px(12);
        assert!(
            m.max_overflow_phys > 0.0,
            "max_overflow_phys = {} (expected > 0 driven by wide ASCII glyphs)",
            m.max_overflow_phys
        );
    }

    /// `max_overflow_phys` must cover the worst face — independently
    /// measure each of the 4 faces and verify each is ≤ the reported
    /// `max_overflow_phys`. Catches a regression where the fold drops
    /// any face from the per-face max computation.
    #[test]
    fn cell_metrics_px_max_overflow_covers_all_faces() {
        let fonts = TerminalFonts::default();
        let m = fonts.cell_metrics_px(12);

        let face = TtfFace::parse(fonts.regular.font_data(), 0).unwrap();
        let upem = f32::from(face.units_per_em());
        let em_scale = (i32::from(face.ascender()) - i32::from(face.descender())) as f32 / upem;
        let px_scale = 12.0_f32 * em_scale;
        let cell_w_phys_floor = m.advance_phys.floor().max(1.0);

        for (name, face_arc) in [
            ("Regular", &fonts.regular),
            ("Italic", &fonts.italic),
            ("Bold", &fonts.bold),
            ("BoldItalic", &fonts.bold_italic),
        ] {
            let face_overflow = max_ascii_overflow_for_face(face_arc, px_scale, cell_w_phys_floor);
            assert!(
                face_overflow <= m.max_overflow_phys + 0.001,
                "{} face overflow = {} exceeds reported max_overflow_phys = {}",
                name,
                face_overflow,
                m.max_overflow_phys,
            );
        }
    }

    /// 24 px metrics are approximately double the 12 px ones.
    #[test]
    fn metrics_scale_linearly_with_size() {
        let fonts = TerminalFonts::default();
        let m12 = fonts.cell_metrics_px(12);
        let m24 = fonts.cell_metrics_px(24);
        assert!((m24.advance_phys - m12.advance_phys * 2.0).abs() < 0.5);
        assert!((m24.line_height_phys - m12.line_height_phys * 2.0).abs() < 0.5);
    }

    /// `cell_metrics_px` and `glyph/atlas.rs` must rasterize at the SAME
    /// PxScale, otherwise atlas glyphs are physically smaller (or larger)
    /// than the cell pitch and either leave blank gutters on the right
    /// (atlas < cell) or overflow without coverage (atlas > cell).
    /// This test guards against accidental divergence.
    #[test]
    fn px_scale_value_matches_cell_metrics_internal_use() {
        let fonts = TerminalFonts::default();
        let phys = 12u16;
        let helper_value = fonts.px_scale_value(phys);
        let metrics = fonts.cell_metrics_px(phys);
        let scaled = fonts
            .regular
            .as_scaled(ab_glyph::PxScale::from(helper_value));
        let expected_advance = scaled.h_advance(scaled.glyph_id('0'));
        assert!(
            (metrics.advance_phys - expected_advance).abs() < 0.001,
            "cell_metrics advance = {} disagrees with px_scale_value-derived advance = {}",
            metrics.advance_phys,
            expected_advance,
        );
    }

    /// `TerminalFontPlugin::build` must not overwrite an already-present
    /// `TerminalFonts` resource. Apps pre-insert a custom `TerminalFonts`
    /// (e.g., from a runtime config-driven font override) BEFORE adding
    /// `TerminalFontPlugin`; if the plugin overwrote it, the override
    /// would be lost.
    #[test]
    fn terminal_font_plugin_preserves_pre_inserted_terminal_fonts() {
        use bevy::window::{PrimaryWindow, Window, WindowResolution};

        // Build a non-default TerminalFonts via from_bytes — same TTF for
        // all four faces (legal for a smoke test; the labels are advisory).
        let bytes: Vec<u8> = crate::bundled::REGULAR.to_vec();
        let custom = TerminalFonts::from_bytes(
            bytes.clone(),
            bytes.clone(),
            bytes.clone(),
            bytes,
        )
        .expect("from_bytes accepts JBM regular for all four slots");

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

    /// `init_cell_metrics_from_primary_window` reads the PrimaryWindow's
    /// scale_factor and inserts a DPR-aware `TerminalCellMetricsResource`.
    /// Verifies BOTH (a) `phys_font_size` reflects the scale_factor and
    /// (b) the derived metrics (advance_phys) are also DPR-scaled —
    /// catches a regression where phys_font_size is correct but a wrong
    /// size (e.g. FONT_SIZE_PX as u16) is fed to cell_metrics_px.
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
}
