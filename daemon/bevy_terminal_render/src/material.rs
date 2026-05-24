use crate::{
    glyph::{
        atlas::{GlyphAtlas, GlyphRect},
        font::{FontFace, GlyphKey, TerminalFonts},
    },
    material::state::TerminalMaterialState,
    schema::{Cell, SelectionKind, TerminalGrid},
};
use bevy::{
    asset::{load_internal_asset, uuid_handle},
    prelude::*,
    render::{
        render_resource::{AsBindGroup, ShaderType},
        storage::ShaderStorageBuffer,
    },
    shader::ShaderRef,
};

mod state;

const TERMINAL_SHADER_HANDLE: Handle<Shader> = uuid_handle!("98195199-3092-42b6-b370-77dfc2ef83f9");

/// Registers the custom material and embeds its WGSL shader into the binary.
#[derive(Default)]
pub struct TerminalMaterialPlugin;

impl Plugin for TerminalMaterialPlugin {
    fn build(&self, app: &mut App) {
        load_internal_asset!(
            app,
            TERMINAL_SHADER_HANDLE,
            "shaders/terminal_ui_material.wgsl",
            Shader::from_wgsl
        );
        app.add_plugins(UiMaterialPlugin::<TerminalUiMaterial>::default())
            .add_systems(Update, update_terminal_material);
    }
}

#[derive(Debug, Clone, Component)]
pub struct TerminalUiMaterialHandle(pub Handle<TerminalUiMaterial>);

/// Custom UI material backing the full-screen terminal node.
#[derive(AsBindGroup, Asset, TypePath, Clone)]
pub struct TerminalUiMaterial {
    #[uniform(0)]
    params: TerminalParams,
    #[storage(1, read_only)]
    cells: Handle<ShaderStorageBuffer>,
    #[storage(2, read_only)]
    glyphs: Handle<ShaderStorageBuffer>,
    #[texture(3)]
    #[sampler(4)]
    atlas: Handle<Image>,
}

impl UiMaterial for TerminalUiMaterial {
    fn fragment_shader() -> ShaderRef {
        TERMINAL_SHADER_HANDLE.into()
    }
}

/// Uniform block uploaded once per frame alongside the storage buffers.
///
/// # Invariants
///
/// "No cursor" is encoded by clearing the `CURSOR_VISIBLE` bit in
/// `cursor_style` (and leaving `cursor_pos` at any value). The shader
/// short-circuits on `cursor_visible == 0u`, so we deliberately keep
/// `cursor_pos` as `UVec2` rather than introducing a signed sentinel —
/// the existing visibility bit already does that job. The vi cursor in
/// scrollback uses the same path: `cursor_visible = 0`.
#[derive(Clone, Copy, ShaderType, Default, Debug)]
struct TerminalParams {
    grid_size: UVec2,
    cell_size_px: Vec2,
    atlas_size_px: Vec2,
    ascent_px: f32,
    dpr: f32,
    cursor_pos: UVec2,
    /// Packed: bit0=visible, bits1-2=shape (0=block / 1=underline / 2=bar), bit3=blinking.
    cursor_style: u32,
    time_seconds: f32,
    /// Selection start row in viewport coords; `i32` because endpoints in
    /// scrollback clamp to `-1` (above) or `rows` (below).
    sel_start_row: i32,
    sel_start_col: u32,
    sel_end_row: i32,
    sel_end_col: u32,
    /// 0 = none, 1 = char, 2 = line. See `SelectionKind` in the wire protocol.
    sel_kind: u32,
    /// Pad to a 16-byte multiple for WebGPU's std140 uniform layout.
    _pad: u32,
}

impl TerminalParams {
    /// Builds the per-frame uniform block from the current grid + frame timing.
    ///
    /// # Invariants
    ///
    /// - When `grid.vi_cursor` is present and inside the viewport, it
    ///   overrides `grid.cursor` and the resulting `cursor_visible` bit is
    ///   forced to `1`. When the vi cursor is in scrollback, `cursor_visible`
    ///   is cleared so the shader skips cursor rendering entirely.
    /// - When `grid.selection` is `None`, `sel_kind == 0` and the shader's
    ///   `is_in_selection_uniform` short-circuits to `false`.
    fn new(
        grid: &TerminalGrid,
        cell_size_px: Vec2,
        atlas_size_px: Vec2,
        ascent_px: f32,
        dpr: f32,
        time_seconds: f32,
    ) -> Self {
        let cols = u32::from(grid.cols);
        let rows = u32::from(grid.rows);

        let (cursor_pos, cursor_style) = grid.current_cursor_pos_and_style();
        let (sel_start_row, sel_start_col, sel_end_row, sel_end_col, sel_kind) =
            match grid.selection {
                Some(sel) => (
                    i32::from(sel.start.row),
                    u32::from(sel.start.column),
                    i32::from(sel.end.row),
                    u32::from(sel.end.column),
                    match sel.kind {
                        SelectionKind::Char => 1u32,
                        SelectionKind::Line => 2,
                    },
                ),
                None => (0, 0, 0, 0, 0),
            };

        Self {
            grid_size: UVec2::new(cols.max(1), rows.max(1)),
            cell_size_px,
            atlas_size_px,
            ascent_px,
            dpr,
            cursor_pos,
            cursor_style,
            time_seconds,
            sel_start_row,
            sel_start_col,
            sel_end_row,
            sel_end_col,
            sel_kind,
            _pad: 0,
        }
    }
}

