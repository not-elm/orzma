use crate::{
    glyph::{
        atlas::{GlyphAtlas, GlyphRect},
        font::{FontFace, GlyphKey, TerminalCellMetricsResource, TerminalFontSize, TerminalFonts},
    },
    material::state::TerminalMaterialState,
    schema::{
        Color as CellColor, GridCell, GridLine, GridSlot, HyperlinkHoverState, Palette, Rgb,
        SelectionGeometry, SelectionRange, Style, TerminalGrid,
    },
};
use bevy::{
    asset::{load_internal_asset, uuid_handle},
    ecs::system::{SystemParamItem, lifetimeless::SRes},
    prelude::*,
    render::{
        render_asset::RenderAssets,
        render_resource::{
            AsBindGroup, AsBindGroupError, BindGroupLayout, BindGroupLayoutEntry, BindingResources,
            BindingType, BufferBindingType, BufferInitDescriptor, BufferUsages,
            OwnedBindingResource, SamplerBindingType, ShaderStages, ShaderType, TextureSampleType,
            TextureViewDimension, UnpreparedBindGroup, encase::UniformBuffer,
        },
        renderer::RenderDevice,
        storage::{GpuShaderBuffer, ShaderBuffer},
        texture::{FallbackImage, GpuImage},
    },
    shader::ShaderRef,
    window::PrimaryWindow,
};

mod state;

/// Ordering anchor for the system that writes each terminal's material.
///
/// A system that resizes a terminal's grid from the layout must run
/// `.before(Self::UpdateMaterial)`.
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
        app.init_resource::<TerminalPaddingFallback>()
            .add_plugins(UiMaterialPlugin::<TerminalUiMaterial>::default())
            .add_plugins(state::TerminalMaterialStatePlugin)
            // NOTE: Scheduled in `PostUpdate` (not `Update`) so it runs after
            // `ui_layout_system` has written the current frame's
            // `ComputedNode.size`. The downstream consumer
            // `resize_terminals_to_node` in orzma depends on layout being
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
#[derive(Asset, TypePath, Clone)]
pub struct TerminalUiMaterial {
    params: TerminalParams,
    cells: Handle<ShaderBuffer>,
    glyphs: Handle<ShaderBuffer>,
    atlas: Handle<Image>,
    overlays: [Option<Handle<Image>>; OVERLAY_SLOTS],
}

/// First `@binding` index of the overlay texture array; slot `i` binds at
/// `OVERLAY_TEX_BINDING_BASE + i`. Bindings 0..=5 are params/cells/glyphs/atlas
/// texture/atlas sampler/shared overlay sampler.
const OVERLAY_TEX_BINDING_BASE: u32 = 6;

impl Default for TerminalUiMaterial {
    fn default() -> Self {
        Self {
            params: TerminalParams::default(),
            cells: Handle::default(),
            glyphs: Handle::default(),
            atlas: Handle::default(),
            overlays: [const { None }; OVERLAY_SLOTS],
        }
    }
}

impl AsBindGroup for TerminalUiMaterial {
    type Data = ();
    type Param = (
        SRes<RenderAssets<GpuImage>>,
        SRes<FallbackImage>,
        SRes<RenderAssets<GpuShaderBuffer>>,
    );

