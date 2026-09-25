
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
    cursor_thickness_phys: f32,
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
    overlay_rects: array<vec4<i32>, 12>,
    overlay_dim: f32,
    overlay_desaturate: f32,
    cursor_packed: u32,
    default_fg_packed: u32,
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
@group(1) @binding(5) var overlay_samp: sampler;
@group(1) @binding(6) var overlay0_tex: texture_2d<f32>;
@group(1) @binding(7) var overlay1_tex: texture_2d<f32>;
@group(1) @binding(8) var overlay2_tex: texture_2d<f32>;
@group(1) @binding(9) var overlay3_tex: texture_2d<f32>;
@group(1) @binding(10) var overlay4_tex: texture_2d<f32>;
@group(1) @binding(11) var overlay5_tex: texture_2d<f32>;
@group(1) @binding(12) var overlay6_tex: texture_2d<f32>;
@group(1) @binding(13) var overlay7_tex: texture_2d<f32>;
@group(1) @binding(14) var overlay8_tex: texture_2d<f32>;
@group(1) @binding(15) var overlay9_tex: texture_2d<f32>;
@group(1) @binding(16) var overlay10_tex: texture_2d<f32>;
@group(1) @binding(17) var overlay11_tex: texture_2d<f32>;

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
const CURSOR_HOLLOW: u32 = 16u;
const CURSOR_SHAPE_BLOCK: u32 = 0u;
const CURSOR_SHAPE_UNDERLINE: u32 = 1u;
const CURSOR_SHAPE_BAR: u32 = 2u;

const GLYPH_NONE: u32 = 0xFFFFFFFFu;

// ============================================================================
// Fragment entrypoint
// ============================================================================

// Blends a BACKGROUND color toward the inactive-pane tint target. `rgb`
// blends toward `params.inactive_tint.rgb` by `params.inactive_tint.a`; alpha
// is preserved. Active pane => `inactive_tint.a == 0.0` (no-op). Applied at
// the background-establishment points (before glyphs/overlays paint), so text
// and webview overlays keep their full color, and to the glyph of a concealed
// cell, which must match the tinted ground it hides in. Runs in LINEAR space —
// `inactive_tint.rgb` is uploaded pre-linearized by the host.
fn tint_bg(c: vec4<f32>) -> vec4<f32> {
    // NOTE: alpha=0 means transparent (terminal default bg sentinel); preserve
    // the zero vector so blend_premultiplied_over does not add phantom RGB.
    if c.a == 0.0 { return c; }
    return vec4<f32>(mix(c.rgb, params.inactive_tint.rgb, params.inactive_tint.a), c.a);
}

@fragment
fn fragment(in: UiVertexOutput) -> @location(0) vec4<f32> {
    // Shader runs entirely in PHYSICAL pixels. Bevy 0.18
    // UiVertexOutput.size is physical px (verified in spec R1 audit), so
    // we use it directly.
    return dim_pane(paint_pane(in.uv * in.size));
}

// The color of the fragment at `p_px`, before the pane-level dim.
//
// Out-of-grid fragments (degenerate grid, or the right/bottom padding
// strip) fall back to bg_padding_color so the surrounding band blends
// with the terminal background instead of opaque black. The padding is a
// background region, so it receives the inactive-pane tint too.
fn paint_pane(p_px: vec2<f32>) -> vec4<f32> {
    let fallback = tint_bg(params.bg_padding_color);
    if params.grid_size.x == 0u || params.grid_size.y == 0u {
        return fallback;
    }
    let hit = locate_cell(p_px);
    if !hit.valid {
        return paint_right_strip(p_px, fallback);
    }
    return paint_grid_cell(hit, fallback);
}

// Pane-level brightness: active pane => params.dim == 1.0 (no-op); inactive
// pane => params.dim <= 1.0. RGB only; alpha is preserved so blending and
// the opaque-padding contract are unchanged. The inactive-pane background
// tint is applied earlier (tint_bg, at the background stage). Composes with
// the per-cell SGR STYLE_DIM independently.
fn dim_pane(color: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(color.rgb * params.dim, color.a);
}

// ============================================================================
// Top-level pipeline stages
// ============================================================================

