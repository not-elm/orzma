//! The per-pane uniform block: its std140 layout and how it is computed
//! from a pane's state.

use crate::{
    cursor::{CaretPaint, PackedCursorStyle},
    font::CellMetrics,
    grid::TerminalView,
    material::{OVERLAY_SLOTS, pack_linear},
    pane_style::PaneInactiveStyle,
};
use bevy::{prelude::*, render::render_resource::ShaderType};
use orzma_vt::prelude::{GridLine, Palette, Rgb, SelectionGeometry, SelectionRange};

mod write;

pub use write::TerminalPaddingFallback;
pub(crate) use write::TerminalParamsPlugin;

/// Uniform block uploaded once per frame alongside the storage buffers.
///
/// The WGSL `TerminalParams` declaration must match this std140 layout,
/// whose field offsets are in bytes and whose total size is 336 bytes:
///
/// | Offset | Field                       |
/// |--------|-----------------------------|
/// | 0      | `grid_size`                 |
/// | 8      | `cell_size_px` (phys)       |
/// | 16     | `atlas_size_px`             |
/// | 24     | `ascent_px` (phys)          |
/// | 28     | `dpr` (informational)       |
/// | 32     | `cursor_pos`                |
/// | 40     | `cursor_style`              |
/// | 44     | `cursor_thickness_phys`     |
/// | 48     | `sel_start_row`             |
/// | 52     | `sel_start_col`             |
/// | 56     | `sel_end_row`               |
/// | 60     | `sel_end_col`               |
/// | 64     | `sel_kind`                  |
/// | 68     | `underline_position_phys`   |
/// | 72     | `underline_thickness_phys`  |
/// | 76     | `max_overflow_phys`         |
/// | 80     | `bg_padding_color`          |
/// | 96     | `hover_hyperlink_id`        |
/// | 100    | `hover_active`              |
/// | 104    | `dim`                       |
/// | 112    | `inactive_tint`             |
/// | 128    | `overlay_rects`             |
/// | 320    | `overlay_dim`               |
/// | 324    | `overlay_desaturate`        |
/// | 328    | `cursor_packed`             |
/// | 332    | `default_fg_packed`         |
///
/// # Invariants
///
/// - All `_phys` fields are PHYSICAL pixels (no DPR division). The shader
///   computes everything in physical-px space and never reads `dpr`.
/// - `bg_padding_color` is the color the shader paints OUTSIDE the
///   `grid_size * cell_size_px` rectangle.
/// - "No cursor" is encoded by clearing the `CURSOR_VISIBLE` bit in
///   `cursor_style` (and leaving `cursor_pos` at any value); the shader
///   then paints no cursor. A cursor (vi or live) whose grid line
///   projects outside the viewport takes the same path.
/// - `cursor_packed == 0` means no cursor color is set, and the shader
///   paints the cursor in the foreground of the cell under it. A set
///   color packs like a cell color, whose alpha byte is never zero. A
///   set color equal to the default foreground uploads as unset.
/// - A cursor fill whose contrast against the cell's ground falls below
///   1.5 is swapped in the shader for `default_fg_packed` or
///   `bg_padding_color`, whichever stands out more.
#[derive(Clone, Copy, ShaderType, Debug)]
pub(super) struct TerminalParams {
    grid_size: UVec2,
    cell_size_px: Vec2,
    atlas_size_px: Vec2,
    ascent_px: f32,
    dpr: f32,
    cursor_pos: UVec2,
    /// The [`PackedCursorStyle`] bits: bit0=visible, bits1-2=shape
    /// (0=block / 1=underline / 2=bar), bit4=hollow. Bit 3 is unused.
    cursor_style: u32,
    /// Caret thickness in physical pixels for the underline, bar and
    /// hollow outlines, at least 1.
    cursor_thickness_phys: f32,
    /// Selection start row in viewport coords; an endpoint in scrollback
    /// clamps to `-1` (above) or `rows` (below).
    sel_start_row: i32,
    sel_start_col: u32,
    sel_end_row: i32,
    sel_end_col: u32,
    /// 0 = none, 1 = char, 2 = line.
    sel_kind: u32,
    underline_position_phys: f32,
    underline_thickness_phys: f32,
    /// Worst-case ASCII rightward bbox overflow (physical px) across all four
    /// faces. The shader uses this to extend its "rightmost column glyph"
    /// evaluation past `grid_size.x * cell_size_px.x` into the bg_padding
    /// strip; the host must reserve the same amount from the node width.
    max_overflow_phys: f32,
    bg_padding_color: Vec4,
    /// Wire id of the hovered link (across all panes); `0` = nothing
    /// hovered, or hovered cell is unlinked.
    hover_hyperlink_id: u32,
    /// `1` when the activation modifier is held AND the hovered link
    /// is in this entity's pane; else `0`. Drives the accent-underline
    /// path in the shader.
    hover_active: u32,
    /// Pane brightness multiplier applied to the final fragment RGB.
    /// `1.0` = active / full-bright; `< 1.0` dims an inactive pane.
    dim: f32,
    /// Per-pane background tint: `rgb` = target color (LINEAR), `a` = blend
    /// amount in `0.0..=1.0`. The shader blends each background source toward
    /// `rgb` by `a` BEFORE glyphs/overlays paint (background only). `a == 0`
    /// (active / no-op) leaves the background untouched.
    inactive_tint: Vec4,
    /// Slot-indexed inline-overlay rects `(row, col, rows, cols)` in cell
    /// coords; `row` may be negative; `rows == 0` = inactive slot sentinel.
    overlay_rects: [IVec4; OVERLAY_SLOTS],
    /// Inline-overlay (webview) brightness multiplier applied to overlay samples
    /// before they blend over the background. `1.0` = active / no-op.
    overlay_dim: f32,
    /// Inline-overlay (webview) desaturation toward Rec.709 luminance applied to
    /// overlay samples. `0.0` = active / no-op.
    overlay_desaturate: f32,
    /// The `OSC 12` cursor color.
    cursor_packed: u32,
    /// The default foreground in the cell-color packing.
    default_fg_packed: u32,
}

