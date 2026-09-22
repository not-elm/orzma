use crate::{
    cursor::{
        CaretPaint, CaretPaintInput, CaretStyle, LastKeyInstant, PackedCursorStyle, blink_phase_on,
    },
    glyph::{
        atlas::{GlyphAtlas, GlyphRect},
        font::{
            CellMetrics, FontFace, GlyphKey, TerminalCellMetricsResource, TerminalFontSize,
            TerminalFonts, physical_font_size,
        },
    },
    material::state::TerminalMaterialState,
    schema::{
        Color as CellColor, GridCell, GridLine, GridSlot, HyperlinkHoverState, HyperlinkId,
        Palette, Rgb, SelectionGeometry, SelectionRange, Style, TerminalCells, TerminalView,
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
struct TerminalParams {
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
        cell_size_px: Vec2,
        atlas_size_px: Vec2,
        ascent_px: f32,
        dpr: f32,
        fallback: [u8; 3],
        hover_hyperlink_id: u32,
        hover_active: u32,
        caret: Option<CaretPaint>,
        cursor_thickness_phys: f32,
    ) -> Self {
        let cols = u32::from(view.cols);
        let rows = u32::from(view.rows);

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
            ascent_px,
            dpr,
            cursor_pos,
            cursor_style,
            cursor_thickness_phys,
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
    /// Packs each color slot of `palette` once.
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
        Ref<TerminalCells>,
        // NOTE: `view` is taken as a plain `&`, never `Ref`. Latching
        //       `view.is_changed()` into `grid_dirty` would make every cursor
        //       move, selection drag and IME toggle rebuild and re-upload the
        //       whole cell SSBO again — the defect the view/cells split removed.
        &TerminalView,
        Option<&PaneInactiveStyle>,
        Option<&TerminalOverlays>,
    )>,
    mut cell_metrics_res: ResMut<TerminalCellMetricsResource>,
    fonts: Res<TerminalFonts>,
    font_size: Res<TerminalFontSize>,
    cursor_config: Res<CaretStyle>,
    last_key: Res<LastKeyInstant>,
    time: Res<Time<Real>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    hover: Res<HyperlinkHoverState>,
    fallback: Res<TerminalPaddingFallback>,
) {
    // NOTE: This system runs unconditionally — *not* gated by
    // `Changed<TerminalCells>`. The `mat.params = ...` write at the end is
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
    let window = windows.single().ok();
    let dpr = window.map(|window| window.scale_factor());
    let phase_on = blink_phase_on(
        time.elapsed().saturating_sub(last_key.0),
        cursor_config.blink_interval,
        cursor_config.blink_timeout,
    );
    let window_focused = window.is_some_and(|window| window.focused);

    for (entity, handle, mut state, cells, view, pane_style, overlays) in terminals.iter_mut() {
        // NOTE: Latch the cells' change signal before the bail-out below.
        // Bevy clears it once this system has run, so cells written on a
        // frame that skips the upload would otherwise never reach the GPU.
        state.grid_dirty |= cells.is_changed();
        let Some(dpr) = dpr else {
            continue;
        };
        let phys_font_size = physical_font_size(font_size.0, dpr);
        let atlas_invalidated = atlas.generation != state.last_atlas_generation;
        let dims_changed = (view.cols, view.rows) != state.last_grid_dims;
        let grid_changed = state.grid_dirty;
        let phys_size_changed = phys_font_size != state.last_phys_font_size;

        let needs_rebuild = !state.initialized
            || grid_changed
            || atlas_invalidated
            || dims_changed
            || phys_size_changed;

        let metrics = resolve_metrics(
            &mut state,
            &mut cell_metrics_res,
            &fonts,
            phys_font_size,
            phys_size_changed,
            atlas_invalidated,
        );
        let cell_w_phys = metrics.advance_phys.floor().max(1.0);
        let cell_h_phys = metrics.line_height_phys.floor().max(1.0);
        let cell_size_phys = Vec2::new(cell_w_phys, cell_h_phys);
        let ascent_phys = metrics.ascent_phys.round();

        let Some((cells_handle, glyphs_handle)) = materials
            .get(&handle.0)
            .map(|m| (m.cells.clone(), m.glyphs.clone()))
        else {
            continue;
        };

        if needs_rebuild {
            upload_cells(
                &mut state,
                &mut atlas,
                &mut buffers,
                &cells,
                &fonts,
                (&cells_handle, &glyphs_handle),
                phys_font_size,
                (view.cols, view.rows),
            );
        }

        let (hover_hyperlink_id, hover_active) = match (hover.entity, hover.hyperlink_id) {
            (Some(e), Some(id)) if e == entity => {
                (id.get(), if hover.modifier_held { 1 } else { 0 })
            }
            _ => (0, 0),
        };
        let treatment = PaneTreatment::from_style(pane_style);
        let caret = CaretPaint::new(
            view.caret(),
            CaretPaintInput {
                suppressed: view.suppress_cursor,
                focused: window_focused && pane_style.is_none(),
                unfocused_hollow: cursor_config.unfocused_hollow,
                phase_on,
            },
        );
        let cursor_thickness_phys = (cursor_config.thickness * cell_size_phys.x)
            .round()
            .max(1.0);
        if let Some(mut mat) = materials.get_mut(&handle.0) {
            let mut params = TerminalParams::new(
                view,
                &cells.palette,
                &metrics,
                &treatment,
                cell_size_phys,
                Vec2::new(atlas.width() as f32, atlas.height() as f32),
                ascent_phys,
                dpr,
                fallback.0,
                hover_hyperlink_id,
                hover_active,
                caret,
                cursor_thickness_phys,
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

/// Resolves the cell metrics for `phys_font_size`, clearing the glyph
/// caches first when `phys_size_changed` or `atlas_invalidated` is set,
/// and refreshing the shared cell-metrics resource.
///
/// A `phys_size_changed` resolve also records `phys_font_size` as the
/// state's last physical size, so the next frame reports no change.
fn resolve_metrics(
    state: &mut TerminalMaterialState,
    cell_metrics: &mut TerminalCellMetricsResource,
    fonts: &TerminalFonts,
    phys_font_size: u16,
    phys_size_changed: bool,
    atlas_invalidated: bool,
) -> CellMetrics {
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

    // NOTE: Write the metrics back to TerminalCellMetricsResource so
    //       gui-side resize_terminals_to_node reads DPR-adjusted phys
    //       values on the next frame. The OR condition also catches
    //       the case where the Resource was reset externally (e.g.
    //       hot-reload) even if our local state matches.
    if phys_size_changed || cell_metrics.phys_font_size != phys_font_size {
        *cell_metrics = TerminalCellMetricsResource {
            metrics,
            phys_font_size,
        };
    }

    metrics
}

/// Rebuilds one terminal's cell and glyph buffers and uploads both,
/// then records the atlas generation and grid dimensions the upload was
/// built from.
///
/// `handles` is `(cells, glyphs)`. `dims` is `(cols, rows)` in cells. A
/// zero in either axis uploads the one-element dummy buffers wgpu
/// requires instead of an empty one.
///
/// Rows and columns of `cells` outside `dims` are ignored, and a slot
/// `cells` does not cover keeps the default cell.
fn upload_cells(
    state: &mut TerminalMaterialState,
    atlas: &mut GlyphAtlas,
    buffers: &mut Assets<ShaderBuffer>,
    cells: &TerminalCells,
    fonts: &TerminalFonts,
    handles: (&Handle<ShaderBuffer>, &Handle<ShaderBuffer>),
    phys_font_size: u16,
    dims: (u16, u16),
) {
    let (cols, rows) = (u32::from(dims.0), u32::from(dims.1));
    let (cells_handle, glyphs_handle) = handles;

    let cell_count = (cols * rows) as usize;
    state.cpu_cells.clear();
    state.cpu_cells.resize(cell_count, GpuCell::default());

    if cols > 0 && rows > 0 {
        rebuild_cells(state, atlas, cells, fonts, phys_font_size, (cols, rows));
    }

    if state.cpu_cells.is_empty() {
        state.cpu_cells.push(GpuCell::default());
    }
    if state.cpu_glyphs.is_empty() {
        state.cpu_glyphs.push(GpuGlyph::default());
    }

    if let Some(mut buf) = buffers.get_mut(cells_handle) {
        buf.set_data(&state.cpu_cells);
    }
    if let Some(mut buf) = buffers.get_mut(glyphs_handle) {
        buf.set_data(&state.cpu_glyphs);
    }

    state.last_atlas_generation = atlas.generation;
    state.grid_dirty = false;
    state.last_grid_dims = dims;
    state.initialized = true;
}

fn rebuild_cells(
    state: &mut TerminalMaterialState,
    atlas: &mut GlyphAtlas,
    cells: &TerminalCells,
    fonts: &TerminalFonts,
    phys_font_size: u16,
    dims: (u32, u32),
) {
    let restarts = atlas.restarts;
    fill_cells(state, atlas, cells, fonts, phys_font_size, dims);
    if atlas.restarts == restarts {
        return;
    }
    // NOTE: A restart during the pass wiped the texels every index
    // resolved before it points at; one more pass re-resolves them
    // against the restarted atlas. A second restart means the grid's
    // glyph set does not fit the atlas at all, so that pass is final.
    state.glyph_index_map.clear();
    state.cpu_glyphs.clear();
    fill_cells(state, atlas, cells, fonts, phys_font_size, dims);
}

/// Writes every visible cell's glyph index, color and style into the CPU
/// cell table, resolving each glyph through the atlas as it goes.
///
/// `dims` is `(cols, rows)` in cells. A row or column of `cells` outside
/// it is skipped without resolving its glyphs.
fn fill_cells(
    state: &mut TerminalMaterialState,
    atlas: &mut GlyphAtlas,
    cells: &TerminalCells,
    fonts: &TerminalFonts,
    phys_font_size: u16,
    dims: (u32, u32),
) {
    let (cols, rows) = dims;
    let packed_palette = PackedPalette::build(&cells.palette);
    for (row_idx, row) in cells.cells.iter().enumerate().take(rows as usize) {
        let mut left_half: Option<GpuCell> = None;
        for (col, slot) in row.iter().enumerate().take(cols as usize) {
            let col = col as u32;
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
                        hyperlink_id: cell.hyperlink.map_or(0, HyperlinkId::get),
                    };
                    if let Some(target) = state.cpu_cells.get_mut(target) {
                        *target = gpu;
                    }
                    left_half = Some(gpu);
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
                    if let Some(left) = left_half.take()
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

/// The line the shader draws for a combining mark, if any.
///
/// Maps U+0332 (combining low line), U+0333 (double low line), U+0331
/// (combining macron below) to `Style::UNDERLINE`, and U+0336 (combining
/// long stroke overlay) to `Style::STRIKE`.
fn line_style(mark: char) -> Option<Style> {
    match mark {
        '\u{0332}' | '\u{0333}' | '\u{0331}' => Some(Style::UNDERLINE),
        '\u{0336}' => Some(Style::STRIKE),
        _ => None,
    }
}

/// Promotes the combining marks in a cell's text that stand for lines to
/// the `Style` underline and strike flags so the shader paints them.
fn style_from_combining_marks(text: &str) -> Style {
    if text.is_ascii() {
        return Style::empty();
    }
    text.chars()
        .filter_map(line_style)
        .fold(Style::empty(), |acc, s| acc | s)
}

/// The marks of a cell's text that are composed onto its glyph: every
/// `char` after the first, except the marks [`line_style`] maps to a line.
fn composable_marks(text: &str) -> impl Iterator<Item = char> + '_ {
    text.chars().skip(1).filter(|c| line_style(*c).is_none())
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
    let key =
        GlyphKey::new(face, codepoint, phys_font_size).with_marks(composable_marks(&cell.text));
    if let Some(&idx) = state.glyph_index_map.get(&key) {
        return idx;
    }
    let Some(rect) = atlas.get_or_insert(key, fonts) else {
        return u32::MAX;
    };
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
        use crate::schema::{Color as CellColor, HyperlinkId};
        GridCell {
            text: text.to_string(),
            fg: CellColor::DefaultForeground,
            bg: CellColor::DefaultBackground,
            style: 0,
            hyperlink: link.map(|id| HyperlinkId::new(id).expect("nonzero")),
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

    fn uploaded(
        state: &mut TerminalMaterialState,
        buffers: &mut Assets<ShaderBuffer>,
        handles: (&Handle<ShaderBuffer>, &Handle<ShaderBuffer>),
        cells: &TerminalCells,
        dims: (u16, u16),
    ) {
        let mut atlas = GlyphAtlas::default();
        let fonts = TerminalFonts::default();
        upload_cells(state, &mut atlas, buffers, cells, &fonts, handles, 16, dims);
    }

    fn grid_of(rows: usize, cols: usize) -> TerminalCells {
        TerminalCells {
            cells: vec![vec![GridSlot::Cell(cell_with_link("x", None)); cols]; rows],
            ..Default::default()
        }
    }

    fn hyperlink_ids(cells: &[GpuCell]) -> Vec<u32> {
        cells.iter().map(|cell| cell.hyperlink_id).collect()
    }

    /// Asserts that an upload ignores the rows and columns outside its
    /// dimensions without resolving their glyphs, and leaves the slots
    /// the retained cells do not cover at their default, instead of
    /// panicking on the mismatch.
    ///
    /// Case: a malformed frame that also resizes the pane is rejected by
    /// the cells while the view takes the new size, so the next rebuilds
    /// see a grid of another shape than the view reports.
    #[test]
    fn upload_cells_clips_a_grid_that_disagrees_with_its_dims() {
        let mut buffers = Assets::<ShaderBuffer>::default();
        let cells_handle = buffers.add(ShaderBuffer::default());
        let glyphs_handle = buffers.add(ShaderBuffer::default());
        let linked = |id| GridSlot::Cell(cell_with_link("x", Some(id)));
        let larger = TerminalCells {
            cells: vec![
                vec![linked(1), linked(2), linked(3)],
                vec![GridSlot::Empty, linked(4)],
                vec![linked(5)],
                vec![GridSlot::Cell(cell_with_link("y", Some(6)))],
            ],
            ..Default::default()
        };
        let smaller = TerminalCells {
            cells: vec![vec![linked(7)]],
            ..Default::default()
        };
        let untouched = gpu_cell_fingerprint(&[GpuCell::default()])[0];
        let mut state = state_for(0);

        uploaded(
            &mut state,
            &mut buffers,
            (&cells_handle, &glyphs_handle),
            &larger,
            (2, 3),
        );
        assert_eq!(hyperlink_ids(&state.cpu_cells), [1, 2, 0, 4, 5, 0]);
        let fingerprint = gpu_cell_fingerprint(&state.cpu_cells);
        assert_eq!(fingerprint[2], untouched);
        assert_eq!(fingerprint[5], untouched);
        assert_eq!(
            state.cpu_glyphs.len(),
            1,
            "the row outside the dimensions resolves no glyph"
        );

        uploaded(
            &mut state,
            &mut buffers,
            (&cells_handle, &glyphs_handle),
            &smaller,
            (2, 3),
        );
        assert_eq!(hyperlink_ids(&state.cpu_cells), [7, 0, 0, 0, 0, 0]);
        let fingerprint = gpu_cell_fingerprint(&state.cpu_cells);
        assert!(fingerprint[1..].iter().all(|slot| *slot == untouched));
    }

    /// Asserts that every upload leaves the CPU cell table holding that
    /// upload's cells alone, and hands the buffer asset their encoding.
    ///
    /// Case: a pane redraws at an unchanged size, and the second frame
    /// blanks a cell that the first one painted.
    #[test]
    fn upload_cells_keeps_its_cpu_cell_table_across_uploads() {
        let mut buffers = Assets::<ShaderBuffer>::default();
        let cells_handle = buffers.add(ShaderBuffer::default());
        let glyphs_handle = buffers.add(ShaderBuffer::default());
        let painted = grid_of(2, 2);
        let mut blanked = grid_of(2, 2);
        blanked.cells[1][1] = GridSlot::Empty;
        let mut state = state_for(0);

        for cells in [&painted, &blanked] {
            uploaded(
                &mut state,
                &mut buffers,
                (&cells_handle, &glyphs_handle),
                cells,
                (2, 2),
            );
            assert_eq!(state.cpu_cells.len(), 4);
            let mut expected = ShaderBuffer::default();
            expected.set_data(&state.cpu_cells);
            let encoded = buffers
                .get(&cells_handle)
                .and_then(|buffer| buffer.data.as_ref());
            assert_eq!(encoded, expected.data.as_ref());
        }
        let untouched = gpu_cell_fingerprint(&[GpuCell::default()])[0];
        assert_eq!(gpu_cell_fingerprint(&state.cpu_cells)[3], untouched);
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
        let wide = cell_with_link("あ", Some(3));
        let combining = cell_with_link("e\u{0332}", None);
        let plain = cell_with_link("z", None);
        let cells = TerminalCells {
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

        rebuild_cells(&mut state, &mut atlas, &cells, &fonts, 16, (4, 1));

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
        let linked = cell_with_link("x", Some(7));
        let unlinked = cell_with_link("y", None);
        let cells = TerminalCells {
            cells: vec![vec![GridSlot::Cell(linked), GridSlot::Cell(unlinked)]],
            ..Default::default()
        };
        let mut state = state_for(2);
        let mut atlas = GlyphAtlas::default();
        let fonts = TerminalFonts::default();

        rebuild_cells(&mut state, &mut atlas, &cells, &fonts, 16, (2, 1));

        assert_eq!(state.cpu_cells[0].hyperlink_id, 7);
        assert_eq!(state.cpu_cells[1].hyperlink_id, 0);
    }

    /// Asserts that a cell resolved before a mid-rebuild atlas restart is
    /// re-resolved against the restarted atlas rather than keeping a
    /// glyph index into the rect the restart evicted.
    ///
    /// Case: a row's first glyph is already cached in the atlas, and its
    /// second glyph overflows a nearly full atlas mid-rebuild.
    #[test]
    fn rebuild_cells_survives_an_atlas_restart_mid_pass() {
        let m = cell_with_link("M", None);
        let a = cell_with_link("A", None);
        let cells = TerminalCells {
            cells: vec![vec![GridSlot::Cell(m), GridSlot::Cell(a)]],
            ..Default::default()
        };
        let mut state = state_for(2);
        let mut atlas = GlyphAtlas::new(32, 24);
        let fonts = TerminalFonts::default();
        // Pack 'M' (12x18) then 'W' (14x18) onto the first shelf so it
        // sits at x=26, leaving no room for 'A' (13x18) beside them and
        // no room below for its height either — resolving 'A' forces the
        // single restart this test exercises, evicting the 'M' rect cell
        // 0 already resolved against.
        atlas
            .get_or_insert(GlyphKey::new(FontFace::Regular, u32::from('M'), 24), &fonts)
            .expect("'M' rasterizes");
        atlas
            .get_or_insert(GlyphKey::new(FontFace::Regular, u32::from('W'), 24), &fonts)
            .expect("'W' rasterizes");
        assert_eq!(
            atlas.restarts, 0,
            "the filler glyphs must not restart the atlas"
        );

        rebuild_cells(&mut state, &mut atlas, &cells, &fonts, 24, (2, 1));

        for (col, ch) in [(0usize, 'M'), (1usize, 'A')] {
            let glyph_index = state.cpu_cells[col].glyph_index;
            let glyph = state.cpu_glyphs[glyph_index as usize];
            let key = GlyphKey::new(FontFace::Regular, u32::from(ch), 24);
            let rect = atlas.glyphs[&key];
            assert_eq!(
                glyph.uv_min,
                Vec2::new(rect.u as f32, rect.v as f32),
                "cell {col} ({ch:?}) glyph index must point at the restarted atlas's rect"
            );
        }
        assert_eq!(atlas.restarts, 1);
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

    /// Asserts that the shader's cursor helper consults the wide-right-half
    /// flag for both halves of a wide pair, and that a bar cursor moves to
    /// the body cell when parked on a wide glyph's right half.
    ///
    /// Case: a block cursor sits on a Japanese character, either on its
    /// body or parked on its right half, and a bar cursor is parked on the
    /// right half.
    #[test]
    fn wgsl_cursor_covers_both_halves_of_a_wide_glyph() {
        let src = include_str!("shaders/terminal_ui_material.wgsl");
        let body = wgsl_fn_body(src, "cursor_covers");
        assert!(body.contains("col == params.cursor_pos.x + 1u"));
        assert!(body.contains("col + 1u == params.cursor_pos.x"));
        let painter = wgsl_fn_body(src, "paint_cursor");
        assert!(painter.contains("cursor_covers(row, col)"));
        assert!(painter.contains("bar_covers(row, col)"));
        assert!(src.contains("fn bar_covers("));
        assert!(painter.contains("if cursor_shape == CURSOR_SHAPE_BAR"));
        assert!(painter.contains("on_cursor_cell = bar_covers(row, col);"));
        assert!(painter.contains("on_cursor_cell = cursor_covers(row, col);"));
    }

    /// Asserts that the marks the shader draws as lines are left out of
    /// the composable set, and that the base glyph is never one of them.
    ///
    /// Case: a cell holds `e` with an acute accent, a combining low line,
    /// a long stroke overlay and a tilde.
    #[test]
    fn composable_marks_skip_the_base_and_the_line_marks() {
        assert_eq!(
            composable_marks("e\u{0301}\u{0332}\u{0336}\u{0303}").collect::<Vec<_>>(),
            ['\u{0301}', '\u{0303}']
        );
        assert_eq!(composable_marks("a").count(), 0);
        assert_eq!(composable_marks("").count(), 0);
    }

    /// The text of WGSL function `name`, from the end of its name to its
    /// closing brace in column zero, under LF and CRLF line endings alike.
    fn wgsl_fn_body<'a>(src: &'a str, name: &str) -> &'a str {
        src.split(&format!("fn {name}("))
            .nth(1)
            .and_then(|rest| rest.split("\n}").next())
            .expect("the shader defines the function")
    }

    /// Asserts that a function body ends at its own closing brace under
    /// CRLF line endings rather than running on into the next function.
    ///
    /// Case: a Windows checkout converts the shader to CRLF before the
    /// tests embed it.
    #[test]
    fn wgsl_fn_body_ends_at_the_closing_brace_under_crlf() {
        let src =
            "fn first(\r\n) {\r\n    if x {\r\n    }\r\n}\r\nfn second() {\r\n    MARKER\r\n}\r\n";
        assert!(!wgsl_fn_body(src, "first").contains("MARKER"));
        assert!(wgsl_fn_body(src, "second").contains("MARKER"));
    }

    /// Asserts that the shader applies concealment as the last stage of
    /// color resolution, after reverse video and dim.
    ///
    /// Case: a program prints concealed text that is also reverse-video
    /// or faint.
    #[test]
    fn wgsl_concealment_is_the_last_color_resolution_stage() {
        let src = include_str!("shaders/terminal_ui_material.wgsl");
        assert!(wgsl_fn_body(src, "conceal").contains("STYLE_HIDDEN"));
        assert!(!wgsl_fn_body(src, "resolve_visible_colors").contains("STYLE_HIDDEN"));
        assert!(
            wgsl_fn_body(src, "resolve_cell_colors")
                .contains("conceal(cell, resolve_visible_colors(cell))")
        );
    }

    /// Asserts that a concealed glyph takes the tinted color its ground
    /// is painted in rather than the untinted cell background.
    ///
    /// Case: a program prints concealed text on a colored background in
    /// a pane that then loses focus and takes the inactive-pane tint.
    #[test]
    fn wgsl_concealment_follows_the_inactive_pane_tint() {
        let src = include_str!("shaders/terminal_ui_material.wgsl");
        assert!(wgsl_fn_body(src, "conceal").contains("CellColors(tint_bg(colors.bg), colors.bg)"));
    }

    /// Asserts that reverse video materializes the transparent default
    /// background through the shared helper rather than inline.
    ///
    /// Case: a program prints a reverse-video cell on the default
    /// background, and its glyph takes the colour that background paints.
    #[test]
    fn wgsl_reverse_video_materializes_the_default_background_through_the_helper() {
        let src = include_str!("shaders/terminal_ui_material.wgsl");
        let visible = wgsl_fn_body(src, "resolve_visible_colors");
        assert!(visible.contains("materialize_default_bg("));
        assert!(!visible.contains("bg_padding_color"));
        assert!(wgsl_fn_body(src, "materialize_default_bg").contains("params.bg_padding_color"));
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
            Vec2::new(8.0, 16.0),
            Vec2::new(64.0, 64.0),
            12.0,
            1.0,
            [0, 0, 0],
            0,
            0,
            None,
            2.0,
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
        let src = include_str!("shaders/terminal_ui_material.wgsl");
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

    /// Asserts that both paint paths resolve cell colors through the
    /// block-cursor override, leaving the plain resolution to the left
    /// neighbour's overdraw alone, and that the override conceals last.
    ///
    /// Case: a block cursor sits on a cell in the last column, so the
    /// grid path and the right-strip path both paint it.
    #[test]
    fn wgsl_block_cursor_is_resolved_into_the_cell_colors() {
        let src = include_str!("shaders/terminal_ui_material.wgsl");
        assert!(wgsl_fn_body(src, "paint_grid_cell").contains("resolve_painted_colors("));
        assert!(wgsl_fn_body(src, "paint_right_strip").contains("resolve_painted_colors("));
        assert_eq!(
            src.matches("resolve_cell_colors(").count(),
            2,
            "only the definition and paint_left_overdraw name the plain resolution"
        );
        let painted = wgsl_fn_body(src, "resolve_painted_colors");
        assert!(painted.contains("block_cursor_covers(row, col)"));
        assert!(painted.contains("conceal("));
        assert!(!painted.contains("STYLE_HIDDEN"));
        assert!(wgsl_fn_body(src, "block_cursor_covers").contains("cursor_covers(row, col)"));
    }

    /// Asserts that a hollow caret is left to the stroke painter rather
    /// than filling the cell it sits on.
    ///
    /// Case: a block caret sits on a cell in a pane that loses focus, so
    /// the caret is drawn as an outline.
    #[test]
    fn wgsl_a_hollow_block_caret_does_not_fill_the_cell() {
        let src = include_str!("shaders/terminal_ui_material.wgsl");
        assert!(wgsl_fn_body(src, "block_cursor_covers").contains("CURSOR_HOLLOW"));
    }

    /// Asserts that every stroked cursor — the bar, the underline and the
    /// hollow outline — takes the guarded fill color from the cell's
    /// colors before concealment, compared against the tinted ground, and
    /// that no pixel inversion is left in the shader.
    ///
    /// Case: an underline cursor sits on a concealed cell in an inactive
    /// pane, which also draws its caret hollow.
    #[test]
    fn wgsl_cursor_strips_take_the_guarded_fill_color() {
        let src = include_str!("shaders/terminal_ui_material.wgsl");
        let painter = wgsl_fn_body(src, "paint_cursor");
        assert!(painter.contains("let visible = resolve_visible_colors(cell);"));
        assert!(painter.contains("let ground = tint_bg(materialize_default_bg(visible.bg));"));
        assert!(painter.contains("let fill = guarded_fill(cursor_fill(visible.fg), ground);"));
        assert_eq!(painter.matches("return fill;").count(), 3);
        assert!(wgsl_fn_body(src, "cursor_fill").contains("params.cursor_packed"));
        assert!(!src.contains("1.0 - base.rgb"));
    }

    /// Asserts that the block cursor passes its fill through the contrast
    /// guard, which falls back to the default foreground or background
    /// against the cell's ground.
    ///
    /// Case: a light-theme editor leaves the cursor on a white cell whose
    /// foreground is also white, and a theme sets a cursor color close to
    /// a reverse-video cell's ground.
    #[test]
    fn wgsl_cursor_fill_is_guarded_against_the_ground() {
        let src = include_str!("shaders/terminal_ui_material.wgsl");
        let block = wgsl_fn_body(src, "resolve_painted_colors");
        assert!(block.contains("let ground = materialize_default_bg(colors.bg);"));
        assert!(block.contains("let fill = guarded_fill(cursor_fill(colors.fg), ground);"));
        let guard = wgsl_fn_body(src, "guarded_fill");
        assert!(guard.contains("contrast_ratio("));
        assert!(guard.contains("MIN_CURSOR_CONTRAST"));
        assert!(guard.contains("params.default_fg_packed"));
        assert!(guard.contains("materialize_default_bg("));
        assert!(src.contains("const MIN_CURSOR_CONTRAST: f32 = 1.5;"));
    }

    /// Asserts that under the block cursor a glyph that does not stand
    /// out from its own ground is painted in the guarded fill rather
    /// than in the ground.
    ///
    /// Case: a color picker draws a swatch as a full block whose
    /// foreground and background are the same color, and the block
    /// cursor lands on it.
    #[test]
    fn wgsl_block_cursor_paints_a_glyph_that_melts_into_its_ground_in_the_fill() {
        let src = include_str!("shaders/terminal_ui_material.wgsl");
        let block = wgsl_fn_body(src, "resolve_painted_colors");
        assert!(block.contains(
            "let glyph_melts = contrast_ratio(colors.fg.rgb, ground.rgb) < MIN_CURSOR_CONTRAST;"
        ));
        assert!(block.contains("CellColors(select(ground, fill, glyph_melts), fill)"));
    }

    /// Asserts that the shader declares no time uniform, so every
    /// blink phase reaching the GPU was decided on the CPU.
    ///
    /// Case: the user watches a caret blink while the terminal is
    /// otherwise idle.
    #[test]
    fn the_shader_declares_no_time_uniform() {
        let src = include_str!("shaders/terminal_ui_material.wgsl");
        assert!(!src.contains("time_seconds"));
    }

    /// Asserts that the shader's cursor bit constants match the Rust
    /// ones they decode.
    ///
    /// Case: a program selects a bar caret, and the pane it sits in
    /// goes inactive so the caret is drawn hollow as well.
    #[test]
    fn the_shader_cursor_bits_match_the_rust_constants() {
        let src = include_str!("shaders/terminal_ui_material.wgsl");
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

    /// Asserts that the hollow caret suppresses the inner edges of a
    /// wide pair, so an unfocused caret over a CJK character is one
    /// outline rather than two boxes.
    ///
    /// Case: the user switches panes while the caret sits on a CJK
    /// character.
    #[test]
    fn the_hollow_caret_spans_a_wide_pair_without_an_inner_seam() {
        let src = include_str!("shaders/terminal_ui_material.wgsl");
        assert!(src.contains("fn cursor_span_left("));
        assert!(src.contains("fn cursor_span_right("));
        let painter = wgsl_fn_body(src, "paint_cursor");
        assert!(painter.contains("col == cursor_span_left()"));
        assert!(painter.contains("col == cursor_span_right()"));
    }
}
