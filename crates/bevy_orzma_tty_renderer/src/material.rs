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