impl Default for TerminalParams {
    fn default() -> Self {
        Self {
            grid_size: UVec2::ZERO,
            cell_size_px: Vec2::ZERO,
            atlas_size_px: Vec2::ZERO,
            ascent_px: 0.0,
            dpr: 0.0,
            cursor_pos: UVec2::ZERO,
            cursor_style: 0,
            cursor_thickness_phys: 1.0,
            sel_start_row: 0,
            sel_start_col: 0,
            sel_end_row: 0,
            sel_end_col: 0,
            sel_kind: 0,
            underline_position_phys: 0.0,
            underline_thickness_phys: 0.0,
            max_overflow_phys: 0.0,
            bg_padding_color: Vec4::ZERO,
            hover_hyperlink_id: 0,
            hover_active: 0,
            dim: 1.0,
            inactive_tint: Vec4::ZERO,
            overlay_rects: [IVec4::ZERO; OVERLAY_SLOTS],
            overlay_dim: 1.0,
            overlay_desaturate: 0.0,
            cursor_packed: 0,
            default_fg_packed: 0,
        }
    }
}

impl TerminalParams {
    /// Builds the per-frame uniform block from the current view and the
    /// caret paint the policy settled on.
    ///
    /// The cell size is the floored cell pitch of `metrics`, the baseline is
    /// its rounded ascent, and the caret strokes the `cursor_thickness`
    /// share of the cell width, rounded and at least one pixel thick.
    ///
    /// # Invariants
    ///
    /// - `cursor_style` is packed from `caret`; a `None` caret paints
    ///   nothing and leaves `cursor_pos` at the origin.
    /// - When `view.selection` is `None`, `sel_kind == 0` and the shader
    ///   paints no selection.
    /// - `overlay_rects` is left at its default; the caller fills it from
    ///   the entity's overlays.
    fn new(
        view: &TerminalView,
        palette: &Palette,
        metrics: &CellMetrics,
        treatment: &PaneTreatment,
        atlas_size_px: Vec2,
        dpr: f32,
        fallback: [u8; 3],
        hover_hyperlink_id: u32,
        hover_active: u32,
        caret: Option<CaretPaint>,
        cursor_thickness: f32,
    ) -> Self {
        let cols = u32::from(view.cols);
        let rows = u32::from(view.rows);
        let cell_size_px = metrics.cell_size_phys();

        let (cursor_pos, cursor_style) = match caret {
            Some(caret) => (caret.pos, PackedCursorStyle::from(caret.stroke).bits()),
            None => (UVec2::ZERO, 0),
        };
        let (sel_start_row, sel_start_col, sel_end_row, sel_end_col, sel_kind) =
            selection_uniforms(view.selection.as_ref(), view.display_offset, view.rows);
        let bg_padding_color = padding_color(palette.background, fallback);

        Self {
            grid_size: UVec2::new(cols.max(1), rows.max(1)),
            cell_size_px,
            atlas_size_px,
            ascent_px: metrics.ascent_phys.round(),
            dpr,
            cursor_pos,
            cursor_style,
            cursor_thickness_phys: (cursor_thickness * cell_size_px.x).round().max(1.0),
            sel_start_row,
            sel_start_col,
            sel_end_row,
            sel_end_col,
            sel_kind,
            underline_position_phys: metrics.underline_position_phys,
            underline_thickness_phys: metrics.underline_thickness_phys.max(1.0),
            max_overflow_phys: metrics.max_overflow_phys,
            bg_padding_color,
            hover_hyperlink_id,
            hover_active,
            dim: treatment.dim,
            inactive_tint: treatment.inactive_tint,
            overlay_rects: [IVec4::ZERO; OVERLAY_SLOTS],
            overlay_dim: treatment.overlay_dim,
            overlay_desaturate: treatment.overlay_desaturate,
            cursor_packed: palette
                .cursor
                .filter(|color| *color != palette.foreground)
                .map_or(0, pack_linear),
            default_fg_packed: pack_linear(palette.foreground),
        }
    }
}

