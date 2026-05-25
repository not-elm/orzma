
#import bevy_ui::ui_vertex_output::UiVertexOutput

struct TerminalParams {
    grid_size: vec2<u32>,
    cell_size_px: vec2<f32>,
    atlas_size_px: vec2<f32>,
    ascent_px: f32,
    dpr: f32,
    cursor_pos: vec2<u32>,
    cursor_style: u32,
    time_seconds: f32,
    sel_start_row: i32,
    sel_start_col: u32,
    sel_end_row: i32,
    sel_end_col: u32,
    sel_kind: u32,
    underline_position_phys: f32,
    underline_thickness_phys: f32,
    bg_padding_color: vec4<f32>,
};

struct Cell {
    glyph_index: u32,
    fg_packed: u32,
    bg_packed: u32,
    style_flags: u32,
};

struct Glyph {
    uv_min: vec2<f32>,
    uv_max: vec2<f32>,
    offset_px: vec2<f32>,
    size_px: vec2<f32>,
};

@group(1) @binding(0) var<uniform> params: TerminalParams;
@group(1) @binding(1) var<storage, read> cells: array<Cell>;
@group(1) @binding(2) var<storage, read> glyphs: array<Glyph>;
@group(1) @binding(3) var atlas_tex: texture_2d<f32>;
@group(1) @binding(4) var atlas_sampler: sampler;

// NOTE: Must stay in sync with `ozmux_terminal_protocol::style::*`. The
//       Rust-side test `style_bits_match_protocol_constants` asserts the
//       literal values here against the canonical Rust constants.
const STYLE_UNDERLINE: u32 = 4u;
const STYLE_STRIKE: u32 = 8u;
const STYLE_REVERSE: u32 = 16u;
const STYLE_DIM: u32 = 32u;
const STYLE_HIDDEN: u32 = 64u;

// Renderer-only style bits (bit 16+) — see material.rs.
const STYLE_WIDE_RIGHT_HALF: u32 = 0x10000u;

const CURSOR_VISIBLE: u32 = 1u;
const CURSOR_BLINKING: u32 = 8u;
const CURSOR_SHAPE_BLOCK: u32 = 0u;
const CURSOR_SHAPE_UNDERLINE: u32 = 1u;
const CURSOR_SHAPE_BAR: u32 = 2u;

const GLYPH_NONE: u32 = 0xFFFFFFFFu;

fn is_in_selection_uniform(
    row: i32, col: i32,
    kind: u32,
    s_row: i32, s_col: i32,
    e_row: i32, e_col: i32,
) -> bool {
    if kind == 0u { return false; }
    var lo_r = s_row; var lo_c = s_col; var hi_r = e_row; var hi_c = e_col;
    let swap = (s_row > e_row) || (s_row == e_row && s_col > e_col);
    if swap {
        lo_r = e_row; lo_c = e_col; hi_r = s_row; hi_c = s_col;
    }
    if kind == 2u {
        return row >= lo_r && row <= hi_r;
    }
    if row < lo_r || row > hi_r { return false; }
    if row == lo_r && col < lo_c { return false; }
    if row == hi_r && col > hi_c { return false; }
    return true;
}

fn unpack_rgba(p: u32) -> vec4<f32> {
    return vec4<f32>(
        f32(p & 0xFFu),
        f32((p >> 8u) & 0xFFu),
        f32((p >> 16u) & 0xFFu),
        f32((p >> 24u) & 0xFFu),
    ) / 255.0;
}

