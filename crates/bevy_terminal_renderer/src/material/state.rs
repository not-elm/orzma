use crate::{
    glyph::{font::GlyphKey, AtlasImage},
    material::{GpuCell, GpuGlyph, TerminalParams, TerminalUiMaterial},
};
use bevy::{
    ecs::{lifecycle::HookContext, world::DeferredWorld},
    platform::collections::HashMap,
    prelude::*,
    render::storage::ShaderStorageBuffer,
};

/// Registers a `MaterialNode<TerminalUiMaterial>` on-add hook that seeds the SSBO buffers, attaches the glyph atlas image, and inserts the per-entity [`TerminalMaterialState`] cache.
pub struct TerminalMaterialStatePlugin;

impl Plugin for TerminalMaterialStatePlugin {
    fn build(&self, app: &mut App) {
        app.world_mut()
            .register_component_hooks::<MaterialNode<TerminalUiMaterial>>()
            .on_add(on_add_material_node);
    }
}

/// CPU-side cache mirroring what the GPU sees this frame.
///
/// SSBO handles and the atlas texture live on [`TerminalUiMaterial`]; this
/// component stores only the cached LUT and per-frame bookkeeping.
#[derive(Component)]
pub(crate) struct TerminalMaterialState {
    pub glyph_index_map: HashMap<GlyphKey, u32>,
    pub cpu_cells: Vec<GpuCell>,
    pub cpu_glyphs: Vec<GpuGlyph>,
    pub last_atlas_generation: u64,
    pub last_grid_seq: u32,
    pub last_grid_dims: (u16, u16),
    pub initialized: bool,
}

fn on_add_material_node(mut world: DeferredWorld, ctx: HookContext) {
    let material_handle = world
        .entity(ctx.entity)
        .get::<MaterialNode<TerminalUiMaterial>>()
        .expect("hook fires after MaterialNode<TerminalUiMaterial> insertion")
        .0
        .clone();
    let atlas_handle = world.resource::<AtlasImage>().handle.clone();

    // NOTE: Seed both storage buffers with one dummy element. wgpu rejects
    //       zero-sized storage buffers at bind time, so the bind group would
    //       fail to materialize before the first wire snapshot arrived and
    //       the whole material would silently drop out of the UI pass.
    let mut cells_seed = ShaderStorageBuffer::default();
    cells_seed.set_data(vec![GpuCell::default()]);
    let mut glyphs_seed = ShaderStorageBuffer::default();
    glyphs_seed.set_data(vec![GpuGlyph::default()]);

    let (cells_buffer, glyphs_buffer) = {
        let mut buffers = world.resource_mut::<Assets<ShaderStorageBuffer>>();
        (buffers.add(cells_seed), buffers.add(glyphs_seed))
    };

    if let Some(material) = world
        .resource_mut::<Assets<TerminalUiMaterial>>()
        .get_mut(&material_handle)
    {
        material.params = TerminalParams::default();
        material.cells = cells_buffer;
        material.glyphs = glyphs_buffer;
        material.atlas = atlas_handle;
    }

    world
        .commands()
        .entity(ctx.entity)
        .insert(TerminalMaterialState {
            glyph_index_map: HashMap::new(),
            cpu_cells: Vec::new(),
            cpu_glyphs: Vec::new(),
            last_atlas_generation: 0,
            last_grid_seq: 0,
            last_grid_dims: (0, 0),
            initialized: false,
        });
}