/// The dimming and tinting one pane applies while it is not the active
/// pane.
struct PaneTreatment {
    dim: f32,
    inactive_tint: Vec4,
    overlay_dim: f32,
    overlay_desaturate: f32,
}

impl PaneTreatment {
    /// Clamps `style`'s factors into range, or returns the neutral
    /// treatment when the pane carries no inactive style.
    fn from_style(style: Option<&PaneInactiveStyle>) -> Self {
        style.map_or(
            Self {
                dim: 1.0,
                inactive_tint: Vec4::ZERO,
                overlay_dim: 1.0,
                overlay_desaturate: 0.0,
            },
            |style| Self {
                dim: style.dim.clamp(0.0, 1.0),
                inactive_tint: style.tint.with_w(style.tint.w.clamp(0.0, 1.0)),
                overlay_dim: style.overlay_dim.clamp(0.0, 1.0),
                overlay_desaturate: style.overlay_desaturate.clamp(0.0, 1.0),
            },
        )
    }
}

fn padding_color(default_bg: Rgb, fallback: [u8; 3]) -> Vec4 {
    let [r, g, b] = if default_bg == (Rgb { r: 0, g: 0, b: 0 }) {
        fallback
    } else {
        [default_bg.r, default_bg.g, default_bg.b]
    };
    let c = Color::srgb_u8(r, g, b).to_linear();
    Vec4::new(c.red, c.green, c.blue, 1.0)
}