/// One GPU-side cell — 16 bytes, indexed `row * cols + col` in the storage buffer.
#[derive(Clone, Copy, ShaderType, Debug)]
struct GpuCell {
    /// Index into the glyph LUT, or `u32::MAX` for an empty cell (space / blank).
    glyph_index: u32,
    /// `0xAABBGGRR` packed foreground.
    fg_packed: u32,
    /// `0xAABBGGRR` packed background.
    bg_packed: u32,
    /// Mirror of `ozmux_terminal_protocol::style::*` bit flags.
    style_flags: u32,
}

impl Default for GpuCell {
    // NOTE: glyph_index defaults to u32::MAX (GLYPH_NONE) — the shader's
    //       sentinel for "no glyph". A naive zero would collide with whatever
    //       real glyph occupies LUT index 0 and paint stray characters into
    //       every uninitialized cell.
    fn default() -> Self {
        Self {
            glyph_index: u32::MAX,
            fg_packed: 0,
            bg_packed: 0,
            style_flags: 0,
        }
    }
}

/// Per-glyph atlas record used by the fragment shader.
#[derive(Clone, Copy, ShaderType, Default, Debug)]
struct GpuGlyph {
    /// Top-left of the glyph rect in atlas physical px.
    uv_min: Vec2,
    /// Bottom-right of the glyph rect in atlas physical px.
    uv_max: Vec2,
    /// Bearing from the glyph origin in logical px (positive Y goes down).
    offset_px: Vec2,
    /// Rasterized bitmap size in logical px.
    size_px: Vec2,
}

impl GpuGlyph {
    pub fn new(rect: GlyphRect, dpr: f32) -> Self {
        Self {
            uv_min: Vec2::new(rect.u as f32, rect.v as f32),
            uv_max: Vec2::new((rect.u + rect.w) as f32, (rect.v + rect.h) as f32),
            offset_px: Vec2::new(rect.offset_x as f32 / dpr, rect.offset_y as f32 / dpr),
            size_px: Vec2::new(rect.w as f32 / dpr, rect.h as f32 / dpr),
        }
    }
}

fn update_terminal_material(
    mut atlas: ResMut<GlyphAtlas>,
    mut materials: ResMut<Assets<TerminalUiMaterial>>,
    mut buffers: ResMut<Assets<ShaderStorageBuffer>>,
    mut terminals: Query<
        (
            &TerminalUiMaterialHandle,
            &mut TerminalMaterialState,
            &TerminalGrid,
        ),
        Changed<TerminalGrid>,
    >,
    fonts: Res<TerminalFonts>,
    palette_time: Res<Time>,
    windows: Query<&Window>,
) {
    //TODO: 設定ファイルからロードするようにする。
    const FONT_SIZE_PX: f32 = 12.0;
    for (handle, mut state, grid) in terminals.iter_mut() {
        let dpr = windows.single().map(|w| w.scale_factor()).unwrap_or(1.0);
        let phys_font_size = (FONT_SIZE_PX * dpr).round() as u16;
        let ascent_logical = (fonts.ascent_px(phys_font_size) / dpr).round();

        // NOTE: atlas.generation can advance during this very system (via
        //       get_or_insert), and a generation jump means the atlas pixel
        //       buffer was wiped — every cached glyph index in cpu_cells is
        //       now stale and would resolve to garbage texels. Clearing the
        //       LUT here forces a full rerasterization on the rebuild path.
        let atlas_invalidated = atlas.generation != state.last_atlas_generation;
        if atlas_invalidated {
            state.glyph_index_map.clear();
            state.cpu_glyphs.clear();
        }

        let cols = grid.cols as u32;
        let rows = grid.rows as u32;
        let dims_changed = (grid.cols, grid.rows) != state.last_grid_dims;
        let grid_changed = grid.last_seq != state.last_grid_seq;
        let needs_rebuild = !state.initialized || grid_changed || atlas_invalidated || dims_changed;

        let Some((cells_handle, glyphs_handle)) =
            materials.get(&handle.0).map(|m| (m.cells.clone(), m.glyphs.clone()))
        else {
            continue;
        };

        if needs_rebuild {
            let cell_count = (cols * rows) as usize;
            state.cpu_cells.clear();
            state.cpu_cells.resize(cell_count, GpuCell::default());

            if cols > 0 && rows > 0 {
                rebuild_cells(
                    grid,
                    &mut state,
                    &fonts,
                    &mut atlas,
                    phys_font_size,
                    dpr,
                    cols,
                );
            }

            if state.cpu_cells.is_empty() {
                state.cpu_cells.push(GpuCell::default());
            }
            if state.cpu_glyphs.is_empty() {
                state.cpu_glyphs.push(GpuGlyph::default());
            }

            if let Some(buf) = buffers.get_mut(&cells_handle) {
                buf.set_data(std::mem::take(&mut state.cpu_cells));
            }
            if let Some(buf) = buffers.get_mut(&glyphs_handle) {
                buf.set_data(state.cpu_glyphs.clone());
            }

            state.last_atlas_generation = atlas.generation;
            state.last_grid_seq = grid.last_seq;
            state.last_grid_dims = (grid.cols, grid.rows);
            state.initialized = true;
        }

        const CELL_W: f32 = 8.0;
        /// Logical pixel height of one terminal cell.
        const CELL_H: f32 = 16.0;
        if let Some(mat) = materials.get_mut(&handle.0) {
            mat.params = TerminalParams::new(
                grid,
                Vec2::new(CELL_W, CELL_H),
                Vec2::new(atlas.width() as f32, atlas.height() as f32),
                ascent_logical,
                dpr,
                palette_time.elapsed_secs(),
            );
        }
    }
}

