
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
    hover_hyperlink_id: u32,
    hover_active: u32,
    dim: f32,
    inactive_tint: vec4<f32>,
    overlay_rects: array<vec4<i32>, 4>,
    overlay_dim: f32,
    overlay_desaturate: f32,
};

struct Cell {
    glyph_index: u32,
    fg_packed: u32,
    bg_packed: u32,
    style_flags: u32,
    hyperlink_id: u32,
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
@group(1) @binding(5) var overlay0_tex: texture_2d<f32>;
@group(1) @binding(6) var overlay0_samp: sampler;
@group(1) @binding(7) var overlay1_tex: texture_2d<f32>;
@group(1) @binding(8) var overlay1_samp: sampler;
@group(1) @binding(9) var overlay2_tex: texture_2d<f32>;
@group(1) @binding(10) var overlay2_samp: sampler;
@group(1) @binding(11) var overlay3_tex: texture_2d<f32>;
@group(1) @binding(12) var overlay3_samp: sampler;

// NOTE: Must stay in sync with `ozmux_terminal_protocol::style::*`. The
//       Rust-side test `style_bits_match_protocol_constants` asserts the
//       literal values here against the canonical Rust constants.
// Hyperlink accent color when the activation modifier is held and the
// cell shares the hovered link's id. Hardcoded for v1.
const ACCENT_LINK_COLOR: vec4<f32> = vec4<f32>(0.4, 0.7, 1.0, 1.0);

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

// Blends a BACKGROUND color toward the inactive-pane tint target. `rgb`
// blends toward `params.inactive_tint.rgb` by `params.inactive_tint.a`; alpha
// is preserved. Active pane => `inactive_tint.a == 0.0` (no-op). Applied only
// at background-establishment points (before glyphs/overlays paint), so text
// and webview overlays keep their full color. Runs in LINEAR space —
// `inactive_tint.rgb` is uploaded pre-linearized by the host.
fn tint_bg(c: vec4<f32>) -> vec4<f32> {
    // NOTE: alpha=0 means transparent (terminal default bg sentinel); preserve
    // the zero vector so blend_premultiplied_over does not add phantom RGB.
    if c.a == 0.0 { return c; }
    return vec4<f32>(mix(c.rgb, params.inactive_tint.rgb, params.inactive_tint.a), c.a);
}

@fragment
fn fragment(in: UiVertexOutput) -> @location(0) vec4<f32> {
    // Out-of-grid fragments (degenerate grid, or the right/bottom padding
    // strip) fall back to bg_padding_color so the surrounding band blends
    // with the terminal background instead of opaque black. The padding is a
    // background region, so it receives the inactive-pane tint too.
    let fallback = tint_bg(params.bg_padding_color);

    var color: vec4<f32>;
    if params.grid_size.x == 0u || params.grid_size.y == 0u {
        color = fallback;
    } else {
        // Shader runs entirely in PHYSICAL pixels. Bevy 0.18
        // UiVertexOutput.size is physical px (verified in spec R1 audit), so
        // we use it directly.
        let p_px = in.uv * in.size;
        let hit = locate_cell(p_px);
        if !hit.valid {
            color = paint_right_strip(p_px, fallback);
        } else {
            color = paint_grid_cell(hit, fallback);
        }
    }

    // Pane-level brightness: active pane => params.dim == 1.0 (no-op); inactive
    // pane => params.dim <= 1.0. RGB only; alpha is preserved so blending and
    // the opaque-padding contract are unchanged. The inactive-pane background
    // tint is applied earlier (tint_bg, at the background stage). Composes with
    // the per-cell SGR STYLE_DIM independently.
    return vec4<f32>(color.rgb * params.dim, color.a);
}

// ============================================================================
// Top-level pipeline stages
// ============================================================================

// Pipeline for a fragment that lies inside the grid: pane background →
// inline overlays → cell background → primary glyph → left-neighbor
// overdraw → text decorations → cursor → selection.
//
// The pane background (fallback) is the base layer. Webview overlays
// composite over it next. The cell's own background then composites OVER the
// overlay result: a transparent cell background (the terminal default) lets
// the webview show through, while an opaque cell background (set by a TUI
// widget) occludes the webview and appears in front of it. Glyphs render
// last, on top of everything.
fn paint_grid_cell(hit: CellHit, fallback: vec4<f32>) -> vec4<f32> {
    let colors = resolve_cell_colors(hit.cell);
    var color = paint_inline_overlays(hit, fallback);
    color = blend_premultiplied_over(color, tint_bg(colors.bg));
    color = paint_primary_glyph(hit, colors.fg, color);
    color = paint_left_overdraw(hit, color);
    return paint_cell_overlays(hit, colors.fg, color);
}

// Handles fragments past the grid's right edge: when they fall within the
// `max_overflow_phys` band reserved by the host, paint the rightmost cell's
// bbox overflow. Falls back to `fallback` outside the band or on a miss.
fn paint_right_strip(p_px: vec2<f32>, fallback: vec4<f32>) -> vec4<f32> {
    // Defensive guard (#10): cell_size_px is Vec2::ZERO during the
    // ~1-frame window between MaterialNode insertion and the first
    // update_terminal_material write. The grid_h_phys = 0 check below
    // already prevents the strip-entry, but the explicit cell_size_px
    // guard documents the invariant and protects against future
    // refactors that might remove the height check.
    let grid_w_phys = params.cell_size_px.x * f32(params.grid_size.x);
    let grid_h_phys = params.cell_size_px.y * f32(params.grid_size.y);
    let in_right_strip = params.cell_size_px.x > 0.0
        && params.cell_size_px.y > 0.0
        && p_px.x >= grid_w_phys
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
    let colors = resolve_cell_colors(strip_cell);
    var color = blend_premultiplied_over(fallback, tint_bg(colors.bg));
    // NOTE: paint_cell_glyph (NOT paint_primary_glyph). strip_local.x is
    // in [cell_pitch.x, cell_pitch.x + max_overflow_phys) — already in
    // the right-half coordinate space of any STYLE_WIDE_RIGHT_HALF wide
    // glyph. paint_primary_glyph would add another +cell_pitch.x, pushing
    // past the wide bitmap (size_px.x ≈ 2*cell_pitch.x) → zero coverage.
    // The asymmetry vs paint_grid_cell is intentional and unavoidable.
    color = paint_cell_glyph(strip_cell, strip_local, colors.fg, color);
    let hit = CellHit(true, row, col, strip_cell, strip_local);
    return paint_cell_overlays(hit, colors.fg, color);
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

// Runs the three overlay stages in canonical order:
// text decorations → cursor → selection. Used by both paint_grid_cell
// and paint_right_strip so the strip cannot drift from the grid path
// on overlay sequence or argument order.
fn paint_cell_overlays(hit: CellHit, fg: vec4<f32>, base: vec4<f32>) -> vec4<f32> {
    var color = paint_text_decorations(
        hit.cell.style_flags,
        hit.in_cell_px,
        fg,
        base,
        hit.cell.hyperlink_id,
    );
    color = paint_cursor(hit.row, hit.col, hit.in_cell_px, color);
    color = paint_selection(hit.row, hit.col, color);
    return color;
}

// Underline metrics come from font-derived uniforms; strike sits at half the
// ascent and reuses the underline thickness.
fn paint_text_decorations(
    style: u32,
    in_cell_px: vec2<f32>,
    fg: vec4<f32>,
    base: vec4<f32>,
    cell_hyperlink_id: u32,
) -> vec4<f32> {
    var color = base;
    var underline_painted = false;
    if cell_hyperlink_id != 0u {
        let is_hovered =
            params.hover_active != 0u &&
            cell_hyperlink_id == params.hover_hyperlink_id;
        let underline_color = select(fg, ACCENT_LINK_COLOR, is_hovered);
        let y_top = params.ascent_px - params.underline_position_phys;
        let y_bot = y_top + params.underline_thickness_phys;
        if in_cell_px.y >= y_top && in_cell_px.y < y_bot {
            color = vec4<f32>(underline_color.rgb, max(color.a, underline_color.a));
        }
        underline_painted = true;
    }
    if (style & STYLE_UNDERLINE) != 0u && !underline_painted {
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
// Inline-overlay compositing
// ============================================================================

// Applies the inactive-pane treatment to one overlay (webview) sample before it
// blends over the background: desaturate toward Rec.709 luminance, then dim.
// `s` is premultiplied-alpha and linear, so both are correct on `s.rgb`
// (luma(a*c) = a*luma(c); a scalar multiply distributes through premultiply).
// Active pane => overlay_dim == 1.0 && overlay_desaturate == 0.0 (no-op).
fn treat_overlay(s: vec4<f32>) -> vec4<f32> {
    let luma = dot(s.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
    let desat = mix(s.rgb, vec3<f32>(luma), params.overlay_desaturate);
    return vec4<f32>(desat * params.overlay_dim, s.a);
}

// Inline-overlay compositing (spec §6.2): samples each ACTIVE overlay slot
// whose cell-rect contains this fragment and composites it OVER the pane
// background (`base`, which is `fallback` from `paint_grid_cell`). Source
// is premultiplied alpha (CEF convention, spec §6.3); the sRGB texture view
// linearizes on read, so values mix in linear space with no manual
// conversion. The cell's own background composites OVER this result in
// `paint_grid_cell`, so an opaque widget background occludes the webview.
// Glyphs paint last, so terminal text sits on top of a webview.
//
// uv derives from the UNCLIPPED rect (rect.x may be negative when the rect
// is partially scrolled above the viewport), so partial visibility never
// distorts the image; grid-edge clipping is inherent because only in-grid
// fragments reach paint_grid_cell.

fn paint_inline_overlays(hit: CellHit, base: vec4<f32>) -> vec4<f32> {
    var color = base;
    let p_px = vec2<f32>(f32(hit.col), f32(hit.row)) * params.cell_size_px + hit.in_cell_px;
    // NOTE: bindings cannot be dynamically indexed in core WGSL — the four
    // slots are unrolled by hand; keep slot order identical to the Rust
    // `set_overlays` field order or textures swap silently.
    {
        let uv = overlay_uv(params.overlay_rects[0], p_px, hit);
        if uv.x >= 0.0 {
            let s = treat_overlay(textureSampleLevel(overlay0_tex, overlay0_samp, uv, 0.0));
            color = blend_premultiplied_over(color, s);
        }
    }
    {
        let uv = overlay_uv(params.overlay_rects[1], p_px, hit);
        if uv.x >= 0.0 {
            let s = treat_overlay(textureSampleLevel(overlay1_tex, overlay1_samp, uv, 0.0));
            color = blend_premultiplied_over(color, s);
        }
    }
    {
        let uv = overlay_uv(params.overlay_rects[2], p_px, hit);
        if uv.x >= 0.0 {
            let s = treat_overlay(textureSampleLevel(overlay2_tex, overlay2_samp, uv, 0.0));
            color = blend_premultiplied_over(color, s);
        }
    }
    {
        let uv = overlay_uv(params.overlay_rects[3], p_px, hit);
        if uv.x >= 0.0 {
            let s = treat_overlay(textureSampleLevel(overlay3_tex, overlay3_samp, uv, 0.0));
            color = blend_premultiplied_over(color, s);
        }
    }
    return color;
}

// Returns the overlay-local uv for `rect = (row, col, rows, cols)` at the
// fragment's grid position, or vec2(-1.0) when the slot is inactive
// (rows == 0) or the fragment's CELL lies outside the rect. The hit test is
// cell-quantized (placeholder-cell semantics); the uv itself is pixel-exact
// against the unclipped rect.
fn overlay_uv(rect: vec4<i32>, p_px: vec2<f32>, hit: CellHit) -> vec2<f32> {
    let miss = vec2<f32>(-1.0, -1.0);
    if rect.z == 0 {
        return miss;
    }
    let row = i32(hit.row);
    let col = i32(hit.col);
    if row < rect.x || row >= rect.x + rect.z || col < rect.y || col >= rect.y + rect.w {
        return miss;
    }
    let origin_px = vec2<f32>(f32(rect.y), f32(rect.x)) * params.cell_size_px;
    let size_px = vec2<f32>(f32(rect.w), f32(rect.z)) * params.cell_size_px;
    return (p_px - origin_px) / size_px;
}

// Premultiplied-alpha OVER in linear space (src premultiplied per CEF; dst is
// the terminal cell background, opaque in practice).
fn blend_premultiplied_over(dst: vec4<f32>, src: vec4<f32>) -> vec4<f32> {
    let inv = 1.0 - src.a;
    return vec4<f32>(src.rgb + dst.rgb * inv, src.a + dst.a * inv);
}

// ============================================================================
// Cell lookup
// ============================================================================

// Resolves the fragment's physical-px position to a grid cell. Returns
// `valid = false` when the fragment lies in the right/bottom padding strip
// between `grid_size * cell_size_px` and the host UI node edge, or when the
// cell index would overflow the `cells` storage buffer.
fn locate_cell(p_px: vec2<f32>) -> CellHit {
    let invalid = CellHit(false, 0u, 0u, Cell(0u, 0u, 0u, 0u, 0u), vec2<f32>(0.0, 0.0));
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
        // NOTE: The terminal default bg maps to transparent (alpha=0) so that
        // cells without an explicit background let webview overlays show through.
        // When reverse-video promotes that transparent sentinel to the glyph
        // foreground color, materialise it as opaque black so text stays visible.
        if fg.a == 0.0 {
            fg = vec4<f32>(0.0, 0.0, 0.0, 1.0);
        }
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
    // NOTE: multiply the blend factor by fg.a so that a transparent fg
    // (STYLE_HIDDEN with transparent default bg) produces no ink at all.
    // For opaque fg (fg.a == 1.0) this is a no-op; coverage drives blending
    // as before.
    return mix(base, vec4<f32>(fg.rgb, max(fg.a, coverage)), coverage * fg.a);
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
