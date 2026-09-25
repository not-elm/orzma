//! The terminal's UI material: its shader data layout and bind group, kept
//! current for every pane.

use crate::{
    glyph::{AtlasImage, atlas::GlyphRect},
    material::{
        params::{TerminalParams, TerminalParamsPlugin},
        upload::{CellUploadPlugin, TerminalMaterialState},
    },
    system_set::{MaterialStage, TerminalMaterialSystems},
};
use bevy::{
    asset::{AssetEventSystems, load_internal_asset, uuid_handle},
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
};
use orzma_vt::prelude::{Rgb, Style};

mod params;
mod upload;

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
            .add_plugins((
                UiMaterialPlugin::<TerminalUiMaterial>::default(),
                CellUploadPlugin,
                TerminalParamsPlugin,
            ))
            .configure_sets(
                PostUpdate,
                (
                    MaterialStage::Metrics,
                    MaterialStage::Upload,
                    MaterialStage::Params,
                )
                    .chain()
                    .in_set(TerminalMaterialSystems::UpdateMaterial),
            )
            // NOTE: An asset write that lands after `AssetEventSystems` is
            // extracted only on a later update, which on-demand redraw may not
            // run until the next wake.
            .configure_sets(
                PostUpdate,
                TerminalMaterialSystems::UpdateMaterial.before(AssetEventSystems),
            )
            .add_observer(init_material_node);
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

/// sRGB byte triple → the opaque linear u32 packing the shader decodes.
fn pack_linear(rgb: Rgb) -> u32 {
    Color::srgb_u8(rgb.r, rgb.g, rgb.b).to_linear().as_u32()
}

