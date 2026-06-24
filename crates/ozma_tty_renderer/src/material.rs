use crate::{
    glyph::{
        atlas::{GlyphAtlas, GlyphRect},
        font::{FontFace, GlyphKey, TerminalCellMetricsResource, TerminalFontSize, TerminalFonts},
    },
    material::state::TerminalMaterialState,
    schema::{Cell, HyperlinkHoverState, SelectionKind, TerminalGrid},
};
use bevy::{
    asset::{load_internal_asset, uuid_handle},
    prelude::*,
    render::{
        render_resource::{AsBindGroup, ShaderType},
        storage::ShaderStorageBuffer,
    },
    shader::ShaderRef,
    window::PrimaryWindow,
};

mod state;

/// Render-side public SystemSet anchor for `update_terminal_material`.
///
/// `sync_atlas_image` in `glyph.rs` is ordered with
/// `.after(TerminalMaterialSystems::UpdateMaterial)` so that any glyphs
/// rasterized this frame are mirrored into the `Image` asset before the
/// next ExtractSchedule, keeping atlas pixels and material params in
/// lock-step with the bind group rebuild. Exposed `pub` so consumers
/// (e.g., ozmux's `resize_terminals_to_node`) can sequence
/// layout-driven grid resizes `.before(Self::UpdateMaterial)`.
#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone)]
pub enum TerminalMaterialSystems {
    UpdateMaterial,
}

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
            .add_plugins(state::TerminalMaterialStatePlugin)
            // NOTE: Scheduled in `PostUpdate` (not `Update`) so it runs after
            // `ui_layout_system` has written the current frame's
            // `ComputedNode.size`. The downstream consumer
            // `resize_terminals_to_node` in ozmux depends on layout being
            // settled before terminal grid params propagate; keeping the
            // material write in the same schedule avoids a cross-frame split
            // where `grid_size`/`cell_size_px` lag layout by one tick.
            .add_systems(
                PostUpdate,
                update_terminal_material.in_set(TerminalMaterialSystems::UpdateMaterial),
            );
    }
}

/// Custom UI material backing the full-screen terminal node.
#[derive(AsBindGroup, Asset, TypePath, Clone, Default)]
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
    #[texture(5)]
    #[sampler(6)]
    overlay0: Option<Handle<Image>>,
    #[texture(7)]
    #[sampler(8)]
    overlay1: Option<Handle<Image>>,
    #[texture(9)]
    #[sampler(10)]
    overlay2: Option<Handle<Image>>,
    #[texture(11)]
    #[sampler(12)]
    overlay3: Option<Handle<Image>>,
}

impl UiMaterial for TerminalUiMaterial {
    fn fragment_shader() -> ShaderRef {
        TERMINAL_SHADER_HANDLE.into()
    }
}

impl TerminalUiMaterial {
    /// Copies the slot-indexed overlay handles onto the per-slot material
    /// fields. The ONLY place that maps slot index -> `overlay<i>` field.
    fn set_overlays(&mut self, textures: &[Option<Handle<Image>>; OVERLAY_SLOTS]) {
        self.overlay0 = textures[0].clone();
        self.overlay1 = textures[1].clone();
        self.overlay2 = textures[2].clone();
        self.overlay3 = textures[3].clone();
    }
}

/// Per-pane inactive-pane treatment for the terminal renderer: a background
/// `tint` (rgb = target color in LINEAR space, `a` = blend amount) and a
/// brightness `dim`. The shader blends each background source toward `tint.rgb`
/// by `tint.a` before glyphs/overlays paint (background only), then multiplies
/// the final color by `dim`. The consumer (e.g. ozmux) sets this on a
/// terminal host from its active-pane state; `update_terminal_material` bakes
/// both into the uniform each frame. An absent component is treated as
/// `{ dim: 1.0, tint: ZERO }` (full-bright, untinted / active).
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct PaneInactiveStyle {
    /// Brightness multiplier in `0.0..=1.0`; `1.0` = full-bright.
    pub dim: f32,
    /// Background tint: rgb = target color (linear), `a` = blend amount in
    /// `0.0..=1.0` (`0.0` = no tint / active).
    pub tint: Vec4,
    /// Inline-overlay (webview) brightness multiplier in `0.0..=1.0`; `1.0` =
    /// full-bright. Applied to overlay samples only, independent of `tint`.
    pub overlay_dim: f32,
    /// Inline-overlay (webview) desaturation in `0.0..=1.0`; `0.0` = full color,
    /// `1.0` = grey.
    pub overlay_desaturate: f32,
}