/// Projects a grid-space selection into the clamped viewport-space
/// uniform tuple the shader consumes. Rows clamp to the -1 (above) /
/// `rows` (below) sentinels, so a partially visible selection still
/// paints its on-screen span.
fn selection_uniforms(
    selection: Option<&SelectionRange>,
    display_offset: u32,
    rows: u16,
) -> (i32, u32, i32, u32, u32) {
    let Some(sel) = selection else {
        return (0, 0, 0, 0, 0);
    };
    let clamp_row = |line: GridLine| -> i32 {
        (i64::from(line.0) + i64::from(display_offset)).clamp(-1, i64::from(rows)) as i32
    };
    let kind = match sel.geometry {
        // TODO: Give the shader a rectangular selection mode so that a
        // `Block` selection stops painting as a char selection.
        SelectionGeometry::Linear | SelectionGeometry::Block => 1u32,
        SelectionGeometry::Lines => 2,
    };
    (
        clamp_row(sel.start.line),
        u32::from(sel.start.column.0),
        clamp_row(sel.end.line),
        u32::from(sel.end.column.0),
        kind,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts that the uniforms take the cell size as the floored cell
    /// pitch, the baseline as the rounded ascent, and the caret thickness
    /// as the rounded share of the cell width.
    ///
    /// Case: a fractional font size measures a 7.6 by 15.4 pixel cell with
    /// an 11.6 pixel ascent, and the caret is set to 30% of the cell width.
    #[test]
    fn terminal_params_derive_cell_size_baseline_and_caret_from_the_metrics() {
        let metrics = CellMetrics {
            advance_phys: 7.6,
            line_height_phys: 15.4,
            ascent_phys: 11.6,
            descent_phys: 3.8,
            underline_position_phys: -1.5,
            underline_thickness_phys: 1.0,
            max_overflow_phys: 0.0,
        };
        let params = TerminalParams::new(
            &TerminalView::default(),
            &Palette::default(),
            &metrics,
            &PaneTreatment::from_style(None),
            Vec2::new(64.0, 64.0),
            1.0,
            [0, 0, 0],
            0,
            0,
            None,
            0.3,
        );
        assert_eq!(params.cell_size_px, Vec2::new(7.0, 15.0));
        assert_eq!(params.ascent_px, 12.0);
        assert_eq!(params.cursor_thickness_phys, 2.0);
    }

    /// Asserts that the default uniforms mark no link as hovered and leave
    /// the hover accent off.
    ///
    /// Case: the user splits a pane while the pointer rests on a link in the
    /// old pane, and the new pane starts from the default uniforms.
    #[test]
    fn terminal_params_default_hyperlink_uniforms_are_zero() {
        let params = TerminalParams::default();
        assert_eq!(params.hover_hyperlink_id, 0);
        assert_eq!(params.hover_active, 0);
    }

    /// Asserts that the default uniforms draw the pane at full brightness.
    ///
    /// Case: a new window opens with a single pane, which carries no
    /// inactive-pane style.
    #[test]
    fn terminal_params_default_dim_is_one() {
        assert_eq!(TerminalParams::default().dim, 1.0);
    }

    /// Asserts that the uniform block's std140 size is 336 bytes with the
    /// overlay rects included.
    ///
    /// Case: a pane shows an inline webview whose placement the shader reads
    /// from the overlay rects in the uniform block.
    #[test]
    fn terminal_params_uniform_size_includes_overlay_rects() {
        assert_eq!(<TerminalParams as ShaderType>::min_size().get(), 336);
    }

    /// Asserts that the default uniforms apply no background tint and leave
    /// overlays undimmed and in full color.
    ///
    /// Case: the only pane in a window shows an inline webview, and no
    /// inactive-pane style has ever been applied to it.
    #[test]
    fn terminal_params_default_inactive_treatment_is_noop() {
        let p = TerminalParams::default();
        assert_eq!(p.inactive_tint, Vec4::ZERO);
        assert_eq!(p.overlay_dim, 1.0);
        assert_eq!(p.overlay_desaturate, 0.0);
    }

    /// Asserts the std140 offsets of the fields after `dim`, where a
    /// `Vec4` and the rect array force alignment padding, down to the
    /// packed default foreground in the tail.
    ///
    /// Case: an inactive pane shows a dimmed, desaturated webview while
    /// nvim in it holds an `OSC 12` cursor color.
    #[test]
    fn terminal_params_field_offsets_are_pinned() {
        assert_eq!(
            <TerminalParams as ShaderType>::METADATA.offset(19),
            104,
            "dim"
        );
        assert_eq!(
            <TerminalParams as ShaderType>::METADATA.offset(20),
            112,
            "inactive_tint (Vec4, 16-byte aligned) after the pad following dim"
        );
        assert_eq!(
            <TerminalParams as ShaderType>::METADATA.offset(21),
            128,
            "overlay_rects after inactive_tint"
        );
        assert_eq!(
            <TerminalParams as ShaderType>::METADATA.offset(22),
            320,
            "overlay_dim after overlay_rects"
        );
        assert_eq!(
            <TerminalParams as ShaderType>::METADATA.offset(23),
            324,
            "overlay_desaturate after overlay_dim"
        );
        assert_eq!(
            <TerminalParams as ShaderType>::METADATA.offset(24),
            328,
            "cursor_packed after overlay_desaturate"
        );
        assert_eq!(
            <TerminalParams as ShaderType>::METADATA.offset(25),
            332,
            "default_fg_packed after cursor_packed"
        );
    }

    /// Asserts that a black default background paints the padding in the
    /// fallback color rather than black.
    ///
    /// Case: the shell never sets a background color, so the pane keeps its
    /// built-in black one, and the window's width leaves a strip beside the
    /// grid's last column.
    #[test]
    fn padding_color_falls_back_when_default_bg_is_black() {
        let got = padding_color(Rgb { r: 0, g: 0, b: 0 }, [30, 32, 40]);
        let c = Color::srgb_u8(30, 32, 40).to_linear();
        assert_eq!(got, Vec4::new(c.red, c.green, c.blue, 1.0));
    }

    /// Asserts that a non-black default background paints the padding in
    /// that background, ignoring the fallback color.
    ///
    /// Case: a theme script sets a dark navy background with `OSC 11`, and
    /// the window's width leaves a strip beside the grid's last column.
    #[test]
    fn padding_color_uses_default_bg_when_set() {
        let got = padding_color(
            Rgb {
                r: 10,
                g: 20,
                b: 30,
            },
            [99, 99, 99],
        );
        let c = Color::srgb_u8(10, 20, 30).to_linear();
        assert_eq!(got, Vec4::new(c.red, c.green, c.blue, 1.0));
    }

    /// Asserts that in-viewport selection endpoints map to their
    /// viewport rows and the geometry maps to the shader encoding.
    ///
    /// Case: the user drags a whole-line selection across two visible rows
    /// at the live tail.
    #[test]
    fn selection_uniforms_projects_in_viewport_endpoints() {
        use orzma_vt::prelude::{
            GridColumn, GridLine, GridPoint, SelectionGeometry, SelectionRange,
        };
        let sel = SelectionRange {
            start: GridPoint {
                line: GridLine(1),
                column: GridColumn(2),
            },
            end: GridPoint {
                line: GridLine(3),
                column: GridColumn(4),
            },
            geometry: SelectionGeometry::Lines,
        };
        assert_eq!(selection_uniforms(Some(&sel), 0, 24), (1, 2, 3, 4, 2));
        assert_eq!(selection_uniforms(None, 0, 24), (0, 0, 0, 0, 0));
    }

    /// Asserts that endpoints outside the viewport clamp to the -1 /
    /// `rows` sentinels instead of disappearing.
    ///
    /// Case: the user scrolls partway back through a selection that
    /// spans from scrollback history down past the visible window, so
    /// only the middle of it is on screen.
    #[test]
    fn selection_uniforms_clamps_off_viewport_endpoints() {
        use orzma_vt::prelude::{
            GridColumn, GridLine, GridPoint, SelectionGeometry, SelectionRange,
        };
        let sel = SelectionRange {
            start: GridPoint {
                line: GridLine(-40),
                column: GridColumn(0),
            },
            end: GridPoint {
                line: GridLine(30),
                column: GridColumn(5),
            },
            geometry: SelectionGeometry::Linear,
        };
        assert_eq!(selection_uniforms(Some(&sel), 10, 24), (-1, 0, 24, 5, 1));
    }

    fn params_for(palette: &Palette) -> TerminalParams {
        let metrics = CellMetrics {
            advance_phys: 8.0,
            line_height_phys: 16.0,
            ascent_phys: 12.0,
            descent_phys: 4.0,
            underline_position_phys: -2.0,
            underline_thickness_phys: 1.0,
            max_overflow_phys: 0.0,
        };
        TerminalParams::new(
            &TerminalView::default(),
            palette,
            &metrics,
            &PaneTreatment::from_style(None),
            Vec2::new(64.0, 64.0),
            1.0,
            [0, 0, 0],
            0,
            0,
            None,
            0.25,
        )
    }

    /// Asserts that the uniform carries zero for an unset cursor color
    /// and the cell packing of the color once one is set, which stays
    /// nonzero even for black.
    ///
    /// Case: nvim recolors the cursor with `OSC 12`, a light theme sets a
    /// black cursor, and `OSC 112` later restores the default.
    #[test]
    fn terminal_params_carry_the_cursor_color() {
        assert_eq!(params_for(&Palette::default()).cursor_packed, 0);
        let orange = Rgb {
            r: 0xff,
            g: 0x88,
            b: 0x00,
        };
        let black = Rgb { r: 0, g: 0, b: 0 };
        for color in [orange, black] {
            let palette = Palette {
                cursor: Some(color),
                ..Palette::default()
            };
            let packed = params_for(&palette).cursor_packed;
            assert_eq!(packed, pack_linear(color));
            assert_ne!(packed, 0, "{color:?} collides with the unset sentinel");
        }
    }

    /// Asserts that the uniform carries the default foreground in the
    /// cell-color packing.
    ///
    /// Case: a theme script recolors the text with `OSC 10`, and the
    /// cursor's contrast fallback has to follow it.
    #[test]
    fn terminal_params_carry_the_default_foreground() {
        let palette = Palette {
            foreground: Rgb {
                r: 0x20,
                g: 0x20,
                b: 0x20,
            },
            ..Palette::default()
        };
        assert_eq!(
            params_for(&palette).default_fg_packed,
            pack_linear(palette.foreground)
        );
    }

    /// Asserts that a cursor color equal to the default foreground is
    /// uploaded as unset rather than as a set color, and that the
    /// comparison follows the live foreground rather than the built-in
    /// one.
    ///
    /// Case: a theme script sends the same value for `OSC 10` and
    /// `OSC 12`, or a program writes the terminal's own `OSC 12 ; ?`
    /// reply back to it.
    #[test]
    fn a_cursor_color_equal_to_the_foreground_is_uploaded_as_unset() {
        let palette = Palette {
            cursor: Some(Palette::default().foreground),
            ..Palette::default()
        };
        assert_eq!(params_for(&palette).cursor_packed, 0);
        let recolored = Palette {
            foreground: Rgb {
                r: 0x20,
                g: 0x20,
                b: 0x20,
            },
            cursor: Some(Palette::default().foreground),
            ..Palette::default()
        };
        assert_ne!(params_for(&recolored).cursor_packed, 0);
    }

    /// Asserts that the shader declares the packed default foreground as
    /// the last field of its uniform block, matching the host layout.
    ///
    /// Case: a shell theme recolors the default foreground with `OSC 10`,
    /// and a cursor whose fill lacks contrast against the cell under it
    /// is repainted in that foreground.
    #[test]
    fn wgsl_terminal_params_end_with_the_default_foreground() {
        let src = include_str!("../shaders/terminal_ui_material.wgsl");
        let declaration = src
            .split("struct TerminalParams {")
            .nth(1)
            .and_then(|rest| rest.split("};").next())
            .expect("the shader declares TerminalParams");
        let last_field = declaration
            .lines()
            .map(str::trim)
            .rfind(|line| !line.is_empty());
        assert_eq!(last_field, Some("default_fg_packed: u32,"));
    }

    /// Asserts that the shader declares no time uniform, so every
    /// blink phase reaching the GPU was decided on the CPU.
    ///
    /// Case: the user watches a caret blink while the terminal is
    /// otherwise idle.
    #[test]
    fn the_shader_declares_no_time_uniform() {
        let src = include_str!("../shaders/terminal_ui_material.wgsl");
        assert!(!src.contains("time_seconds"));
    }

    /// Asserts that the shader's cursor bit constants match the Rust
    /// ones they decode.
    ///
    /// Case: a program selects a bar caret, and the pane it sits in
    /// goes inactive so the caret is drawn hollow as well.
    #[test]
    fn the_shader_cursor_bits_match_the_rust_constants() {
        let src = include_str!("../shaders/terminal_ui_material.wgsl");
        assert!(src.contains(&format!(
            "const CURSOR_VISIBLE: u32 = {}u;",
            PackedCursorStyle::VISIBLE.bits()
        )));
        assert!(src.contains(&format!(
            "const CURSOR_HOLLOW: u32 = {}u;",
            PackedCursorStyle::HOLLOW.bits()
        )));
        assert!(src.contains("const CURSOR_SHAPE_BLOCK: u32 = 0u;"));
        assert!(src.contains(&format!(
            "const CURSOR_SHAPE_UNDERLINE: u32 = {}u;",
            PackedCursorStyle::SHAPE_UNDERLINE.bits() >> 1
        )));
        assert!(src.contains(&format!(
            "const CURSOR_SHAPE_BAR: u32 = {}u;",
            PackedCursorStyle::SHAPE_BAR.bits() >> 1
        )));
    }
}