@fragment
fn fragment(in: UiVertexOutput) -> @location(0) vec4<f32> {
    // rev 4: out-of-grid fragments are painted with the bg_padding_color
    // uniform (set by Rust-side TerminalParams::new) so the strip between
    // the grid's bottom-right edge and the host UI node edge matches the
    // terminal background. Replaces the old opaque-black fallback.
    let fallback = params.bg_padding_color;

    if params.grid_size.x == 0u || params.grid_size.y == 0u {
        return fallback;
    }

    // rev 4: shader runs entirely in PHYSICAL pixels. Bevy 0.18
    // UiVertexOutput.size is physical px (verified in spec R1 audit), so
    // we use it directly without dpr_inv conversion.
    let p_px = in.uv * in.size;
    let cell_pitch_px = params.cell_size_px;

    // Out-of-grid right/bottom padding (the strip between
    // grid_size * cell_size_px and the host UI node edge).
    let grid_pixel_w = cell_pitch_px.x * f32(params.grid_size.x);
    let grid_pixel_h = cell_pitch_px.y * f32(params.grid_size.y);
    if p_px.x >= grid_pixel_w || p_px.y >= grid_pixel_h {
        return fallback;
    }

    let col_f = p_px.x / cell_pitch_px.x;
    let row_f = p_px.y / cell_pitch_px.y;
    if col_f < 0.0 || row_f < 0.0 {
        return fallback;
    }
    let col = u32(floor(col_f));
    let row = u32(floor(row_f));
    if col >= params.grid_size.x || row >= params.grid_size.y {
        return fallback;
    }

    let idx = row * params.grid_size.x + col;
    if idx >= arrayLength(&cells) {
        return fallback;
    }
    let cell = cells[idx];

    let style = cell.style_flags;
    let reverse = (style & STYLE_REVERSE) != 0u;
    let hidden = (style & STYLE_HIDDEN) != 0u;
    let dim = (style & STYLE_DIM) != 0u;

    var fg = unpack_rgba(cell.fg_packed);
    var bg = unpack_rgba(cell.bg_packed);

    if hidden {
        fg = bg;
    }
    if reverse {
        let tmp = fg;
        fg = bg;
        bg = tmp;
    }
    if dim {
        fg = vec4<f32>(fg.rgb * 0.66, fg.a);
    }

    var color = bg;

    let cell_top_left = vec2<f32>(f32(col), f32(row)) * cell_pitch_px;
    let in_cell_px = p_px - cell_top_left;

    // rev 4: WIDE_RIGHT_HALF cells render the LEFT half's glyph but
    // anchored to the left-half's origin (i.e. shifted +cell_pitch_px.x
    // into "this cell" coordinates). This makes the wide glyph render
    // continuously across both cells.
    let cur_is_wide_right = (cell.style_flags & STYLE_WIDE_RIGHT_HALF) != 0u;
    let in_cell_px_eff = select(
        in_cell_px,
        in_cell_px + vec2<f32>(cell_pitch_px.x, 0.0),
        cur_is_wide_right,
    );

    // Primary glyph (this cell's own glyph_index, evaluated against
    // in_cell_px_eff for wide-right-half cells).
    if cell.glyph_index != GLYPH_NONE && cell.glyph_index < arrayLength(&glyphs) {
        let glyph = glyphs[cell.glyph_index];
        // Glyph origin in cell-local PHYSICAL px. params.ascent_px shifts
        // down to the baseline, then glyph.offset_px.y lifts back up to
        // the bitmap top.
        let glyph_origin = vec2<f32>(
            glyph.offset_px.x,
            params.ascent_px + glyph.offset_px.y,
        );
        let glyph_origin_phys = floor(glyph_origin + vec2<f32>(0.5, 0.5));
        let glyph_local = in_cell_px_eff - glyph_origin_phys;
        if glyph_local.x >= 0.0 && glyph_local.y >= 0.0 &&
            glyph_local.x < glyph.size_px.x && glyph_local.y < glyph.size_px.y {
            let atlas_px = mix(glyph.uv_min, glyph.uv_max, glyph_local / glyph.size_px);
            let atlas_uv = atlas_px / params.atlas_size_px;
            // NOTE: `textureSampleLevel` (explicit mip 0) instead of
            //       `textureSample` — the latter auto-computes derivatives
            //       and requires uniform control flow; WebGPU rejects it
            //       inside per-fragment branches like this one.
            let coverage = textureSampleLevel(atlas_tex, atlas_sampler, atlas_uv, 0.0).a;
            color = mix(color, vec4<f32>(fg.rgb, max(fg.a, coverage)), coverage);
        }
    }

    // rev 4: glyph overdraw from the LEFT neighbor (for fonts where
    // bbox.width > h_advance, e.g. JetBrains Mono `W`). Skip when:
    //   (a) current cell is WIDE_RIGHT_HALF (already handled above), or
    //   (b) left neighbor is WIDE_RIGHT_HALF (its glyph_index points to a
    //       wide glyph that Change 4 already paints into the right-half
    //       via in_cell_px_eff; re-evaluating here would double-paint).
    if col >= 1u && !cur_is_wide_right {
        let left_idx = row * params.grid_size.x + (col - 1u);
        let left_cell = cells[left_idx];
        let left_is_wide_right = (left_cell.style_flags & STYLE_WIDE_RIGHT_HALF) != 0u;
        if !left_is_wide_right
            && left_cell.glyph_index != GLYPH_NONE
            && left_cell.glyph_index < arrayLength(&glyphs) {
            let left_glyph = glyphs[left_cell.glyph_index];
            let left_glyph_origin = vec2<f32>(
                left_glyph.offset_px.x,
                params.ascent_px + left_glyph.offset_px.y,
            );
            let left_origin_phys = floor(left_glyph_origin + vec2<f32>(0.5, 0.5));
            // Left neighbor's cell-local coord = my in_cell_px + one cell to the left.
            let left_glyph_local = in_cell_px + vec2<f32>(cell_pitch_px.x, 0.0) - left_origin_phys;
            if left_glyph_local.x >= 0.0 && left_glyph_local.y >= 0.0
                && left_glyph_local.x < left_glyph.size_px.x
                && left_glyph_local.y < left_glyph.size_px.y {
                let atlas_px = mix(left_glyph.uv_min, left_glyph.uv_max,
                    left_glyph_local / left_glyph.size_px);
                let atlas_uv = atlas_px / params.atlas_size_px;
                let coverage = textureSampleLevel(atlas_tex, atlas_sampler, atlas_uv, 0.0).a;
                let left_fg = unpack_rgba(left_cell.fg_packed);
                color = mix(color, vec4<f32>(left_fg.rgb, max(left_fg.a, coverage)), coverage);
            }
        }
    }

    // rev 4: underline / strike use font-derived metrics from uniform
    // rather than hardcoded cell_pitch.y - 1 / cell_pitch.y * 0.5.
    let style_underline = (style & STYLE_UNDERLINE) != 0u;
    let style_strike = (style & STYLE_STRIKE) != 0u;
    if style_underline {
        // underline_position_phys is negative (below baseline). The actual
        // y in the cell is baseline + |underline_position|.
        let underline_y_top = params.ascent_px - params.underline_position_phys;
        let underline_y_bot = underline_y_top + params.underline_thickness_phys;
        if in_cell_px.y >= underline_y_top && in_cell_px.y < underline_y_bot {
            color = vec4<f32>(fg.rgb, max(color.a, fg.a));
        }
    }
    if style_strike {
        // Strike sits at ~half of ascent, with same thickness as underline.
        let strike_y_top = params.ascent_px * 0.5 - params.underline_thickness_phys * 0.5;
        let strike_y_bot = strike_y_top + params.underline_thickness_phys;
        if in_cell_px.y >= strike_y_top && in_cell_px.y < strike_y_bot {
            color = vec4<f32>(fg.rgb, max(color.a, fg.a));
        }
    }

    let cursor_visible = (params.cursor_style & CURSOR_VISIBLE) != 0u;
    let cursor_blinking = (params.cursor_style & CURSOR_BLINKING) != 0u;
    let cursor_shape = (params.cursor_style >> 1u) & 3u;
    let blink_on = !cursor_blinking || (fract(params.time_seconds) < 0.5);
    if cursor_visible && blink_on && col == params.cursor_pos.x && row == params.cursor_pos.y {
        let thickness = 2.0;
        let invert = vec4<f32>(1.0 - color.rgb, color.a);
        if cursor_shape == CURSOR_SHAPE_BLOCK {
            color = invert;
        } else if cursor_shape == CURSOR_SHAPE_UNDERLINE {
            if in_cell_px.y >= cell_pitch_px.y - thickness {
                color = invert;
            }
        } else if cursor_shape == CURSOR_SHAPE_BAR {
            if in_cell_px.x < thickness {
                color = invert;
            }
        }
    }

    if params.sel_kind != 0u {
        let in_sel = is_in_selection_uniform(
            i32(row), i32(col),
            params.sel_kind,
            params.sel_start_row, i32(params.sel_start_col),
            params.sel_end_row, i32(params.sel_end_col),
        );
        if in_sel {
            let sel_bg = vec4<f32>(0.27, 0.50, 0.66, 0.4);
            color = mix(color, sel_bg, sel_bg.a);
        }
    }

    return color;
}