impl Default for PaneInactiveStyle {
    fn default() -> Self {
        Self {
            dim: 1.0,
            tint: Vec4::ZERO,
            overlay_dim: 1.0,
            overlay_desaturate: 0.0,
        }
    }
}

/// Number of inline-overlay texture slots on `TerminalUiMaterial`.
///
/// Slot index = array index = `overlay<i>` field order (NOT the WGSL binding
/// number). Hard upper bound per terminal surface (spec §6.1).
pub const OVERLAY_SLOTS: usize = 4;

/// Per-terminal overlay placements + textures, derived every frame by the
/// consumer (e.g. ozmux's webview projection) and consumed by
/// `update_terminal_material` — the only material mutation site. The renderer
/// knows nothing about what the textures contain.
///
/// # Invariants
///
/// - `rects[i]` is `(row, col, rows, cols)` in CELL coordinates; `row` may be
///   negative (rect partially above the viewport). `rows == 0` is the
///   inactive-slot sentinel — the shader skips the slot; the renderer binds
///   `textures[i]` regardless, so consumers should set freed slots to `None`
///   (the rebuild-every-frame contract below does this naturally).
/// - Consumers must rebuild this component from live state every frame
///   (all-sentinel start), so stale texture handles cannot outlive their
///   producers (spec §5).
#[derive(Component, Clone, Debug)]
pub struct TerminalOverlays {
    /// Placement rects, slot-indexed: `(row, col, rows, cols)` in cells.
    pub rects: [IVec4; OVERLAY_SLOTS],
    /// Texture handles, slot-indexed; `None` for inactive slots.
    pub textures: [Option<Handle<Image>>; OVERLAY_SLOTS],
}

impl Default for TerminalOverlays {
    fn default() -> Self {
        Self {
            rects: [IVec4::ZERO; OVERLAY_SLOTS],
            textures: [const { None }; OVERLAY_SLOTS],
        }
    }
}

/// Uniform block uploaded once per frame alongside the storage buffers.
///
/// # Invariants
///
/// - All `_phys` fields are PHYSICAL pixels (no DPR division). The shader
///   computes everything in physical-px space; `dpr` is provided for
///   diagnostic purposes only (currently unused in shader after Tier 1
///   `dpr_inv` removal).
/// - `bg_padding_color` is the color the shader paints OUTSIDE the
///   `grid_size * cell_size_px` rectangle (where the gui-side
///   `resize_terminals_to_node` left padding).
/// - "No cursor" is encoded by clearing the `CURSOR_VISIBLE` bit in
///   `cursor_style` (and leaving `cursor_pos` at any value). The shader
///   short-circuits on `cursor_visible == 0u`, so we deliberately keep
///   `cursor_pos` as `UVec2` rather than introducing a signed sentinel —
///   the existing visibility bit already does that job. The vi cursor in
///   scrollback uses the same path: `cursor_visible = 0`.
///
/// # Layout (std140, encase derive)
///
/// Field offsets in bytes. `max_overflow_phys` fills the 4-byte padding slot
/// at offset 76 that encase would otherwise insert before `bg_padding_color`
/// (Vec4 needs 16-byte alignment, lands at offset 80). `inactive_tint` is also
/// a Vec4 (16-byte alignment), so encase pads the 4 bytes after `dim` and lands
/// it at offset 112; `overlay_rects` (`array<vec4<i32>, 4>`) follows at offset
/// 128. The trailing `overlay_dim` / `overlay_desaturate` scalars sit at 192/196
/// and the struct rounds up to its 16-byte alignment (total 208 bytes):
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
/// | 44     | `time_seconds`              |
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
/// | 192    | `overlay_dim`               |
/// | 196    | `overlay_desaturate`        |
#[derive(Clone, Copy, ShaderType, Debug)]
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
    underline_position_phys: f32,
    underline_thickness_phys: f32,
    /// Worst-case ASCII rightward bbox overflow (physical px) across all four
    /// faces. The shader uses this to extend its "rightmost column glyph"
    /// evaluation past `grid_size.x * cell_size_px.x` into the bg_padding
    /// strip; the host reserves the same amount from the node width.
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
    /// `1.0` = active / full-bright; `< 1.0` dims an inactive pane. The
    /// hand-written `Default` below sets this to `1.0` so an un-updated
    /// material never renders dark (a derived `Default` would give `0.0`).
    dim: f32,
    /// Per-pane background tint: `rgb` = target color (LINEAR), `a` = blend
    /// amount in `0.0..=1.0`. The shader blends each background source toward
    /// `rgb` by `a` BEFORE glyphs/overlays paint (background only). `a == 0`
    /// (active / no-op) leaves the background untouched; `Default` (below) sets
    /// this to `Vec4::ZERO`. A Vec4, so encase pads the 4 bytes after `dim`.
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
            time_seconds: 0.0,
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
        }
    }
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
        underline_position_phys: f32,
        underline_thickness_phys: f32,
        max_overflow_phys: f32,
        bg_padding_color: Vec4,
        hover_hyperlink_id: u32,
        hover_active: u32,
        dim: f32,
        inactive_tint: Vec4,
        overlay_dim: f32,
        overlay_desaturate: f32,
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
            underline_position_phys,
            underline_thickness_phys,
            max_overflow_phys,
            bg_padding_color,
            hover_hyperlink_id,
            hover_active,
            dim,
            inactive_tint,
            overlay_rects: [IVec4::ZERO; OVERLAY_SLOTS],
            overlay_dim,
            overlay_desaturate,
        }
    }
}