// Pipeline for a fragment that lies inside the grid: pane background →
// inline overlays → cell background → primary glyph → left-neighbor
// overdraw → text decorations → bar / underline cursor → selection. A block
// cursor is not a stage: it is resolved into the cell's colors up front.
//
// The pane background (fallback) is the base layer. Webview overlays
// composite over it next. The cell's own background then composites OVER the
// overlay result: a transparent cell background (the terminal default) lets
// the webview show through, while an opaque cell background (set by a TUI
// widget) occludes the webview and appears in front of it. Glyphs render
// last, on top of everything.
fn paint_grid_cell(hit: CellHit, fallback: vec4<f32>) -> vec4<f32> {
    let colors = resolve_painted_colors(hit.cell, hit.row, hit.col);
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
    if !in_right_strip(p_px) {
        return fallback;
    }

    let col = params.grid_size.x - 1u;
    let row = u32(floor(p_px.y / params.cell_size_px.y));
    let idx = cell_index(row, col);
    if idx >= arrayLength(&cells) {
        return fallback;
    }

    let strip_cell = cells[idx];
    let strip_local = p_px - cell_origin_px(row, col);
    let colors = resolve_painted_colors(strip_cell, row, col);
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

// Whether `p_px` lies in the band past the grid's right edge that the host
// reserves for the last column's glyph overflow.
fn in_right_strip(p_px: vec2<f32>) -> bool {
    // Defensive guard (#10): cell_size_px is Vec2::ZERO during the
    // ~1-frame window between MaterialNode insertion and the first
    // write_terminal_params write. The grid-height check below
    // already prevents the strip-entry, but the explicit cell_size_px
    // guard documents the invariant and protects against future
    // refactors that might remove the height check.
    let grid_px = grid_extent_px();
    return params.cell_size_px.x > 0.0
        && params.cell_size_px.y > 0.0
        && p_px.x >= grid_px.x
        && p_px.x < grid_px.x + params.max_overflow_phys
        && p_px.y < grid_px.y;
}

// ============================================================================
// Cell glyph stages (primary + left overdraw)
// ============================================================================

// Paints the cell's own glyph. WIDE_RIGHT_HALF cells render the LEFT half's
// glyph anchored to the left-half origin — i.e. shifted +cell_pitch.x into
// "this cell" coordinates — so the wide glyph spans both cells.
fn paint_primary_glyph(hit: CellHit, fg: vec4<f32>, base: vec4<f32>) -> vec4<f32> {
    let primary_local = select(
        hit.in_cell_px,
        in_left_neighbor_px(hit.in_cell_px),
        is_wide_right_half(hit.cell.style_flags),
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
    if is_wide_right_half(hit.cell.style_flags) {
        return base;
    }
    let left_cell = cells[cell_index(hit.row, hit.col - 1u)];
    if is_wide_right_half(left_cell.style_flags) {
        return base;
    }
    let left_local = in_left_neighbor_px(hit.in_cell_px);
    let left_fg = resolve_cell_colors(left_cell).fg;
    return paint_cell_glyph(left_cell, left_local, left_fg, base);
}

// The fragment's position in the cell-local px of the cell to its left.
fn in_left_neighbor_px(in_cell_px: vec2<f32>) -> vec2<f32> {
    return in_cell_px + vec2<f32>(params.cell_size_px.x, 0.0);
}

// ============================================================================
// Overlay stages (decorations / cursor / selection)
// ============================================================================

// Runs the three overlay stages in canonical order:
// text decorations → bar / underline cursor → selection. Used by both
// paint_grid_cell and paint_right_strip so the strip cannot drift from the
// grid path on overlay sequence or argument order.
fn paint_cell_overlays(hit: CellHit, fg: vec4<f32>, base: vec4<f32>) -> vec4<f32> {
    var color = paint_text_decorations(
        hit.cell.style_flags,
        hit.in_cell_px,
        fg,
        base,
        hit.cell.hyperlink_id,
    );
    color = paint_cursor(hit.row, hit.col, hit.in_cell_px, hit.cell, color);
    color = paint_selection(hit.row, hit.col, color);
    return color;
}

// Paints the decoration lines the cell's style and link ask for: the
// underline, then the strike line.
fn paint_text_decorations(
    style: u32,
    in_cell_px: vec2<f32>,
    fg: vec4<f32>,
    base: vec4<f32>,
    cell_hyperlink_id: u32,
) -> vec4<f32> {
    let color = paint_underline(style, in_cell_px.y, fg, base, cell_hyperlink_id);
    return paint_strike(style, in_cell_px.y, fg, color);
}

// Paints the underline of an underlined or hyperlinked cell at the cell-local
// `y`, in the accent color while the cell's link is hovered. The underline
// metrics come from font-derived uniforms.
fn paint_underline(
    style: u32,
    y: f32,
    fg: vec4<f32>,
    base: vec4<f32>,
    cell_hyperlink_id: u32,
) -> vec4<f32> {
    if cell_hyperlink_id == 0u && (style & STYLE_UNDERLINE) == 0u {
        return base;
    }
    // underline_position_phys is negative (below baseline). The actual
    // y in the cell is baseline + |underline_position|.
    let top = params.ascent_px - params.underline_position_phys;
    if !in_band(y, top, params.underline_thickness_phys) {
        return base;
    }
    let line_color = select(fg, ACCENT_LINK_COLOR, is_hovered_link(cell_hyperlink_id));
    return paint_line(base, line_color);
}

// Paints the strike line of a struck-through cell at the cell-local `y`.
// The line sits at half the ascent and reuses the underline thickness.
fn paint_strike(style: u32, y: f32, fg: vec4<f32>, base: vec4<f32>) -> vec4<f32> {
    if (style & STYLE_STRIKE) == 0u {
        return base;
    }
    let top = params.ascent_px * 0.5 - params.underline_thickness_phys * 0.5;
    if !in_band(y, top, params.underline_thickness_phys) {
        return base;
    }
    return paint_line(base, fg);
}

// Whether the cell belongs to the hyperlink the pointer hovers while the
// activation modifier is held.
fn is_hovered_link(cell_hyperlink_id: u32) -> bool {
    return cell_hyperlink_id != 0u
        && params.hover_active != 0u
        && cell_hyperlink_id == params.hover_hyperlink_id;
}

// Whether `y` lies in the horizontal band `thickness` tall whose top edge
// is `top`.
fn in_band(y: f32, top: f32, thickness: f32) -> bool {
    return y >= top && y < top + thickness;
}

// `base` under a decoration line painted in `line_color`.
fn paint_line(base: vec4<f32>, line_color: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(line_color.rgb, max(base.a, line_color.a));
}

// Whether the cursor covers (row, col): the cursor's own cell, the right
// half of a wide glyph whose body holds the cursor, or the body of a wide
// glyph whose right half holds the cursor.
fn cursor_covers(row: u32, col: u32) -> bool {
    if row != params.cursor_pos.y {
        return false;
    }
    if col == params.cursor_pos.x {
        return true;
    }
    if col == params.cursor_pos.x + 1u && wide_right_half_at(row, col) {
        return true;
    }
    return col + 1u == params.cursor_pos.x && wide_right_half_at(row, params.cursor_pos.x);
}

// Whether the cursor sits on the right half of a wide glyph.
fn cursor_on_wide_right_half() -> bool {
    return wide_right_half_at(params.cursor_pos.y, params.cursor_pos.x);
}

// The column holding the caret's left edge: the body cell when the cursor
// sits on a wide glyph's right half.
fn cursor_span_left() -> u32 {
    if cursor_on_wide_right_half() && params.cursor_pos.x > 0u {
        return params.cursor_pos.x - 1u;
    }
    return params.cursor_pos.x;
}

// The column holding the caret's right edge: the right half when the
// cursor sits on a wide glyph's body.
fn cursor_span_right() -> u32 {
    let next = params.cursor_pos.x + 1u;
    if next < params.grid_size.x && wide_right_half_at(params.cursor_pos.y, next) {
        return next;
    }
    return params.cursor_pos.x;
}

// Whether a bar cursor is drawn in (row, col): the cursor's own cell, or
// the body cell when the cursor sits on a wide glyph's right half.
fn bar_covers(row: u32, col: u32) -> bool {
    return row == params.cursor_pos.y && col == cursor_span_left();
}

// Whether the cursor is drawn this frame. The blink phase is decided on
// the CPU, which packs an invisible style on every dark phase.
fn cursor_is_lit() -> bool {
    return (params.cursor_style & CURSOR_VISIBLE) != 0u;
}

// The caret shape the cursor style packs: one of the `CURSOR_SHAPE_*`
// constants.
fn cursor_shape() -> u32 {
    return (params.cursor_style >> 1u) & 3u;
}

// Whether the caret is drawn as a hollow outline rather than filled.
fn cursor_is_hollow() -> bool {
    return (params.cursor_style & CURSOR_HOLLOW) != 0u;
}

// Whether a lit, filled block cursor covers (row, col). Runs for every
// fragment, so the checks that read no cell come first. A hollow caret is
// drawn as an outline instead, so it never takes over the cell's colors.
fn block_cursor_covers(row: u32, col: u32) -> bool {
    return row == params.cursor_pos.y
        && cursor_shape() == CURSOR_SHAPE_BLOCK
        && !cursor_is_hollow()
        && cursor_is_lit()
        && cursor_covers(row, col);
}

// The color the cursor is painted in: the OSC 12 color when one is set,
// else `cell_fg`.
fn cursor_fill(cell_fg: vec4<f32>) -> vec4<f32> {
    if params.cursor_packed != 0u {
        return unpack_rgba(params.cursor_packed);
    }
    return cell_fg;
}

// The Rec.709 luminance of a linear color.
fn luminance(rgb: vec3<f32>) -> f32 {
    return dot(rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
}

// The WCAG contrast ratio of two linear colors, from 1.0 for equal
// luminance upward.
fn contrast_ratio(a: vec3<f32>, b: vec3<f32>) -> f32 {
    let la = luminance(a);
    let lb = luminance(b);
    return (max(la, lb) + 0.05) / (min(la, lb) + 0.05);
}

// The contrast a cursor fill must have against the ground it is painted
// on; below it the fill is swapped for a default color.
const MIN_CURSOR_CONTRAST: f32 = 1.5;

// `fill` made opaque, or, when it would not stand out against `ground`,
// whichever of the default foreground and background stands out more.
fn guarded_fill(fill: vec4<f32>, ground: vec4<f32>) -> vec4<f32> {
    if contrast_ratio(fill.rgb, ground.rgb) >= MIN_CURSOR_CONTRAST {
        return vec4<f32>(fill.rgb, 1.0);
    }
    let default_fg = unpack_rgba(params.default_fg_packed).rgb;
    let default_bg = materialize_default_bg(vec4<f32>(0.0)).rgb;
    let fg_stands_out = contrast_ratio(default_fg, ground.rgb) >= contrast_ratio(default_bg, ground.rgb);
    return vec4<f32>(select(default_bg, default_fg, fg_stands_out), 1.0);
}

// The colors (row, col) is painted in: the cell's own, or, under a lit
// block cursor, the colors under_block_cursor turns them into.
// Concealment applies last, so a concealed glyph stays hidden inside the
// block.
fn resolve_painted_colors(cell: Cell, row: u32, col: u32) -> CellColors {
    var colors = resolve_visible_colors(cell);
    if block_cursor_covers(row, col) {
        colors = under_block_cursor(colors);
    }
    return conceal(cell, colors);
}

// `colors` under a lit block cursor: the guarded fill as background and
// the cell's background as the glyph color. A glyph that does not stand
// out from that background takes the fill instead, so a full block of
// one color still shows the cursor.
fn under_block_cursor(colors: CellColors) -> CellColors {
    let ground = materialize_default_bg(colors.bg);
    let fill = guarded_fill(cursor_fill(colors.fg), ground);
    let glyph_melts = contrast_ratio(colors.fg.rgb, ground.rgb) < MIN_CURSOR_CONTRAST;
    return CellColors(select(ground, fill, glyph_melts), fill);
}

// Paints the cursors drawn as strokes clear of the glyph: the bar, the
// underline, and the hollow outline an unfocused pane takes. The filled
// block cursor is resolved into the cell's colors instead.
fn paint_cursor(
    row: u32,
    col: u32,
    in_cell_px: vec2<f32>,
    cell: Cell,
    base: vec4<f32>,
) -> vec4<f32> {
    // NOTE: stroked_cursor_covers reads the cell buffer, so this uniform
    // test must stay ahead of it: a blinking caret packs an invisible
    // style on every dark phase, and without the early return every
    // fragment pays those loads.
    if !cursor_is_lit() {
        return base;
    }
    if !stroked_cursor_covers(row, col) || !on_cursor_stroke(col, in_cell_px) {
        return base;
    }
    return cursor_stroke_fill(cell);
}

// Whether a stroked caret is drawn in (row, col): a bar in the cell holding
// its left edge, any other shape in every cell the cursor covers.
fn stroked_cursor_covers(row: u32, col: u32) -> bool {
    // NOTE: cursor_shape() reads only the uniform, so this branch is
    // wave-uniform and runs one arm; `select` would evaluate both, and
    // each arm reads the cell buffer.
    if cursor_shape() == CURSOR_SHAPE_BAR {
        return bar_covers(row, col);
    }
    return cursor_covers(row, col);
}

// Whether the fragment at `in_cell_px` of column `col` lies on the caret's
// stroke: the hollow outline, the underline, or the bar. A filled block
// has no stroke.
fn on_cursor_stroke(col: u32, in_cell_px: vec2<f32>) -> bool {
    // NOTE: paint_right_strip reaches this with in_cell_px.x past the cell
    // width, so every branch that is not already bounded on x must test
    // this or its stroke strays outside the cell.
    let inside_cell = in_cell_px.x < params.cell_size_px.x;
    if cursor_is_hollow() {
        return inside_cell && on_hollow_outline(col, in_cell_px);
    }
    let thickness = params.cursor_thickness_phys;
    if cursor_shape() == CURSOR_SHAPE_UNDERLINE {
        return inside_cell && in_cell_px.y >= params.cell_size_px.y - thickness;
    }
    return cursor_shape() == CURSOR_SHAPE_BAR && in_cell_px.x < thickness;
}

// Whether the fragment at `in_cell_px` of column `col` lies on the hollow
// caret's outline. The outline wraps both halves of a wide pair as one
// box, so the edges between the halves are left out.
fn on_hollow_outline(col: u32, in_cell_px: vec2<f32>) -> bool {
    let thickness = params.cursor_thickness_phys;
    // NOTE: thickness is a fraction of the cell WIDTH, so an outline
    // that took it unbounded would have its two opposite edges meet
    // and fill the cell — a hollow caret that reads as a filled one.
    // Each axis keeps its stroke under half of its own extent.
    let edge_x = min(thickness, (params.cell_size_px.x - 1.0) * 0.5);
    let edge_y = min(thickness, (params.cell_size_px.y - 1.0) * 0.5);
    // NOTE: the in-cell tests come first on purpose. `&&` short
    // circuits left to right, and the span helpers read the cell
    // buffer, so testing them first would pay that read for every
    // interior fragment the comparison then rejects.
    return in_cell_px.y < edge_y
        || in_cell_px.y >= params.cell_size_px.y - edge_y
        || (in_cell_px.x < edge_x && col == cursor_span_left())
        || (in_cell_px.x >= params.cell_size_px.x - edge_x
            && col == cursor_span_right());
}

// The color a stroked caret is painted in over `cell`: the guarded fill,
// taken from the cell's colors before concealment and compared against
// the tinted ground.
fn cursor_stroke_fill(cell: Cell) -> vec4<f32> {
    let visible = resolve_visible_colors(cell);
    let ground = tint_bg(materialize_default_bg(visible.bg));
    return guarded_fill(cursor_fill(visible.fg), ground);
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
    let luma = luminance(s.rgb);
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

fn sample_overlay_slot(
    rect: vec4<i32>,
    tex: texture_2d<f32>,
    samp: sampler,
    p_px: vec2<f32>,
    hit: CellHit,
    color: vec4<f32>,
) -> vec4<f32> {
    let uv = overlay_uv(rect, p_px, hit);
    if uv.x >= 0.0 {
        let s = treat_overlay(textureSampleLevel(tex, samp, uv, 0.0));
        return blend_premultiplied_over(color, s);
    }
    return color;
}

fn paint_inline_overlays(hit: CellHit, base: vec4<f32>) -> vec4<f32> {
    var color = base;
    let p_px = cell_origin_px(hit.row, hit.col) + hit.in_cell_px;
    color = sample_overlay_slot(params.overlay_rects[0], overlay0_tex, overlay_samp, p_px, hit, color);
    color = sample_overlay_slot(params.overlay_rects[1], overlay1_tex, overlay_samp, p_px, hit, color);
    color = sample_overlay_slot(params.overlay_rects[2], overlay2_tex, overlay_samp, p_px, hit, color);
    color = sample_overlay_slot(params.overlay_rects[3], overlay3_tex, overlay_samp, p_px, hit, color);
    color = sample_overlay_slot(params.overlay_rects[4], overlay4_tex, overlay_samp, p_px, hit, color);
    color = sample_overlay_slot(params.overlay_rects[5], overlay5_tex, overlay_samp, p_px, hit, color);
    color = sample_overlay_slot(params.overlay_rects[6], overlay6_tex, overlay_samp, p_px, hit, color);
    color = sample_overlay_slot(params.overlay_rects[7], overlay7_tex, overlay_samp, p_px, hit, color);
    color = sample_overlay_slot(params.overlay_rects[8], overlay8_tex, overlay_samp, p_px, hit, color);
    color = sample_overlay_slot(params.overlay_rects[9], overlay9_tex, overlay_samp, p_px, hit, color);
    color = sample_overlay_slot(params.overlay_rects[10], overlay10_tex, overlay_samp, p_px, hit, color);
    color = sample_overlay_slot(params.overlay_rects[11], overlay11_tex, overlay_samp, p_px, hit, color);
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
    let grid_px = grid_extent_px();
    if p_px.x >= grid_px.x || p_px.y >= grid_px.y {
        return invalid;
    }
    let col_f = p_px.x / params.cell_size_px.x;
    let row_f = p_px.y / params.cell_size_px.y;
    if col_f < 0.0 || row_f < 0.0 {
        return invalid;
    }
    let col = u32(floor(col_f));
    let row = u32(floor(row_f));
    if col >= params.grid_size.x || row >= params.grid_size.y {
        return invalid;
    }
    let idx = cell_index(row, col);
    if idx >= arrayLength(&cells) {
        return invalid;
    }
    return CellHit(true, row, col, cells[idx], p_px - cell_origin_px(row, col));
}

// The grid's extent in physical px.
fn grid_extent_px() -> vec2<f32> {
    return params.cell_size_px * vec2<f32>(params.grid_size);
}

// The top-left corner of cell (row, col) in physical px.
fn cell_origin_px(row: u32, col: u32) -> vec2<f32> {
    return vec2<f32>(f32(col), f32(row)) * params.cell_size_px;
}

// The index of cell (row, col) in the `cells` storage buffer.
fn cell_index(row: u32, col: u32) -> u32 {
    return row * params.grid_size.x + col;
}

// Whether `style` marks the right half of a wide glyph.
fn is_wide_right_half(style: u32) -> bool {
    return (style & STYLE_WIDE_RIGHT_HALF) != 0u;
}

// Whether the `cells` storage buffer holds (row, col) as the right half of
// a wide glyph. Only the style flags are read, and only when the index is
// in range.
fn wide_right_half_at(row: u32, col: u32) -> bool {
    let idx = cell_index(row, col);
    return idx < arrayLength(&cells) && is_wide_right_half(cells[idx].style_flags);
}

// ============================================================================
// Style resolution (reverse / dim, then concealment)
// ============================================================================

// The color the terminal default background paints: `c` itself, or the
// padding color when `c` is the transparent sentinel.
fn materialize_default_bg(c: vec4<f32>) -> vec4<f32> {
    // NOTE: The terminal default bg maps to transparent (alpha=0) so that
    // cells without an explicit background let webview overlays show through.
    // When that transparent sentinel is promoted to a glyph color, it must
    // be materialised as the colour the default background actually paints
    // (bg_padding_color). A hardcoded black here would paint the glyph black
    // once OSC 11 recolors the default background, which is unreadable on a
    // dark cell.
    if c.a == 0.0 {
        return vec4<f32>(params.bg_padding_color.rgb, 1.0);
    }
    return c;
}

// Reverse video and dim applied; concealment is not.
fn resolve_visible_colors(cell: Cell) -> CellColors {
    var colors = CellColors(unpack_rgba(cell.fg_packed), unpack_rgba(cell.bg_packed));
    if (cell.style_flags & STYLE_REVERSE) != 0u {
        colors = reverse_video(colors);
    }
    if (cell.style_flags & STYLE_DIM) != 0u {
        colors = faint(colors);
    }
    return colors;
}

// `colors` swapped for reverse video: the glyph takes the color the
// background paints, and the background takes the glyph's color.
fn reverse_video(colors: CellColors) -> CellColors {
    return CellColors(materialize_default_bg(colors.bg), colors.fg);
}

// `colors` with the glyph faded for faint (STYLE_DIM) text.
fn faint(colors: CellColors) -> CellColors {
    return CellColors(vec4<f32>(colors.fg.rgb * 0.66, colors.fg.a), colors.bg);
}

// `colors` with the glyph taking the color the background is painted in
// when the cell is concealed.
fn conceal(cell: Cell, colors: CellColors) -> CellColors {
    if (cell.style_flags & STYLE_HIDDEN) != 0u {
        // NOTE: The ground is painted through tint_bg, so the glyph must take
        // the tinted color as well. The untinted background would leave the
        // concealed text readable in an inactive pane, where the tint moves
        // the ground away from it. tint_bg keeps the transparent sentinel, so
        // a concealed glyph on the default background still paints no ink.
        return CellColors(tint_bg(colors.bg), colors.bg);
    }
    return colors;
}

// The colors the cell is painted in when no cursor covers it.
fn resolve_cell_colors(cell: Cell) -> CellColors {
    return conceal(cell, resolve_visible_colors(cell));
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
