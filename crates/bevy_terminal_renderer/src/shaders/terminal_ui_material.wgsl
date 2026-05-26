
#import bevy_ui::ui_vertex_output::UiVertexOutput

// ============================================================================
// Module data
// ============================================================================

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
    max_overflow_phys: f32,
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

struct CellHit {
    valid: bool,
    row: u32,
    col: u32,
    cell: Cell,
    // Fragment position relative to the cell's top-left, in physical px.
    in_cell_px: vec2<f32>,
};

struct CellColors {
    fg: vec4<f32>,
    bg: vec4<f32>,
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

// ============================================================================
// Fragment entrypoint
// ============================================================================

@fragment
fn fragment(in: UiVertexOutput) -> @location(0) vec4<f32> {
    // Out-of-grid fragments (degenerate grid, or the right/bottom padding
    // strip) fall back to bg_padding_color so the surrounding band blends
    // with the terminal background instead of opaque black.
    let fallback = params.bg_padding_color;
    if params.grid_size.x == 0u || params.grid_size.y == 0u {
        return fallback;
    }

    // Shader runs entirely in PHYSICAL pixels. Bevy 0.18 UiVertexOutput.size
    // is physical px (verified in spec R1 audit), so we use it directly.
    let p_px = in.uv * in.size;
    let hit = locate_cell(p_px);
    if !hit.valid {
        return paint_right_strip(p_px, fallback);
    }
    return paint_grid_cell(hit, fallback);
}

// ============================================================================
// Top-level pipeline stages
// ============================================================================

// Pipeline for a fragment that lies inside the grid: background → primary
// glyph → left-neighbor overdraw → text decorations → cursor → selection.
fn paint_grid_cell(hit: CellHit, fallback: vec4<f32>) -> vec4<f32> {
    let colors = resolve_cell_colors(hit.cell);
    var color = colors.bg;
    color = paint_primary_glyph(hit, colors.fg, color);
    color = paint_left_overdraw(hit, color);
    color = paint_text_decorations(hit.cell.style_flags, hit.in_cell_px, colors.fg, color);
    color = paint_cursor(hit.row, hit.col, hit.in_cell_px, color);
    color = paint_selection(hit.row, hit.col, color);
    return color;
}

// Handles fragments past the grid's right edge: when they fall within the
// `max_overflow_phys` band reserved by the host, paint the rightmost cell's
// bbox overflow. Falls back to `fallback` outside the band or on a miss.
fn paint_right_strip(p_px: vec2<f32>, fallback: vec4<f32>) -> vec4<f32> {
    let grid_w_phys = params.cell_size_px.x * f32(params.grid_size.x);
    let grid_h_phys = params.cell_size_px.y * f32(params.grid_size.y);
    let in_right_strip = p_px.x >= grid_w_phys
        && p_px.x < grid_w_phys + params.max_overflow_phys
        && p_px.y < grid_h_phys;
    if !in_right_strip {
        return fallback;
    }

    let col = params.grid_size.x - 1u;
    let row = u32(floor(p_px.y / params.cell_size_px.y));
    let idx = row * params.grid_size.x + col;
    if idx >= arrayLength(&cells) {
        return fallback;
    }

    let strip_cell = cells[idx];
    let strip_local = vec2<f32>(
        p_px.x - f32(col) * params.cell_size_px.x,
        p_px.y - f32(row) * params.cell_size_px.y,
    );
    let strip_fg = resolve_cell_colors(strip_cell).fg;
    return paint_cell_glyph(strip_cell, strip_local, strip_fg, fallback);
}

// ============================================================================
// Cell glyph stages (primary + left overdraw)
// ============================================================================

// Paints the cell's own glyph. WIDE_RIGHT_HALF cells render the LEFT half's
// glyph anchored to the left-half origin — i.e. shifted +cell_pitch.x into
// "this cell" coordinates — so the wide glyph spans both cells.
fn paint_primary_glyph(hit: CellHit, fg: vec4<f32>, base: vec4<f32>) -> vec4<f32> {
    let cur_is_wide_right = (hit.cell.style_flags & STYLE_WIDE_RIGHT_HALF) != 0u;
    let primary_local = select(
        hit.in_cell_px,
        hit.in_cell_px + vec2<f32>(params.cell_size_px.x, 0.0),
        cur_is_wide_right,
    );
    return paint_cell_glyph(hit.cell, primary_local, fg, base);
}

// Paints any overflow pixels from the LEFT neighbor's glyph — needed for
// fonts where bbox.width > h_advance (e.g. JetBrains Mono `W`). Skipped when:
//   (a) we're in column 0 (no left neighbour), or
//   (b) the current cell is WIDE_RIGHT_HALF (already painted via primary), or
//   (c) the left neighbour is WIDE_RIGHT_HALF (its glyph_index points to a
//       wide glyph already painted by paint_primary_glyph on the right half;
//       re-evaluating here would double-paint).
fn paint_left_overdraw(hit: CellHit, base: vec4<f32>) -> vec4<f32> {
    if hit.col == 0u {
        return base;
    }
    let cur_is_wide_right = (hit.cell.style_flags & STYLE_WIDE_RIGHT_HALF) != 0u;
    if cur_is_wide_right {
        return base;
    }
    let left_idx = hit.row * params.grid_size.x + (hit.col - 1u);
    let left_cell = cells[left_idx];
    let left_is_wide_right = (left_cell.style_flags & STYLE_WIDE_RIGHT_HALF) != 0u;
    if left_is_wide_right {
        return base;
    }
    let left_local = hit.in_cell_px + vec2<f32>(params.cell_size_px.x, 0.0);
    let left_fg = resolve_cell_colors(left_cell).fg;
    return paint_cell_glyph(left_cell, left_local, left_fg, base);
}

// ============================================================================
// Overlay stages (decorations / cursor / selection)
// ============================================================================

// Underline metrics come from font-derived uniforms; strike sits at half the
// ascent and reuses the underline thickness.
fn paint_text_decorations(
    style: u32,
    in_cell_px: vec2<f32>,
    fg: vec4<f32>,
    base: vec4<f32>,
) -> vec4<f32> {
    var color = base;
    if (style & STYLE_UNDERLINE) != 0u {
        // underline_position_phys is negative (below baseline). The actual
        // y in the cell is baseline + |underline_position|.
        let y_top = params.ascent_px - params.underline_position_phys;
        let y_bot = y_top + params.underline_thickness_phys;
        if in_cell_px.y >= y_top && in_cell_px.y < y_bot {
            color = vec4<f32>(fg.rgb, max(color.a, fg.a));
        }
    }
    if (style & STYLE_STRIKE) != 0u {
        let y_top = params.ascent_px * 0.5 - params.underline_thickness_phys * 0.5;
        let y_bot = y_top + params.underline_thickness_phys;
        if in_cell_px.y >= y_top && in_cell_px.y < y_bot {
            color = vec4<f32>(fg.rgb, max(color.a, fg.a));
        }
    }
    return color;
}

fn paint_cursor(
    row: u32,
    col: u32,
    in_cell_px: vec2<f32>,
    base: vec4<f32>,
) -> vec4<f32> {
    let cursor_visible = (params.cursor_style & CURSOR_VISIBLE) != 0u;
    let cursor_blinking = (params.cursor_style & CURSOR_BLINKING) != 0u;
    let cursor_shape = (params.cursor_style >> 1u) & 3u;
    let blink_on = !cursor_blinking || (fract(params.time_seconds) < 0.5);
    let on_cursor_cell = col == params.cursor_pos.x && row == params.cursor_pos.y;
    if !(cursor_visible && blink_on && on_cursor_cell) {
        return base;
    }

    let thickness = 2.0;
    let invert = vec4<f32>(1.0 - base.rgb, base.a);
    if cursor_shape == CURSOR_SHAPE_BLOCK {
        return invert;
    }
    if cursor_shape == CURSOR_SHAPE_UNDERLINE
        && in_cell_px.y >= params.cell_size_px.y - thickness {
        return invert;
    }
    if cursor_shape == CURSOR_SHAPE_BAR && in_cell_px.x < thickness {
        return invert;
    }
    return base;
}

fn paint_selection(row: u32, col: u32, base: vec4<f32>) -> vec4<f32> {
    if params.sel_kind == 0u {
        return base;
    }
    let in_sel = is_in_selection_uniform(
        i32(row), i32(col),
        params.sel_kind,
        params.sel_start_row, i32(params.sel_start_col),
        params.sel_end_row, i32(params.sel_end_col),
    );
    if !in_sel {
        return base;
    }
    let sel_bg = vec4<f32>(0.27, 0.50, 0.66, 0.4);
    return mix(base, sel_bg, sel_bg.a);
}

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

// ============================================================================
// Cell lookup
// ============================================================================

// Resolves the fragment's physical-px position to a grid cell. Returns
// `valid = false` when the fragment lies in the right/bottom padding strip
// between `grid_size * cell_size_px` and the host UI node edge, or when the
// cell index would overflow the `cells` storage buffer.
fn locate_cell(p_px: vec2<f32>) -> CellHit {
    let invalid = CellHit(false, 0u, 0u, Cell(0u, 0u, 0u, 0u), vec2<f32>(0.0, 0.0));
    let cell_pitch = params.cell_size_px;
    let grid_w_px = cell_pitch.x * f32(params.grid_size.x);
    let grid_h_px = cell_pitch.y * f32(params.grid_size.y);
    if p_px.x >= grid_w_px || p_px.y >= grid_h_px {
        return invalid;
    }
    let col_f = p_px.x / cell_pitch.x;
    let row_f = p_px.y / cell_pitch.y;
    if col_f < 0.0 || row_f < 0.0 {
        return invalid;
    }
    let col = u32(floor(col_f));
    let row = u32(floor(row_f));
    if col >= params.grid_size.x || row >= params.grid_size.y {
        return invalid;
    }
    let idx = row * params.grid_size.x + col;
    if idx >= arrayLength(&cells) {
        return invalid;
    }
    let cell_origin = vec2<f32>(f32(col), f32(row)) * cell_pitch;
    return CellHit(true, row, col, cells[idx], p_px - cell_origin);
}

// ============================================================================
// Style resolution (reverse / hidden / dim)
// ============================================================================

fn resolve_cell_colors(cell: Cell) -> CellColors {
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
    return CellColors(fg, bg);
}

// ============================================================================
// Glyph painting (low-level)
// ============================================================================

// Paints `cell`'s glyph into `base`, evaluating against `local_px` (the
// fragment's cell-local coord). Returns `base` unchanged when the cell has
// no glyph or the index is out of range.
fn paint_cell_glyph(
    cell: Cell,
    local_px: vec2<f32>,
    fg: vec4<f32>,
    base: vec4<f32>,
) -> vec4<f32> {
    if cell.glyph_index == GLYPH_NONE || cell.glyph_index >= arrayLength(&glyphs) {
        return base;
    }
    let glyph = glyphs[cell.glyph_index];
    let glyph_local = local_px - glyph_origin_phys(glyph);
    let coverage = sample_glyph_coverage(glyph, glyph_local);
    return blend_glyph(base, fg, coverage);
}

// Glyph origin in cell-local PHYSICAL px, snapped to integer pixels.
// `params.ascent_px` shifts down to the baseline; `glyph.offset_px.y` then
// lifts back up to the bitmap top.
fn glyph_origin_phys(glyph: Glyph) -> vec2<f32> {
    let origin = vec2<f32>(
        glyph.offset_px.x,
        params.ascent_px + glyph.offset_px.y,
    );
    return floor(origin + vec2<f32>(0.5, 0.5));
}

// Atlas alpha coverage at `local_px` (cell-local px relative to the glyph's
// snapped origin). Returns 0.0 when outside the glyph bitmap so callers can
// blend unconditionally.
//
// NOTE: `textureSampleLevel` (explicit mip 0) instead of `textureSample` —
//       the latter auto-computes derivatives and requires uniform control
//       flow; WebGPU rejects it inside per-fragment branches.
fn sample_glyph_coverage(glyph: Glyph, local_px: vec2<f32>) -> f32 {
    if local_px.x < 0.0 || local_px.y < 0.0
        || local_px.x >= glyph.size_px.x || local_px.y >= glyph.size_px.y {
        return 0.0;
    }
    let atlas_px = mix(glyph.uv_min, glyph.uv_max, local_px / glyph.size_px);
    let atlas_uv = atlas_px / params.atlas_size_px;
    return textureSampleLevel(atlas_tex, atlas_sampler, atlas_uv, 0.0).a;
}

fn blend_glyph(base: vec4<f32>, fg: vec4<f32>, coverage: f32) -> vec4<f32> {
    return mix(base, vec4<f32>(fg.rgb, max(fg.a, coverage)), coverage);
}

// ============================================================================
// Utilities
// ============================================================================

fn unpack_rgba(p: u32) -> vec4<f32> {
    return vec4<f32>(
        f32(p & 0xFFu),
        f32((p >> 8u) & 0xFFu),
        f32((p >> 16u) & 0xFFu),
        f32((p >> 24u) & 0xFFu),
    ) / 255.0;
}
