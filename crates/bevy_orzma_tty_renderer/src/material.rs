//! The terminal's UI material: its shader data layout and bind group, kept
//! current for every pane.

use crate::{
    glyph::{AtlasImage, GlyphRect},
    material::{
        params::{TerminalParams, TerminalParamsPlugin},
        upload::{CellUploadPlugin, TerminalMaterialState},
    },
    system_set::{MaterialStage, TerminalMaterialSystems},
};
use bevy::{
    asset::{AssetEventSystems, load_internal_asset, uuid_handle},
    prelude::*,
    render::{render_resource::ShaderType, storage::ShaderBuffer},
    shader::ShaderRef,
};
use bytemuck::{Pod, Zeroable};
use orzma_vt::prelude::{Rgb, Style};

mod bind_group;
mod overlay;
mod params;
#[cfg(test)]
mod shader;
mod upload;

pub use overlay::{OVERLAY_SLOTS, TerminalOverlays};
pub use params::TerminalPaddingFallback;

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
        app.add_plugins((
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

const TERMINAL_SHADER_HANDLE: Handle<Shader> = uuid_handle!("98195199-3092-42b6-b370-77dfc2ef83f9");

/// One GPU-side cell — 20 bytes, indexed `row * cols + col` in the storage buffer.
#[repr(C)]
#[derive(Clone, Copy, ShaderType, Pod, Zeroable, PartialEq, Debug)]
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
#[repr(C)]
#[derive(Clone, Copy, ShaderType, Pod, Zeroable, Default, PartialEq, Debug)]
struct GpuGlyph {
    /// Top-left of the glyph rect in atlas physical px.
    uv_min: Vec2,
    /// Bottom-right of the glyph rect in atlas physical px.
    uv_max: Vec2,
    /// Bearing from the glyph origin in physical px (positive Y goes down).
    offset_px: Vec2,
    /// Rasterized bitmap size in physical px.
    size_px: Vec2,
    /// [`GpuGlyph::COLOR`] when the sprite holds the glyph's own colors,
    /// otherwise 0.
    flags: u32,
    /// Zero padding up to the 40-byte stride of the shader's `Glyph` array.
    _pad: u32,
}

impl GpuGlyph {
    /// The flag of a sprite that holds the glyph's own colors rather than
    /// coverage.
    const COLOR: u32 = 1;

    /// Builds a `GpuGlyph` from an atlas rect, with all offsets and sizes
    /// in physical pixels.
    fn new(rect: GlyphRect) -> Self {
        Self {
            uv_min: Vec2::new(rect.u as f32, rect.v as f32),
            uv_max: Vec2::new((rect.u + rect.w) as f32, (rect.v + rect.h) as f32),
            offset_px: Vec2::new(rect.offset_x as f32, rect.offset_y as f32),
            size_px: Vec2::new(rect.w as f32, rect.h as f32),
            flags: if rect.color { Self::COLOR } else { 0 },
            _pad: 0,
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
    add: On<Add<MaterialNode<TerminalUiMaterial>>>,
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
    let cells_buffer = buffers.add(ShaderBuffer::from(vec![GpuCell::default()]));
    let glyphs_buffer = buffers.add(ShaderBuffer::from(vec![GpuGlyph::default()]));

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
    use bevy::render::render_resource::encase::{StorageBuffer, internal::WriteInto};
    use std::collections::BTreeSet;
    use std::mem::{offset_of, size_of};

    #[test]
    fn gpu_cell_is_twenty_bytes() {
        assert_eq!(size_of::<GpuCell>(), 20);
    }

    /// Asserts that a `GpuGlyph` occupies 40 bytes in its shader layout, the
    /// stride of the shader's `Glyph` array.
    ///
    /// Case: a pane shows an emoji next to ordinary text, so the renderer
    /// uploads a glyph table that holds both the color record and the text
    /// records.
    #[test]
    fn gpu_glyph_is_forty_bytes() {
        assert_eq!(<GpuGlyph as ShaderType>::min_size().get(), 40);
    }

    /// Asserts that `GpuCell` and `GpuGlyph` hold the shader's `Cell` and
    /// `Glyph` fields in the shader's order and at the shader's offsets, and
    /// that their bytes equal their shader encoding.
    ///
    /// Case: a pane shows an emoji next to underlined text, so the shader
    /// reads the color flag and the style bits from records the CPU wrote.
    #[test]
    fn gpu_records_match_the_shader_layout() {
        assert_eq!(
            wgsl_struct_fields("Cell"),
            [
                "glyph_index",
                "fg_packed",
                "bg_packed",
                "style_flags",
                "hyperlink_id"
            ]
        );
        assert_eq!(
            [
                offset_of!(GpuCell, glyph_index),
                offset_of!(GpuCell, fg_packed),
                offset_of!(GpuCell, bg_packed),
                offset_of!(GpuCell, style_flags),
                offset_of!(GpuCell, hyperlink_id),
            ],
            [0, 4, 8, 12, 16]
        );
        assert_eq!(
            wgsl_struct_fields("Glyph"),
            ["uv_min", "uv_max", "offset_px", "size_px", "flags"]
        );
        assert_eq!(
            [
                offset_of!(GpuGlyph, uv_min),
                offset_of!(GpuGlyph, uv_max),
                offset_of!(GpuGlyph, offset_px),
                offset_of!(GpuGlyph, size_px),
                offset_of!(GpuGlyph, flags),
            ],
            [0, 8, 16, 24, 32]
        );
        assert_eq!(
            size_of::<GpuCell>(),
            <GpuCell as ShaderType>::min_size().get() as usize
        );
        assert_eq!(
            size_of::<GpuGlyph>(),
            <GpuGlyph as ShaderType>::min_size().get() as usize
        );

        let cell = GpuCell {
            glyph_index: 1,
            fg_packed: 2,
            bg_packed: 3,
            style_flags: 4,
            hyperlink_id: 5,
        };
        let glyph = GpuGlyph {
            uv_min: Vec2::new(1.0, 2.0),
            uv_max: Vec2::new(3.0, 4.0),
            offset_px: Vec2::new(5.0, 6.0),
            size_px: Vec2::new(7.0, 8.0),
            flags: GpuGlyph::COLOR,
            _pad: 0,
        };
        assert_eq!(shader_encoding(&cell), bytemuck::bytes_of(&cell));
        assert_eq!(shader_encoding(&glyph), bytemuck::bytes_of(&glyph));
    }

    /// The bytes encase writes for `value` in a storage buffer.
    fn shader_encoding<T: ShaderType + WriteInto>(value: &T) -> Vec<u8> {
        let mut buffer = StorageBuffer::new(Vec::<u8>::new());
        buffer
            .write(value)
            .expect("a Vec-backed buffer grows to fit");
        buffer.into_inner()
    }

    /// The field names of the shader struct `name`, in declaration order.
    fn wgsl_struct_fields(name: &str) -> Vec<&'static str> {
        include_str!("shaders/terminal_ui_material.wgsl")
            .split(&format!("struct {name} {{"))
            .nth(1)
            .and_then(|rest| rest.split("};").next())
            .expect("the shader declares the struct")
            .lines()
            .filter_map(|line| line.trim().split(':').next())
            .map(str::trim)
            .filter(|field| !field.is_empty())
            .collect()
    }

    #[test]
    fn gpu_cell_default_has_zero_hyperlink_id() {
        let cell = GpuCell::default();
        assert_eq!(cell.hyperlink_id, 0);
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

    /// Asserts that a glyph record carries the color flag exactly when its
    /// atlas rect holds a color sprite, and that the shader declares the
    /// flag with the same bit.
    ///
    /// Case: a row shows an emoji next to a letter.
    #[test]
    fn a_color_rect_sets_the_color_flag_the_shader_reads() {
        let rect = GlyphRect {
            u: 0,
            v: 0,
            w: 24,
            h: 24,
            offset_x: 0,
            offset_y: -21,
            color: true,
        };

        assert_eq!(GpuGlyph::new(rect).flags, GpuGlyph::COLOR);
        assert_eq!(
            GpuGlyph::new(GlyphRect {
                color: false,
                ..rect
            })
            .flags,
            0
        );
        let src = include_str!("shaders/terminal_ui_material.wgsl");
        assert!(src.contains(&format!("const GLYPH_COLOR: u32 = {}u;", GpuGlyph::COLOR)));
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
                    .is_some_and(|seed| seed.buffer_size() > 0),
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
        use crate::font::{TerminalCellMetricsResource, TerminalFonts};
        use crate::glyph::GlyphAtlas;
        use crate::grid::{TerminalCells, TerminalView};
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

        let cells = cells_buffer
            .cast_slice::<GpuCell>()
            .expect("the upload wrote the cell buffer");
        assert_eq!(cells.len(), 1, "one cell for a 1×1 view");
        assert_ne!(
            cells,
            [GpuCell::default()],
            "the upload must replace the seed cell"
        );
        let glyphs = glyphs_buffer
            .cast_slice::<GpuGlyph>()
            .expect("the upload wrote the glyph buffer");
        assert_eq!(glyphs.len(), 1, "one glyph for one character");
        assert_ne!(
            glyphs,
            [GpuGlyph::default()],
            "the upload must replace the seed glyph"
        );
    }
}