    fn label() -> &'static str {
        "terminal_ui_material"
    }

    fn bind_group_data(&self) -> Self::Data {}

    fn unprepared_bind_group(
        &self,
        _layout: &BindGroupLayout,
        render_device: &RenderDevice,
        (images, fallback_image, storage_buffers): &mut SystemParamItem<'_, '_, Self::Param>,
        _force_no_bindless: bool,
    ) -> Result<UnpreparedBindGroup, AsBindGroupError> {
        let mut params_buffer = UniformBuffer::new(Vec::new());
        params_buffer.write(&self.params).unwrap();

        let cells = storage_buffers
            .get(&self.cells)
            .ok_or(AsBindGroupError::RetryNextUpdate)?;
        let glyphs = storage_buffers
            .get(&self.glyphs)
            .ok_or(AsBindGroupError::RetryNextUpdate)?;
        let atlas = images
            .get(&self.atlas)
            .ok_or(AsBindGroupError::RetryNextUpdate)?;

        let mut bindings = vec![
            (
                0,
                OwnedBindingResource::Buffer(render_device.create_buffer_with_data(
                    &BufferInitDescriptor {
                        label: Some("terminal_params"),
                        usage: BufferUsages::UNIFORM,
                        contents: params_buffer.as_ref(),
                    },
                )),
            ),
            (1, OwnedBindingResource::Buffer(cells.buffer.clone())),
            (2, OwnedBindingResource::Buffer(glyphs.buffer.clone())),
            (
                3,
                OwnedBindingResource::TextureView(
                    TextureViewDimension::D2,
                    atlas.texture_view.clone(),
                ),
            ),
            (
                4,
                OwnedBindingResource::Sampler(SamplerBindingType::Filtering, atlas.sampler.clone()),
            ),
            (
                5,
                OwnedBindingResource::Sampler(
                    SamplerBindingType::Filtering,
                    fallback_image.d2.sampler.clone(),
                ),
            ),
        ];
        bindings.reserve(OVERLAY_SLOTS);

        // NOTE: A `None` slot, OR a `Some` whose GpuImage is not yet loaded,
        // binds the fallback view. The shader never samples it (the `rect.z==0`
        // sentinel gates it), and this avoids stalling the whole material's bind
        // group on a single mid-load overlay texture.
        for (i, handle) in self.overlays.iter().enumerate() {
            let view = handle.as_ref().and_then(|h| images.get(h)).map_or_else(
                || fallback_image.d2.texture_view.clone(),
                |img| img.texture_view.clone(),
            );
            bindings.push((
                OVERLAY_TEX_BINDING_BASE + i as u32,
                OwnedBindingResource::TextureView(TextureViewDimension::D2, view),
            ));
        }

        Ok(UnpreparedBindGroup {
            bindings: BindingResources(bindings),
        })
    }

    fn bind_group_layout_entries(
        _render_device: &RenderDevice,
        _force_no_bindless: bool,
    ) -> Vec<BindGroupLayoutEntry> {
        let texture = |binding: u32| BindGroupLayoutEntry {
            binding,
            visibility: ShaderStages::FRAGMENT,
            ty: BindingType::Texture {
                sample_type: TextureSampleType::Float { filterable: true },
                view_dimension: TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let sampler = |binding: u32| BindGroupLayoutEntry {
            binding,
            visibility: ShaderStages::FRAGMENT,
            ty: BindingType::Sampler(SamplerBindingType::Filtering),
            count: None,
        };
        let storage = |binding: u32| BindGroupLayoutEntry {
            binding,
            visibility: ShaderStages::FRAGMENT,
            ty: BindingType::Buffer {
                ty: BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };

        let mut entries = vec![
            BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Buffer {
                    ty: BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: Some(<TerminalParams as ShaderType>::min_size()),
                },
                count: None,
            },
            storage(1),
            storage(2),
            texture(3),
            sampler(4),
            sampler(5),
        ];
        for i in 0..OVERLAY_SLOTS as u32 {
            entries.push(texture(OVERLAY_TEX_BINDING_BASE + i));
        }
        entries
    }
}

impl UiMaterial for TerminalUiMaterial {
    fn fragment_shader() -> ShaderRef {
        TERMINAL_SHADER_HANDLE.into()
    }
}

impl TerminalUiMaterial {
    /// Overwrites the overlay texture array, slot-indexed (`None` = inactive).
    fn set_overlays(&mut self, textures: &[Option<Handle<Image>>; OVERLAY_SLOTS]) {
        self.overlays = textures.clone();
    }
}

/// Per-pane inactive-pane treatment for the terminal renderer: a background
/// `tint` (rgb = target color in LINEAR space, `a` = blend amount) and a
/// brightness `dim`. The shader blends each background source toward `tint.rgb`
/// by `tint.a` before glyphs/overlays paint (background only), then multiplies
/// the final color by `dim`. An absent component is treated as
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

/// Padding colour used for the area outside a terminal grid (and the whole
/// quad while a grid is unpainted) when the terminal's default background
/// is black. Defaults to black.
#[derive(Resource, Default)]
pub struct TerminalPaddingFallback(pub [u8; 3]);

/// Number of inline-overlay texture slots on `TerminalUiMaterial`.
///
/// Slot index = array index into `overlays` / `overlay_rects`; the WGSL
/// texture binding is `OVERLAY_TEX_BINDING_BASE + i`. Hard upper bound per
/// terminal surface.
pub const OVERLAY_SLOTS: usize = 12;

/// Per-terminal overlay placements and textures.
///
/// `rects[i]` is `(row, col, rows, cols)` in CELL coordinates; `row` may be
/// negative when the rect starts above the viewport. `rows == 0` is the
/// inactive-slot sentinel: the shader skips the slot, while the renderer
/// binds `textures[i]` regardless, so consumers should set freed slots to
/// `None`.
///
/// Consumers must rebuild this component from live state every frame
/// (all-sentinel start), so stale texture handles cannot outlive their
/// producers.
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
/// | 320    | `overlay_dim`               |
/// | 324    | `overlay_desaturate`        |
///
/// # Invariants
///
/// - All `_phys` fields are PHYSICAL pixels (no DPR division). The shader
///   computes everything in physical-px space and never reads `dpr`.
/// - `bg_padding_color` is the color the shader paints OUTSIDE the
///   `grid_size * cell_size_px` rectangle.
/// - "No cursor" is encoded by clearing the `CURSOR_VISIBLE` bit in
///   `cursor_style` (and leaving `cursor_pos` at any value); the shader
///   short-circuits on `cursor_visible == 0u`. A cursor (vi or live)
///   whose grid line projects outside the viewport takes the same path:
///   `cursor_visible = 0`.
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
    /// - When `grid.vi_cursor` is present and its grid point projects into
    ///   the viewport, it overrides `grid.cursor` and the resulting
    ///   `cursor_visible` bit is forced to `1`. When the projection falls
    ///   outside the viewport, `cursor_visible` is cleared so the shader
    ///   skips cursor rendering entirely.
    /// - A live (non-vi) cursor carries the application's DECTCEM state, so
    ///   `cursor_visible` is `0` while the terminal has seen `CSI ? 25 l`,
    ///   and `grid.suppress_cursor` clears the bit on top of either source.
    /// - When `grid.selection` is `None`, `sel_kind == 0` and the shader
    ///   paints no selection.
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
            selection_uniforms(grid.selection.as_ref(), grid.display_offset, grid.rows);

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
    /// The cell's `Style` bits ORed with the underline and strike bits its
    /// combining marks promote, plus the renderer-only flags from bit 16 up.
    style_flags: u32,
    /// OSC 8 wire id of this cell, or `0` for "no link".
    hyperlink_id: u32,
}

