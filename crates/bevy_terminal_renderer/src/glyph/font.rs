use ab_glyph::{Font, FontArc, ScaleFont};
use bevy::prelude::*;
use ttf_parser::Face as TtfFace;

#[derive(Default)]
pub struct TerminalFontPlugin;

impl Plugin for TerminalFontPlugin {
    fn build(&self, app: &mut App) {
        let fonts = TerminalFonts::default();
        let default_metrics = fonts.cell_metrics_px(12);
        app.insert_resource(fonts);
        app.insert_resource(TerminalCellMetricsResource {
            metrics: default_metrics,
            phys_font_size: 12,
        });
    }
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
}

/// Cross-crate public Resource exposing the current `CellMetrics` for
/// `ozmux-gui::resize_terminals_to_node` and any other consumer that needs
/// the canonical cell pitch / advance values.
///
/// Initialized at `Startup` with DPR=1.0 defaults; updated every time
/// `update_terminal_material` recomputes metrics (DPR or font-size change).
/// Consumers reading this Resource on a frame between Startup and the first
/// `update_terminal_material` invocation will see DPR=1.0 values; the spec
/// documents this 1-frame jitter as an accepted Tier 1 trade-off.
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
    /// Raw byte slice of `regular` for `ttf-parser` re-parse (underline metrics).
    ///
    /// `ab_glyph` does not expose `post`/`OS/2` table data, so we hold the
    /// same `include_bytes!` slice to feed `ttf_parser::Face::parse` on
    /// metrics requests. Zero extra memory cost — both crates borrow the
    /// same static slice.
    pub(crate) regular_ttf_bytes: &'static [u8],
}

impl TerminalFonts {
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
        let face = TtfFace::parse(self.regular_ttf_bytes, 0)
            .expect("JetBrainsMono-Regular ttf-parser parse");
        // NOTE: cast to i32 before subtraction. ascender() / descender() return
        // i16, and (asc − desc) can exceed i16::MAX for fonts where the
        // typographic envelope is unusually tall. JBM (1320) is safe; a
        // user-provided font might not be.
        let asc = i32::from(face.ascender());
        let desc = i32::from(face.descender());
        let upem = f32::from(face.units_per_em());
        let em_scale = (asc - desc) as f32 / upem;
        let phys_size_px_f = f32::from(phys_size_px);
        let px_scale_value = phys_size_px_f * em_scale;

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

        CellMetrics {
            advance_phys,
            line_height_phys,
            ascent_phys,
            descent_phys,
            underline_position_phys,
            underline_thickness_phys,
        }
    }
}

impl Default for TerminalFonts {
    fn default() -> Self {
        const REGULAR_BYTES: &[u8] = include_bytes!("./JetBrainsMono-Regular.ttf");
        Self {
            regular: FontArc::try_from_slice(REGULAR_BYTES).expect("JetBrainsMono-Regular load"),
            bold: FontArc::try_from_slice(include_bytes!("./JetBrainsMono-Bold.ttf"))
                .expect("JetBrainsMono-Bold load"),
            italic: FontArc::try_from_slice(include_bytes!("./JetBrainsMono-Italic.ttf"))
                .expect("JetBrainsMono-Italic load"),
            bold_italic: FontArc::try_from_slice(include_bytes!("./JetBrainsMono-BoldItalic.ttf"))
                .expect("JetBrainsMono-BoldItalic load"),
            regular_ttf_bytes: REGULAR_BYTES,
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

    /// `cell_metrics_px(12)` returns sensible values for JetBrains Mono Regular 12px.
    /// Empirical reference values after em-square scaling (JBM 1.32 multiplier):
    ///   advance(`0`) ≈ 7.2,  line_height ≈ 15.8,  ascent ≈ 12.x,  descent ≈ 3.x
    /// underline_position is negative (below baseline), underline_thickness is positive.
    #[test]
    fn jetbrains_mono_12px_metrics_are_sensible() {
        let fonts = TerminalFonts::default();
        let m = fonts.cell_metrics_px(12);
        assert!(
            m.advance_phys > 7.0 && m.advance_phys < 7.5,
            "advance_phys = {} (CSS/Terminal.app range)",
            m.advance_phys
        );
        assert!(
            m.line_height_phys > 15.0 && m.line_height_phys < 17.0,
            "line_height_phys = {}",
            m.line_height_phys
        );
        assert!(
            m.ascent_phys > 11.5 && m.ascent_phys < 13.0,
            "ascent_phys = {}",
            m.ascent_phys
        );
        assert!(
            m.descent_phys > 2.5 && m.descent_phys < 4.5,
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

    /// 24 px metrics are approximately double the 12 px ones.
    #[test]
    fn metrics_scale_linearly_with_size() {
        let fonts = TerminalFonts::default();
        let m12 = fonts.cell_metrics_px(12);
        let m24 = fonts.cell_metrics_px(24);
        assert!((m24.advance_phys - m12.advance_phys * 2.0).abs() < 0.5);
        assert!((m24.line_height_phys - m12.line_height_phys * 2.0).abs() < 0.5);
    }

    /// `TerminalFontPlugin` inserts `TerminalCellMetricsResource` at Startup
    /// with the DPR=1.0 / FONT_SIZE_PX=12 default values, so gui-side
    /// consumers can read non-None metrics on the first frame.
    #[test]
    fn font_plugin_inserts_default_cell_metrics_resource() {
        let mut app = App::new();
        app.add_plugins(TerminalFontPlugin);
        app.update();
        let res = app
            .world()
            .get_resource::<TerminalCellMetricsResource>()
            .expect("TerminalCellMetricsResource should be inserted by Startup");
        assert_eq!(res.phys_font_size, 12);
        assert!(res.metrics.advance_phys > 7.0 && res.metrics.advance_phys < 7.5);
    }
}