/// One GPU-side cell — 20 bytes, indexed `row * cols + col` in the storage buffer.
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
    /// OSC 8 wire id of this cell, or `0` for "no link". Safe because
    /// `HyperlinkInterner` reserves `HyperlinkId(0)`.
    hyperlink_id: u32,
}

/// Set on the right-half cell of a width=2 (CJK / wide) grapheme so the
/// shader knows to render its glyph anchored to the left-half cell's
/// origin. See `rebuild_cells` and `terminal_ui_material.wgsl`.
///
/// Bit allocation in `GpuCell.style_flags` (a `u32`):
/// - Bits 0-15: wire-protocol style mirrored from
///   `ozmux_terminal_protocol::style::*` (BOLD=1, ITALIC=2, UNDERLINE=4,
///   STRIKE=8, REVERSE=16, DIM=32, HIDDEN=64; bits 7-15 reserved).
/// - Bits 16+: renderer-only flags (this const), kept physically separate
///   from the wire range so a future wire extension cannot collide.
const STYLE_WIDE_RIGHT_HALF: u32 = 0x1_0000;

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
            hyperlink_id: 0,
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
    /// Bearing from the glyph origin in physical px (positive Y goes down).
    offset_px: Vec2,
    /// Rasterized bitmap size in physical px.
    size_px: Vec2,
}

impl GpuGlyph {
    /// Builds a `GpuGlyph` from an atlas rect. All offsets and sizes are in
    /// physical pixels — the shader handles all DPR-aware geometry.
    fn new(rect: GlyphRect) -> Self {
        Self {
            uv_min: Vec2::new(rect.u as f32, rect.v as f32),
            uv_max: Vec2::new((rect.u + rect.w) as f32, (rect.v + rect.h) as f32),
            offset_px: Vec2::new(rect.offset_x as f32, rect.offset_y as f32),
            size_px: Vec2::new(rect.w as f32, rect.h as f32),
        }
    }
}