/// Seeds a new terminal node's material with one-element cell and glyph
/// buffers, the glyph atlas and default uniforms, and gives the node the
/// upload cache for those buffers. A node whose material asset is missing
/// is left uninitialized.
fn init_material_node(
    add: On<Add, MaterialNode<TerminalUiMaterial>>,
    mut commands: Commands,
    mut buffers: ResMut<Assets<ShaderBuffer>>,
    mut materials: ResMut<Assets<TerminalUiMaterial>>,
    nodes: Query<&MaterialNode<TerminalUiMaterial>>,
    atlas_image: Res<AtlasImage>,
) {
    let entity = add.event_target();
    let Ok(node) = nodes.get(entity) else {
        warn!(
            ?entity,
            "a terminal material node vanished before it was initialized"
        );
        return;
    };
    let Some(mut material) = materials.get_mut(&node.0) else {
        warn!(
            ?entity,
            "a terminal material node has no material asset; it is not initialized"
        );
        return;
    };

    // NOTE: Seed both storage buffers with one dummy element. wgpu rejects
    //       zero-sized storage buffers at bind time, so the bind group would
    //       fail to materialize before the first wire snapshot arrived and
    //       the whole material would silently drop out of the UI pass.
    let mut cells_seed = ShaderBuffer::default();
    cells_seed.set_data(vec![GpuCell::default()]);
    let mut glyphs_seed = ShaderBuffer::default();
    glyphs_seed.set_data(vec![GpuGlyph::default()]);
    let cells_buffer = buffers.add(cells_seed);
    let glyphs_buffer = buffers.add(glyphs_seed);

    material.params = TerminalParams::default();
    material.cells = cells_buffer.clone();
    material.glyphs = glyphs_buffer.clone();
    material.atlas = atlas_image.handle.clone();
    commands
        .entity(entity)
        .insert(TerminalMaterialState::new(cells_buffer, glyphs_buffer));
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
        assert!(body.contains("col == params.cursor_pos.x + 1u && wide_right_half_at(row, col)"));
        assert!(body.contains(
            "col + 1u == params.cursor_pos.x && wide_right_half_at(row, params.cursor_pos.x)"
        ));
        assert!(
            wgsl_fn_body(src, "wide_right_half_at")
                .contains("is_wide_right_half(cells[idx].style_flags)")
        );
        assert!(wgsl_fn_body(src, "is_wide_right_half").contains("STYLE_WIDE_RIGHT_HALF"));
        assert!(wgsl_fn_body(src, "paint_cursor").contains("stroked_cursor_covers(row, col)"));
        let covers = wgsl_fn_body(src, "stroked_cursor_covers");
        assert!(covers.contains("if cursor_shape() == CURSOR_SHAPE_BAR"));
        assert!(covers.contains("return bar_covers(row, col);"));
        assert!(covers.contains("return cursor_covers(row, col);"));
        assert!(wgsl_fn_body(src, "bar_covers").contains("col == cursor_span_left()"));
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
        assert!(visible.contains("reverse_video(colors)"));
        assert!(!visible.contains("bg_padding_color"));
        let reverse = wgsl_fn_body(src, "reverse_video");
        assert!(reverse.contains("materialize_default_bg("));
        assert!(!reverse.contains("bg_padding_color"));
        assert!(wgsl_fn_body(src, "materialize_default_bg").contains("params.bg_padding_color"));
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
        assert!(wgsl_fn_body(src, "block_cursor_covers").contains("!cursor_is_hollow()"));
        assert!(wgsl_fn_body(src, "cursor_is_hollow").contains("CURSOR_HOLLOW"));
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
        assert!(painter.contains("!on_cursor_stroke(col, in_cell_px)"));
        assert!(painter.contains("return cursor_stroke_fill(cell);"));
        let stroke = wgsl_fn_body(src, "on_cursor_stroke");
        assert!(stroke.contains("on_hollow_outline(col, in_cell_px)"));
        assert!(stroke.contains("CURSOR_SHAPE_UNDERLINE"));
        assert!(stroke.contains("CURSOR_SHAPE_BAR"));
        let fill = wgsl_fn_body(src, "cursor_stroke_fill");
        assert!(fill.contains("let visible = resolve_visible_colors(cell);"));
        assert!(fill.contains("let ground = tint_bg(materialize_default_bg(visible.bg));"));
        assert!(fill.contains("return guarded_fill(cursor_fill(visible.fg), ground);"));
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
        assert!(
            wgsl_fn_body(src, "resolve_painted_colors")
                .contains("colors = under_block_cursor(colors);")
        );
        let block = wgsl_fn_body(src, "under_block_cursor");
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
        let block = wgsl_fn_body(src, "under_block_cursor");
        assert!(block.contains(
            "let glyph_melts = contrast_ratio(colors.fg.rgb, ground.rgb) < MIN_CURSOR_CONTRAST;"
        ));
        assert!(block.contains("CellColors(select(ground, fill, glyph_melts), fill)"));
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
        let outline = wgsl_fn_body(src, "on_hollow_outline");
        assert!(outline.contains("col == cursor_span_left()"));
        assert!(outline.contains("col == cursor_span_right()"));
    }

    const SEEDED_ATLAS: Handle<Image> = uuid_handle!("c0fee000-0000-4000-8000-000000000003");

    /// An app that seeds material nodes through `init_material_node`, with
    /// one material ready for a node to use.
    fn material_node_app() -> (App, Handle<TerminalUiMaterial>) {
        let mut app = App::new();
        app.init_resource::<Assets<ShaderBuffer>>()
            .init_resource::<Assets<TerminalUiMaterial>>()
            .insert_resource(AtlasImage {
                handle: SEEDED_ATLAS,
                last_generation: 0,
            })
            .add_observer(init_material_node);
        let material = app
            .world_mut()
            .resource_mut::<Assets<TerminalUiMaterial>>()
            .add(TerminalUiMaterial::default());
        (app, material)
    }

    /// Asserts that a new terminal node gets its upload cache and a
    /// material seeded with one-element cell and glyph buffers and the glyph
    /// atlas.
    ///
    /// Case: the shell surface spawns the first terminal node at startup.
    #[test]
    fn a_new_material_node_is_seeded_and_given_its_cache() {
        let (mut app, material) = material_node_app();
        let node = app.world_mut().spawn(MaterialNode(material.clone())).id();
        app.update();

        assert!(app.world().entity(node).contains::<TerminalMaterialState>());
        let seeded = app
            .world()
            .resource::<Assets<TerminalUiMaterial>>()
            .get(&material)
            .expect("the node's material");
        assert_eq!(seeded.atlas, SEEDED_ATLAS);
        let buffers = app.world().resource::<Assets<ShaderBuffer>>();
        for buffer in [&seeded.cells, &seeded.glyphs] {
            assert!(
                buffers
                    .get(buffer)
                    .and_then(|seed| seed.data.as_ref())
                    .is_some_and(|data| !data.is_empty()),
                "a seeded buffer holds one element"
            );
        }
    }

    /// Asserts that an upload writes the cell array into the buffer the
    /// material binds as `cells`, and the glyph array into the buffer it
    /// binds as `glyphs`.
    ///
    /// Case: a terminal node is created and renders its very first
    /// character.
    #[test]
    fn an_upload_lands_in_the_buffers_its_material_binds() {
        use crate::glyph::atlas::GlyphAtlas;
        use crate::glyph::font::{TerminalCellMetricsResource, TerminalFonts};
        use crate::schema::{TerminalCells, TerminalView};
        use orzma_vt::prelude::Cell;

        let fonts = TerminalFonts::default();
        let (mut app, material) = material_node_app();
        app.insert_resource(TerminalCellMetricsResource::new(&fonts, 16))
            .insert_resource(fonts)
            .insert_resource(GlyphAtlas::default())
            .add_plugins(CellUploadPlugin);

        let view = TerminalView {
            cols: 1,
            rows: 1,
            ..Default::default()
        };
        let cells = TerminalCells {
            cells: vec![vec![Cell {
                c: 'x',
                ..Cell::default()
            }]],
            ..Default::default()
        };
        app.world_mut()
            .spawn((MaterialNode(material.clone()), view, cells));
        app.update();

        let mut cell_seed = ShaderBuffer::default();
        cell_seed.set_data(vec![GpuCell::default()]);
        let mut glyph_seed = ShaderBuffer::default();
        glyph_seed.set_data(vec![GpuGlyph::default()]);

        let seeded = app
            .world()
            .resource::<Assets<TerminalUiMaterial>>()
            .get(&material)
            .expect("the node's material");
        let buffers = app.world().resource::<Assets<ShaderBuffer>>();
        let cells_buffer = buffers.get(&seeded.cells).expect("the cells asset exists");
        let glyphs_buffer = buffers
            .get(&seeded.glyphs)
            .expect("the glyphs asset exists");

        assert_ne!(
            cells_buffer.data, cell_seed.data,
            "the upload must replace the seed cell"
        );
        assert_eq!(
            cells_buffer.data.as_ref().map(Vec::len),
            cell_seed.data.as_ref().map(Vec::len),
            "a handle swap would land the glyph array's length here instead"
        );
        assert_ne!(
            glyphs_buffer.data, glyph_seed.data,
            "the upload must replace the seed glyph"
        );
        assert_eq!(
            glyphs_buffer.data.as_ref().map(Vec::len),
            glyph_seed.data.as_ref().map(Vec::len),
            "a handle swap would land the cell array's length here instead"
        );
    }
}