fn rebuild_cells(
    grid: &TerminalGrid,
    state: &mut TerminalMaterialState,
    fonts: &TerminalFonts,
    atlas: &mut GlyphAtlas,
    phys_font_size: u16,
    dpr: f32,
    cols: u32,
) {
    for (row_idx, row) in grid.cells.iter().enumerate() {
        let mut col: u32 = 0;
        for cell in row {
            if col >= cols {
                break;
            }
            // NOTE: width=0 cells are combining-mark grapheme clusters that
            //       wire emits as a separate Cell (Run boundary lands inside
            //       a cluster). They must not consume a column or write a
            //       GPU slot — otherwise we get phantom dark boxes between
            //       characters carrying the base cell's style flags.
            if cell.width == 0 {
                continue;
            }
            let cell_width = u32::from(cell.width);
            let glyph_index = resolve_glyph_index(cell, state, fonts, atlas, phys_font_size, dpr);
            let fg = cell.fg.to_linear().as_u32();
            let bg = cell.bg.to_linear().as_u32();
            let style_flags = u32::from(cell.style) | style_bits_from_combining_marks(&cell.text);
            let target = (row_idx as u32 * cols + col) as usize;
            if let Some(slot) = state.cpu_cells.get_mut(target) {
                *slot = GpuCell {
                    glyph_index,
                    fg_packed: fg,
                    bg_packed: bg,
                    style_flags,
                };
            }
            col = col.saturating_add(cell_width);
        }
    }
}

/// TODO: 以下のようなスタイにも対応できるようにする
///
///  - U+0301 (鋭アクセント á)
///  - U+0303 (チルダ ã)
///  - U+0308 (ウムラウト ä)
///  - U+20D7 (上向きベクトル a⃗)
///
///
/// Promotes specific combining marks in a grapheme cluster to terminal-style
/// underline/strike bits so the shader paints them, since `ab_glyph` cannot
/// composite combining glyphs onto the base char in Tier 1.
///
/// Maps U+0332 (combining low line), U+0333 (double low line), U+0331
/// (combining macron below) to `style::UNDERLINE`, and U+0336 (combining
/// long stroke overlay) to `style::STRIKE`. Other combining marks are
/// ignored — the base glyph still renders.
fn style_bits_from_combining_marks(text: &str) -> u32 {
    // ASCII bytes (< 0x80) can never be combining marks (those live above
    // U+0300, i.e. multi-byte UTF-8). is_ascii is SIMD-vectorized and skips
    // the per-char decode for the dominant case in a typical terminal frame.
    if text.is_ascii() {
        return 0;
    }
    const UNDERLINE: u32 = 4;
    const STRIKE: u32 = 8;
    let mut bits = 0u32;
    for c in text.chars() {
        match c {
            '\u{0332}' | '\u{0333}' | '\u{0331}' => bits |= UNDERLINE,
            '\u{0336}' => bits |= STRIKE,
            _ => {}
        }
    }
    bits
}

fn resolve_glyph_index(
    cell: &Cell,
    state: &mut TerminalMaterialState,
    fonts: &TerminalFonts,
    atlas: &mut GlyphAtlas,
    phys_font_size: u16,
    dpr: f32,
) -> u32 {
    if cell.width == 0 || cell.text.is_empty() || cell.text.trim().is_empty() {
        return u32::MAX;
    }
    let codepoint = cell.text.chars().next().map(|c| c as u32).unwrap_or(0);
    if codepoint == 0 || codepoint == 0x20 {
        return u32::MAX;
    }
    let face = FontFace::from_style(cell.style);
    let key = GlyphKey {
        face,
        codepoint,
        size_px: phys_font_size,
    };
    let Some(rect) = atlas.get_or_insert(key, fonts) else {
        return u32::MAX;
    };
    if let Some(&idx) = state.glyph_index_map.get(&key) {
        return idx;
    }
    let idx = state.cpu_glyphs.len() as u32;
    state.cpu_glyphs.push(GpuGlyph::new(rect, dpr));
    state.glyph_index_map.insert(key, idx);
    idx
}