fn update_terminal_material(
    mut atlas: ResMut<GlyphAtlas>,
    mut materials: ResMut<Assets<TerminalUiMaterial>>,
    mut buffers: ResMut<Assets<ShaderStorageBuffer>>,
    mut terminals: Query<(
        Entity,
        &MaterialNode<TerminalUiMaterial>,
        &mut TerminalMaterialState,
        &TerminalGrid,
        Option<&PaneInactiveStyle>,
        Option<&TerminalOverlays>,
    )>,
    fonts: Res<TerminalFonts>,
    font_size: Res<TerminalFontSize>,
    palette_time: Res<Time>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut cell_metrics_res: ResMut<TerminalCellMetricsResource>,
    hover: Res<HyperlinkHoverState>,
) {
    // NOTE: This system runs unconditionally — *not* gated by
    // `Changed<TerminalGrid>`. The `mat.params = ...` write at the end is
    // load-bearing for rendering correctness: it forces `AssetEvent::Modified`
    // on the material every frame so `PreparedUiMaterial::prepare_asset` runs
    // and rebuilds the bind group against the latest `GpuImage` /
    // `GpuShaderStorageBuffer`. Without this, the bind group keeps a stale
    // reference to the initial (empty) atlas texture even after
    // `sync_atlas_image` re-uploads pixels — the glyphs are present on GPU
    // but the shader's `textureSampleLevel` returns 0. The actual GPU upload
    // cost is bounded by `needs_rebuild` below. The same every-frame Modified
    // is also the overlay-texture rebind lifeline: a bevy_cef headless target
    // re-creates its GPU texture on resize, and only this rebuild repoints
    // the bind group at it (spec §4).
    // NOTE: Skip the entire system when PrimaryWindow is transiently
    // absent (display hotplug, brief winit reconnect). Trade-off: the
    // `mat.params = ...` write below would fire AssetEvent::Modified
    // every frame (load-bearing for bind-group rebuild — see NOTE above);
    // skipping for one frame means the previous frame's bind group
    // continues to serve. This is bounded (sync_atlas_image is also
    // ordered after this system, so atlas uploads defer in lock-step)
    // and far less disruptive than the previous .unwrap_or(1.0) flash
    // that would re-rasterize the entire atlas at half scale.
    let Ok(window) = windows.single() else {
        return;
    };
    let dpr = window.scale_factor();
    let phys_font_size = (font_size.0 * dpr).round() as u16;

    for (entity, handle, mut state, grid, pane_style, overlays) in terminals.iter_mut() {
        let atlas_invalidated = atlas.generation != state.last_atlas_generation;
        let cols = grid.cols as u32;
        let rows = grid.rows as u32;
        let dims_changed = (grid.cols, grid.rows) != state.last_grid_dims;
        let grid_changed = grid.last_seq != state.last_grid_seq;
        let phys_size_changed = phys_font_size != state.last_phys_font_size;

        let needs_rebuild = !state.initialized
            || grid_changed
            || atlas_invalidated
            || dims_changed
            || phys_size_changed;

        if phys_size_changed {
            state.invalidate_all();
            state.last_phys_font_size = phys_font_size;
        }

        // NOTE: atlas.generation can advance during this very system (via
        //       get_or_insert in rebuild_cells), and a generation jump means
        //       the atlas pixel buffer was wiped — every cached glyph index
        //       in cpu_cells is now stale and would resolve to garbage
        //       texels. Clearing the LUT here forces a full rerasterization
        //       on the rebuild path.
        if atlas_invalidated {
            state.glyph_index_map.clear();
            state.cpu_glyphs.clear();
        }

        let metrics = if let Some(cached) = state.cached_metrics {
            cached
        } else {
            let m = fonts.cell_metrics_px(phys_font_size);
            state.cached_metrics = Some(m);
            m
        };
        let cell_w_phys = metrics.advance_phys.floor().max(1.0);
        let cell_h_phys = metrics.line_height_phys.floor().max(1.0);
        let cell_size_phys = Vec2::new(cell_w_phys, cell_h_phys);
        let ascent_phys = metrics.ascent_phys.round();

        // NOTE: Write the metrics back to TerminalCellMetricsResource so
        //       gui-side resize_terminals_to_node reads DPR-adjusted phys
        //       values on the next frame. The OR condition also catches
        //       the case where the Resource was reset externally (e.g.
        //       hot-reload) even if our local state matches.
        if phys_size_changed || cell_metrics_res.phys_font_size != phys_font_size {
            *cell_metrics_res = TerminalCellMetricsResource {
                metrics,
                phys_font_size,
            };
        }

        let Some((cells_handle, glyphs_handle)) = materials
            .get(&handle.0)
            .map(|m| (m.cells.clone(), m.glyphs.clone()))
        else {
            continue;
        };

        if needs_rebuild {
            let cell_count = (cols * rows) as usize;
            state.cpu_cells.clear();
            state.cpu_cells.resize(cell_count, GpuCell::default());

            if cols > 0 && rows > 0 {
                rebuild_cells(grid, &mut state, &fonts, &mut atlas, phys_font_size, cols);
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

        let bg_padding_color = {
            let [r, g, b] = grid.default_bg;
            let c = Color::srgb_u8(r, g, b).to_linear();
            Vec4::new(c.red, c.green, c.blue, 1.0)
        };

        let (hover_hyperlink_id, hover_active) = match (hover.entity, hover.hyperlink_id) {
            (Some(e), Some(id)) if e == entity => (id.0, if hover.modifier_held { 1 } else { 0 }),
            _ => (0, 0),
        };

        let (dim, inactive_tint, overlay_dim, overlay_desaturate) =
            pane_style.map_or((1.0, Vec4::ZERO, 1.0, 0.0), |s| {
                (
                    s.dim.clamp(0.0, 1.0),
                    s.tint.with_w(s.tint.w.clamp(0.0, 1.0)),
                    s.overlay_dim.clamp(0.0, 1.0),
                    s.overlay_desaturate.clamp(0.0, 1.0),
                )
            });
        if let Some(mat) = materials.get_mut(&handle.0) {
            let mut params = TerminalParams::new(
                grid,
                cell_size_phys,
                Vec2::new(atlas.width() as f32, atlas.height() as f32),
                ascent_phys,
                dpr,
                palette_time.elapsed_secs(),
                metrics.underline_position_phys,
                metrics.underline_thickness_phys.max(1.0),
                metrics.max_overflow_phys,
                bg_padding_color,
                hover_hyperlink_id,
                hover_active,
                dim,
                inactive_tint,
                overlay_dim,
                overlay_desaturate,
            );
            match overlays {
                Some(o) => {
                    params.overlay_rects = o.rects;
                    mat.set_overlays(&o.textures);
                }
                None => {
                    mat.set_overlays(&[const { None }; OVERLAY_SLOTS]);
                }
            }
            mat.params = params;
        }
    }
}

fn rebuild_cells(
    grid: &TerminalGrid,
    state: &mut TerminalMaterialState,
    fonts: &TerminalFonts,
    atlas: &mut GlyphAtlas,
    phys_font_size: u16,
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
            let glyph_index = resolve_glyph_index(cell, state, fonts, atlas, phys_font_size);
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
                    hyperlink_id: cell.hyperlink_id.map_or(0, |h| h.0),
                };
            }

            // NOTE: For width=2 (CJK / wide) cells we ALSO populate the
            //       right-half slot with the same glyph_index + fg + bg
            //       and set STYLE_WIDE_RIGHT_HALF. The shader uses the
            //       bit to anchor the wide glyph to the left-half cell's
            //       origin (`in_cell_px_eff = in_cell_px + vec2(cell_pitch_px.x, 0)`),
            //       rendering a continuous wide glyph across both cells.
            //       Without this, the right half stays at GpuCell::default
            //       (bg=0 transparent, glyph_index=GLYPH_NONE) and CJK
            //       characters render as half-glyphs with black gaps.
            if cell_width == 2 && col + 1 < cols {
                let right_target = (row_idx as u32 * cols + col + 1) as usize;
                if let Some(right_slot) = state.cpu_cells.get_mut(right_target) {
                    *right_slot = GpuCell {
                        glyph_index,
                        fg_packed: fg,
                        bg_packed: bg,
                        style_flags: style_flags | STYLE_WIDE_RIGHT_HALF,
                        hyperlink_id: cell.hyperlink_id.map_or(0, |h| h.0),
                    };
                }
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
    state.cpu_glyphs.push(GpuGlyph::new(rect));
    state.glyph_index_map.insert(key, idx);
    idx
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::size_of;

    #[test]
    fn gpu_cell_is_twenty_bytes() {
        assert_eq!(size_of::<GpuCell>(), 20);
    }

    #[test]
    fn gpu_cell_default_has_zero_hyperlink_id() {
        let cell = GpuCell::default();
        assert_eq!(cell.hyperlink_id, 0);
    }

    #[test]
    fn rebuild_cells_writes_hyperlink_id_when_present() {
        use crate::schema::HyperlinkId;
        use bevy::platform::collections::HashMap;

        let linked = Cell {
            text: "x".to_string(),
            width: 1,
            fg: Color::WHITE,
            bg: Color::BLACK,
            style: 0,
            hyperlink_id: Some(HyperlinkId(7)),
        };
        let unlinked = Cell {
            text: "y".to_string(),
            width: 1,
            fg: Color::WHITE,
            bg: Color::BLACK,
            style: 0,
            hyperlink_id: None,
        };
        let grid = TerminalGrid {
            cols: 2,
            rows: 1,
            cells: vec![vec![linked, unlinked]],
            ..Default::default()
        };
        let mut state = TerminalMaterialState {
            glyph_index_map: HashMap::new(),
            cpu_cells: vec![GpuCell::default(); 2],
            cpu_glyphs: Vec::new(),
            last_atlas_generation: 0,
            last_grid_seq: 0,
            last_grid_dims: (0, 0),
            last_phys_font_size: 0,
            cached_metrics: None,
            initialized: false,
        };
        let mut atlas = GlyphAtlas::default();
        let fonts = TerminalFonts::default();

        rebuild_cells(&grid, &mut state, &fonts, &mut atlas, 16, 2);

        assert_eq!(state.cpu_cells[0].hyperlink_id, 7);
        assert_eq!(state.cpu_cells[1].hyperlink_id, 0);
    }

    #[test]
    fn terminal_params_default_hyperlink_uniforms_are_zero() {
        let params = TerminalParams::default();
        assert_eq!(params.hover_hyperlink_id, 0);
        assert_eq!(params.hover_active, 0);
    }

    #[test]
    fn terminal_params_default_dim_is_one() {
        assert_eq!(TerminalParams::default().dim, 1.0);
    }

    #[test]
    fn terminal_overlays_default_is_all_sentinel() {
        let o = TerminalOverlays::default();
        assert!(
            o.rects.iter().all(|r| r.z == 0),
            "rows == 0 sentinel on every slot"
        );
        assert!(o.textures.iter().all(Option::is_none));
    }

    #[test]
    fn set_overlays_maps_slot_index_to_field_order() {
        const H_A: Handle<Image> = uuid_handle!("c0fee000-0000-4000-8000-000000000001");
        const H_B: Handle<Image> = uuid_handle!("c0fee000-0000-4000-8000-000000000002");
        let mut mat = TerminalUiMaterial::default();
        let textures = [Some(H_A), None, Some(H_B), None];
        mat.set_overlays(&textures);
        assert_eq!(mat.overlay0, Some(H_A));
        assert_eq!(mat.overlay1, None);
        assert_eq!(mat.overlay2, Some(H_B));
        assert_eq!(mat.overlay3, None);
    }

    #[test]
    fn terminal_params_uniform_size_includes_overlay_rects() {
        assert_eq!(<TerminalParams as ShaderType>::min_size().get(), 208);
    }

    #[test]
    fn terminal_params_default_inactive_treatment_is_noop() {
        let p = TerminalParams::default();
        assert_eq!(p.inactive_tint, Vec4::ZERO);
        assert_eq!(p.overlay_dim, 1.0);
        assert_eq!(p.overlay_desaturate, 0.0);
    }

    #[test]
    fn terminal_params_field_offsets_are_pinned() {
        // `dim` is at offset 104; `inactive_tint` (a Vec4, 16-byte aligned)
        // lands at 112 after encase pads the 4 bytes following `dim`;
        // `overlay_rects` follows at 128; the trailing scalars `overlay_dim` /
        // `overlay_desaturate` sit at 192/196 (total 208 bytes). Field indices
        // are 0-based in declaration order.
        assert_eq!(
            <TerminalParams as ShaderType>::METADATA.offset(19),
            104,
            "dim"
        );
        assert_eq!(
            <TerminalParams as ShaderType>::METADATA.offset(20),
            112,
            "inactive_tint (Vec4) after the pad following dim"
        );
        assert_eq!(
            <TerminalParams as ShaderType>::METADATA.offset(21),
            128,
            "overlay_rects after inactive_tint"
        );
        assert_eq!(
            <TerminalParams as ShaderType>::METADATA.offset(22),
            192,
            "overlay_dim after overlay_rects"
        );
        assert_eq!(
            <TerminalParams as ShaderType>::METADATA.offset(23),
            196,
            "overlay_desaturate after overlay_dim"
        );
    }
}