/// Set on the right-half cell of a width=2 (CJK / wide) grapheme so the
/// shader renders its glyph anchored to the left-half cell's origin.
///
/// Bit allocation in `GpuCell.style_flags` (a `u32`):
/// - Bits 0-15: `Style` flags at the bits `Style` assigns.
/// - Bits 16+: renderer-only flags such as this one.
const STYLE_WIDE_RIGHT_HALF: u32 = 0x1_0000;

const _: () = assert!(
    (STYLE_WIDE_RIGHT_HALF & Style::all().bits() as u32) == 0,
    "a renderer-only style flag overlaps a `Style` bit",
);

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

/// Per-glyph atlas record in the glyph storage buffer.
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
    /// Builds a `GpuGlyph` from an atlas rect, with all offsets and sizes
    /// in physical pixels.
    fn new(rect: GlyphRect) -> Self {
        Self {
            uv_min: Vec2::new(rect.u as f32, rect.v as f32),
            uv_max: Vec2::new((rect.u + rect.w) as f32, (rect.v + rect.h) as f32),
            offset_px: Vec2::new(rect.offset_x as f32, rect.offset_y as f32),
            size_px: Vec2::new(rect.w as f32, rect.h as f32),
        }
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

/// The transparent cell-background packing (`alpha == 0`) the shader
/// treats as "terminal default background".
const TRANSPARENT_BG: u32 = 0;

/// The grid palette pre-packed to the shader's linear `u32` encoding.
struct PackedPalette {
    indexed: [u32; 256],
    foreground: u32,
    background: u32,
}

impl PackedPalette {
    /// Packs every slot of `palette` once.
    fn build(palette: &Palette) -> Self {
        Self {
            indexed: palette.indexed.map(pack_linear),
            foreground: pack_linear(palette.foreground),
            background: pack_linear(palette.background),
        }
    }

    /// Packs a cell foreground, resolving symbolic colors to their
    /// palette slot.
    //
    // NOTE: The variant-to-slot mapping mirrors `Palette::resolve` in
    //       `orzma_vt`, pre-packed here for the per-cell hot path; a
    //       change to either mapping must be applied to both, or
    //       symbolic colors silently diverge between producers.
    fn cell_fg(&self, color: CellColor) -> u32 {
        match color {
            CellColor::DefaultForeground => self.foreground,
            CellColor::DefaultBackground => self.background,
            CellColor::Indexed(index) => self.indexed[usize::from(index)],
            CellColor::Rgb(rgb) => pack_linear(rgb),
        }
    }

    /// Packs a cell background.
    ///
    /// # Invariants
    ///
    /// `DefaultBackground` packs [`TRANSPARENT_BG`], never the opaque
    /// palette background, so an explicit RGB equal to that background
    /// stays distinguishable from the default.
    fn cell_bg(&self, color: CellColor) -> u32 {
        match color {
            CellColor::DefaultBackground => TRANSPARENT_BG,
            other => self.cell_fg(other),
        }
    }
}

/// sRGB byte triple → the opaque linear u32 packing the shader decodes.
fn pack_linear(rgb: Rgb) -> u32 {
    Color::srgb_u8(rgb.r, rgb.g, rgb.b).to_linear().as_u32()
}

fn update_terminal_material(
    mut atlas: ResMut<GlyphAtlas>,
    mut materials: ResMut<Assets<TerminalUiMaterial>>,
    mut buffers: ResMut<Assets<ShaderBuffer>>,
    mut terminals: Query<(
        Entity,
        &MaterialNode<TerminalUiMaterial>,
        &mut TerminalMaterialState,
        Ref<TerminalGrid>,
        Option<&PaneInactiveStyle>,
        Option<&TerminalOverlays>,
    )>,
    fonts: Res<TerminalFonts>,
    font_size: Res<TerminalFontSize>,
    palette_time: Res<Time>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut cell_metrics_res: ResMut<TerminalCellMetricsResource>,
    hover: Res<HyperlinkHoverState>,
    fallback: Res<TerminalPaddingFallback>,
) {
    // NOTE: This system runs unconditionally — *not* gated by
    // `Changed<TerminalGrid>`. The `mat.params = ...` write at the end is
    // load-bearing for rendering correctness: it forces `AssetEvent::Modified`
    // on the material every frame so `PreparedUiMaterial::prepare_asset` runs
    // and rebuilds the bind group against the latest `GpuImage` /
    // `GpuShaderBuffer`. Without this, the bind group keeps a stale
    // reference to the initial (empty) atlas texture even after
    // `sync_atlas_image` re-uploads pixels — the glyphs are present on GPU
    // but the shader's `textureSampleLevel` returns 0. The actual GPU upload
    // cost is bounded by `needs_rebuild` below. The same every-frame Modified
    // is also the overlay-texture rebind lifeline: a bevy_cef headless target
    // re-creates its GPU texture on resize, and only this rebuild repoints
    // the bind group at it.
    // NOTE: Skip the per-entity work when PrimaryWindow is transiently
    // absent (display hotplug, brief winit reconnect). Trade-off: the
    // `mat.params = ...` write below would fire AssetEvent::Modified
    // every frame (load-bearing for bind-group rebuild — see NOTE above);
    // skipping for one frame means the previous frame's bind group
    // continues to serve. This is bounded (sync_atlas_image is also
    // ordered after this system, so atlas uploads defer in lock-step)
    // and far less disruptive than the previous .unwrap_or(1.0) flash
    // that would re-rasterize the entire atlas at half scale.
    let dpr = windows.single().ok().map(|window| window.scale_factor());

    for (entity, handle, mut state, grid, pane_style, overlays) in terminals.iter_mut() {
        // NOTE: Latch the grid's change signal before the bail-out below.
        // Bevy clears it once this system has run, so a grid written on a
        // frame that skips the upload would otherwise never reach the GPU.
        state.grid_dirty |= grid.is_changed();
        let Some(dpr) = dpr else {
            continue;
        };
        let phys_font_size = (font_size.0 * dpr).round() as u16;
        let atlas_invalidated = atlas.generation != state.last_atlas_generation;
        let cols = grid.cols as u32;
        let rows = grid.rows as u32;
        let dims_changed = (grid.cols, grid.rows) != state.last_grid_dims;
        let grid_changed = state.grid_dirty;
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
                rebuild_cells(&mut state, &mut atlas, &grid, &fonts, phys_font_size, cols);
            }

            if state.cpu_cells.is_empty() {
                state.cpu_cells.push(GpuCell::default());
            }
            if state.cpu_glyphs.is_empty() {
                state.cpu_glyphs.push(GpuGlyph::default());
            }

            if let Some(mut buf) = buffers.get_mut(&cells_handle) {
                buf.set_data(std::mem::take(&mut state.cpu_cells));
            }
            if let Some(mut buf) = buffers.get_mut(&glyphs_handle) {
                buf.set_data(state.cpu_glyphs.clone());
            }

            state.last_atlas_generation = atlas.generation;
            state.grid_dirty = false;
            state.last_grid_dims = (grid.cols, grid.rows);
            state.initialized = true;
        }

        let bg_padding_color = padding_color(grid.palette.background, fallback.0);

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
        if let Some(mut mat) = materials.get_mut(&handle.0) {
            let mut params = TerminalParams::new(
                &grid,
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
    state: &mut TerminalMaterialState,
    atlas: &mut GlyphAtlas,
    grid: &TerminalGrid,
    fonts: &TerminalFonts,
    phys_font_size: u16,
    cols: u32,
) {
    let packed_palette = PackedPalette::build(&grid.palette);
    for (row_idx, row) in grid.cells.iter().enumerate() {
        debug_assert_eq!(
            row.len(),
            cols as usize,
            "every retained row is exactly as wide as the grid"
        );
        let mut left_half: Option<GpuCell> = None;
        for (col, slot) in row.iter().enumerate() {
            let col = col as u32;
            if col >= cols {
                break;
            }
            let target = (row_idx as u32 * cols + col) as usize;
            match slot {
                GridSlot::Empty => left_half = None,
                GridSlot::Cell(cell) => {
                    let gpu = GpuCell {
                        glyph_index: resolve_glyph_index(cell, state, fonts, atlas, phys_font_size),
                        fg_packed: packed_palette.cell_fg(cell.fg),
                        bg_packed: packed_palette.cell_bg(cell.bg),
                        style_flags: u32::from(
                            cell.style | style_from_combining_marks(&cell.text).bits(),
                        ),
                        hyperlink_id: cell.hyperlink.as_ref().map_or(0, |h| h.id.0),
                    };
                    if let Some(target) = state.cpu_cells.get_mut(target) {
                        *target = gpu;
                    }
                    left_half = (cell.width == 2).then_some(gpu);
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
                GridSlot::WideTrailer => {
                    if let Some(left) = left_half
                        && let Some(target) = state.cpu_cells.get_mut(target)
                    {
                        *target = GpuCell {
                            style_flags: left.style_flags | STYLE_WIDE_RIGHT_HALF,
                            ..left
                        };
                    }
                }
            }
        }
    }
}

/// Promotes specific combining marks in a grapheme cluster to the `Style`
/// underline and strike flags so the shader paints them.
///
/// Maps U+0332 (combining low line), U+0333 (double low line), U+0331
/// (combining macron below) to `Style::UNDERLINE`, and U+0336 (combining
/// long stroke overlay) to `Style::STRIKE`. Other combining marks are
/// ignored — the base glyph still renders.
fn style_from_combining_marks(text: &str) -> Style {
    if text.is_ascii() {
        return Style::empty();
    }
    // TODO: Render other combining marks as well, such as U+0301
    // (combining acute accent), U+0303 (combining tilde), U+0308
    // (combining diaeresis), and U+20D7 (combining right arrow above).
    let mut style = Style::empty();
    for c in text.chars() {
        match c {
            '\u{0332}' | '\u{0333}' | '\u{0331}' => style |= Style::UNDERLINE,
            '\u{0336}' => style |= Style::STRIKE,
            _ => {}
        }
    }
    style
}

fn resolve_glyph_index(
    cell: &GridCell,
    state: &mut TerminalMaterialState,
    fonts: &TerminalFonts,
    atlas: &mut GlyphAtlas,
    phys_font_size: u16,
) -> u32 {
    if cell.is_blank() {
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
        // NOTE: Block degrades to the char encoding until the shader
        // grows a rectangular mode; SelectionKind cannot currently
        // produce Block, so no user-visible selection takes this arm.
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
    use std::collections::BTreeSet;
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

    fn cell_with_link(text: &str, link: Option<u32>) -> GridCell {
        use crate::schema::{Color as CellColor, Hyperlink, HyperlinkId, HyperlinkUri};
        GridCell {
            text: text.to_string(),
            width: 1,
            fg: CellColor::DefaultForeground,
            bg: CellColor::DefaultBackground,
            style: 0,
            hyperlink: link.map(|id| Hyperlink {
                id: HyperlinkId(id),
                uri: HyperlinkUri::new("https://example"),
            }),
        }
    }

    /// Returns the observable payload of each GPU slot as
    /// `(glyph_index, fg, bg, style_flags, hyperlink_id)`.
    fn gpu_cell_fingerprint(cells: &[GpuCell]) -> Vec<(u32, u32, u32, u32, u32)> {
        cells
            .iter()
            .map(|cell| {
                (
                    cell.glyph_index,
                    cell.fg_packed,
                    cell.bg_packed,
                    cell.style_flags,
                    cell.hyperlink_id,
                )
            })
            .collect()
    }

    /// Builds a state whose cell buffer is sized for `cell_count` slots.
    fn state_for(cell_count: usize) -> TerminalMaterialState {
        use bevy::platform::collections::HashMap;
        TerminalMaterialState {
            glyph_index_map: HashMap::new(),
            cpu_cells: vec![GpuCell::default(); cell_count],
            cpu_glyphs: Vec::new(),
            last_atlas_generation: 0,
            grid_dirty: true,
            last_grid_dims: (0, 0),
            last_phys_font_size: 0,
            cached_metrics: None,
            initialized: false,
        }
    }

    /// Asserts the GPU slots a row of a wide char, a combining mark and a
    /// linked cell produces, pinning the payload of every slot including
    /// the wide char's right half.
    ///
    /// Case: a file listing hyperlinks a CJK filename so it opens on
    /// click, while an accented latin suffix typed right after it stays
    /// plain, unlinked text.
    #[test]
    fn rebuild_cells_pins_wide_combining_and_linked_slots() {
        let mut wide = cell_with_link("あ", Some(3));
        wide.width = 2;
        let combining = {
            let mut cell = cell_with_link("e\u{0332}", None);
            cell.width = 1;
            cell
        };
        let plain = cell_with_link("z", None);
        let grid = TerminalGrid {
            cols: 4,
            rows: 1,
            cells: vec![vec![
                GridSlot::Cell(wide),
                GridSlot::WideTrailer,
                GridSlot::Cell(combining),
                GridSlot::Cell(plain),
            ]],
            ..Default::default()
        };
        let mut state = state_for(4);
        let mut atlas = GlyphAtlas::default();
        let fonts = TerminalFonts::default();

        rebuild_cells(&mut state, &mut atlas, &grid, &fonts, 16, 4);

        let fingerprint = gpu_cell_fingerprint(&state.cpu_cells);
        assert_eq!(
            fingerprint[0].4, 3,
            "the wide cell carries its hyperlink id"
        );
        assert_eq!(
            fingerprint[1].4, 3,
            "the wide cell's right half repeats the hyperlink id"
        );
        assert_eq!(
            fingerprint[1].0, fingerprint[0].0,
            "the right half repeats the left half's glyph"
        );
        assert_ne!(
            fingerprint[1].3 & STYLE_WIDE_RIGHT_HALF,
            0,
            "the right half is flagged"
        );
        assert_eq!(fingerprint[2].4, 0, "the combining cell is unlinked");
        assert_eq!(fingerprint[3].4, 0, "the plain cell is unlinked");
        assert!(
            fingerprint.iter().all(|slot| slot.4 != 9),
            "the zero-width cell occupies no GPU slot"
        );
        assert_eq!(
            fingerprint,
            vec![
                (0, u32::MAX, 0, 0, 3),
                (0, u32::MAX, 0, STYLE_WIDE_RIGHT_HALF, 3),
                (1, u32::MAX, 0, 4, 0),
                (2, u32::MAX, 0, 0, 0),
            ]
        );
    }

    /// Asserts that a linked cell's wire id reaches its GPU slot while
    /// an unlinked cell's slot keeps the 0 sentinel.
    ///
    /// Case: a row mixes OSC 8 linked text with plain text.
    #[test]
    fn rebuild_cells_writes_hyperlink_id_when_present() {
        use bevy::platform::collections::HashMap;

        let linked = cell_with_link("x", Some(7));
        let unlinked = cell_with_link("y", None);
        let grid = TerminalGrid {
            cols: 2,
            rows: 1,
            cells: vec![vec![GridSlot::Cell(linked), GridSlot::Cell(unlinked)]],
            ..Default::default()
        };
        let mut state = TerminalMaterialState {
            glyph_index_map: HashMap::new(),
            cpu_cells: vec![GpuCell::default(); 2],
            cpu_glyphs: Vec::new(),
            last_atlas_generation: 0,
            grid_dirty: true,
            last_grid_dims: (0, 0),
            last_phys_font_size: 0,
            cached_metrics: None,
            initialized: false,
        };
        let mut atlas = GlyphAtlas::default();
        let fonts = TerminalFonts::default();

        rebuild_cells(&mut state, &mut atlas, &grid, &fonts, 16, 2);

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
    fn set_overlays_copies_into_overlays_array() {
        const H_A: Handle<Image> = uuid_handle!("c0fee000-0000-4000-8000-000000000001");
        const H_B: Handle<Image> = uuid_handle!("c0fee000-0000-4000-8000-000000000002");
        let mut mat = TerminalUiMaterial::default();
        let mut textures = [const { None }; OVERLAY_SLOTS];
        textures[0] = Some(H_A);
        textures[2] = Some(H_B);
        mat.set_overlays(&textures);
        assert_eq!(mat.overlays[0], Some(H_A));
        assert_eq!(mat.overlays[1], None);
        assert_eq!(mat.overlays[2], Some(H_B));
        assert_eq!(mat.overlays[3], None);
    }

    #[test]
    fn terminal_params_uniform_size_includes_overlay_rects() {
        assert_eq!(<TerminalParams as ShaderType>::min_size().get(), 336);
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
        // `overlay_desaturate` sit at 320/324 (total 336 bytes). Field indices
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
            320,
            "overlay_dim after overlay_rects"
        );
        assert_eq!(
            <TerminalParams as ShaderType>::METADATA.offset(23),
            324,
            "overlay_desaturate after overlay_dim"
        );
    }

    #[test]
    fn padding_color_falls_back_when_default_bg_is_black() {
        let got = padding_color(Rgb { r: 0, g: 0, b: 0 }, [30, 32, 40]);
        let c = Color::srgb_u8(30, 32, 40).to_linear();
        assert_eq!(got, Vec4::new(c.red, c.green, c.blue, 1.0));
    }

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

    #[test]
    fn wgsl_overlay_bindings_track_overlay_slots() {
        let src = include_str!("shaders/terminal_ui_material.wgsl");
        let texture_decls = src.matches("_tex: texture_2d<f32>").count();
        assert_eq!(
            texture_decls,
            OVERLAY_SLOTS + 1,
            "expected atlas + {OVERLAY_SLOTS} overlay texture declarations"
        );
        // NOTE: a stale rect-array size is a silent uniform-offset bug — no
        // binding error, but the shader misreads overlay_dim/overlay_desaturate
        // at the wrong offsets (192/196 instead of 320/324).
        assert!(
            src.contains(&format!("overlay_rects: array<vec4<i32>, {OVERLAY_SLOTS}>")),
            "overlay_rects must be array<vec4<i32>, {OVERLAY_SLOTS}>"
        );
        let calls = src.matches("color = sample_overlay_slot(").count();
        assert_eq!(calls, OVERLAY_SLOTS, "sample_overlay_slot call count");

        // NOTE: the counts above cannot catch a binding-number drift or a
        // copy-paste that samples the wrong texture for a slot — both produce a
        // runtime bind-group/pipeline error, never a test failure. Pin the
        // shared sampler binding, each slot's texture binding, and the per-call
        // overlay_rects[i] <-> overlay{i}_tex pairing, all from the Rust
        // constants, so a drift fails here instead.
        assert!(
            src.contains(&format!(
                "@binding({}) var overlay_samp: sampler;",
                OVERLAY_TEX_BINDING_BASE - 1
            )),
            "shared overlay_samp must be at binding {}",
            OVERLAY_TEX_BINDING_BASE - 1
        );
        for i in 0..OVERLAY_SLOTS {
            let binding = OVERLAY_TEX_BINDING_BASE + i as u32;
            assert!(
                src.contains(&format!(
                    "@binding({binding}) var overlay{i}_tex: texture_2d<f32>;"
                )),
                "overlay{i}_tex must be declared at binding {binding}"
            );
            assert!(
                src.contains(&format!(
                    "sample_overlay_slot(params.overlay_rects[{i}], overlay{i}_tex, overlay_samp,"
                )),
                "slot {i} call must pair overlay_rects[{i}] with overlay{i}_tex"
            );
        }
    }

    /// Asserts that the shader's style constants are exactly the `Style`
    /// flags other than the font-selecting `BOLD` and `ITALIC`, plus the
    /// renderer-only `WIDE_RIGHT_HALF`, each declared with the bit the Rust
    /// side assigns.
    ///
    /// Case: a program prints underlined, struck-through, reverse-video,
    /// faint, and concealed text, and the shader paints each attribute from
    /// the cell's raw `Style` bits.
    #[test]
    fn wgsl_style_constants_track_the_style_bits() {
        const FONT_SELECTED: Style = Style::BOLD.union(Style::ITALIC);
        let src = include_str!("shaders/terminal_ui_material.wgsl");
        let declared: BTreeSet<String> = src
            .lines()
            .map(str::trim)
            .filter(|line| line.starts_with("const STYLE_"))
            .map(str::to_owned)
            .collect();
        let expected: BTreeSet<String> = Style::all()
            .difference(FONT_SELECTED)
            .iter_names()
            .map(|(name, flag)| format!("const STYLE_{name}: u32 = {}u;", flag.bits()))
            .chain([format!(
                "const STYLE_WIDE_RIGHT_HALF: u32 = {STYLE_WIDE_RIGHT_HALF:#x}u;"
            )])
            .collect();
        assert_eq!(declared, expected);
    }

    /// Asserts that the underline-like combining marks promote to
    /// `Style::UNDERLINE`, the long stroke overlay to `Style::STRIKE`, and
    /// any other text to no flags.
    ///
    /// Case: a program decorates text with combining low lines and stroke
    /// overlays next to plain ASCII and accented text.
    #[test]
    fn combining_marks_promote_to_underline_and_strike() {
        assert_eq!(style_from_combining_marks("a"), Style::empty());
        assert_eq!(style_from_combining_marks("e\u{0301}"), Style::empty());
        for mark in ['\u{0331}', '\u{0332}', '\u{0333}'] {
            assert_eq!(
                style_from_combining_marks(&format!("a{mark}")),
                Style::UNDERLINE
            );
        }
        assert_eq!(style_from_combining_marks("a\u{0336}"), Style::STRIKE);
        assert_eq!(
            style_from_combining_marks("a\u{0332}\u{0336}"),
            Style::UNDERLINE | Style::STRIKE
        );
    }

    /// Asserts that in-viewport selection endpoints map to their
    /// viewport rows and the geometry maps to the shader encoding.
    ///
    /// Case: the user drags a whole-line selection across two visible rows
    /// at the live tail.
    #[test]
    fn selection_uniforms_projects_in_viewport_endpoints() {
        use crate::schema::{GridColumn, GridLine, GridPoint, SelectionGeometry, SelectionRange};
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
        use crate::schema::{GridColumn, GridLine, GridPoint, SelectionGeometry, SelectionRange};
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

    /// Asserts that fg and bg packing resolve symbolic colors through
    /// the live palette, and that the default background packs the
    /// transparent sentinel instead of the palette value.
    ///
    /// Case: a palette whose default foreground and indexed slot 1
    /// already hold custom colors packs cells for a webview overlay
    /// mounted behind default-background cells.
    #[test]
    fn cell_packing_resolves_through_the_live_palette() {
        use crate::schema::{Color as CellColor, Palette, Rgb};
        let mut palette = Palette {
            foreground: Rgb {
                r: 10,
                g: 20,
                b: 30,
            },
            ..Palette::default()
        };
        palette.indexed[1] = Rgb {
            r: 40,
            g: 50,
            b: 60,
        };
        let packed = PackedPalette::build(&palette);
        assert_eq!(
            packed.cell_fg(CellColor::DefaultForeground),
            pack_linear(Rgb {
                r: 10,
                g: 20,
                b: 30,
            })
        );
        assert_eq!(
            packed.cell_fg(CellColor::Indexed(1)),
            pack_linear(Rgb {
                r: 40,
                g: 50,
                b: 60,
            })
        );
        assert_eq!(packed.cell_bg(CellColor::DefaultBackground), TRANSPARENT_BG);
        assert_ne!(
            packed.cell_bg(CellColor::Rgb(palette.background)),
            TRANSPARENT_BG,
            "an explicit RGB equal to the palette background must stay opaque"
        );
    }
}
