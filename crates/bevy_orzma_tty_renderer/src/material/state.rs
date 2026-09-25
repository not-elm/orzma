use crate::{
    glyph::AtlasImage,
    material::{
        GpuCell, GpuGlyph, TerminalUiMaterial, params::TerminalParams,
        upload::TerminalMaterialState,
    },
};
use bevy::{
    ecs::{lifecycle::HookContext, world::DeferredWorld},
    prelude::*,
    render::storage::ShaderBuffer,
};

/// Seeds the SSBO buffers, attaches the glyph atlas image, and inserts the per-entity [`TerminalMaterialState`] cache whenever a `MaterialNode<TerminalUiMaterial>` is added.
pub struct TerminalMaterialStatePlugin;

impl Plugin for TerminalMaterialStatePlugin {
    fn build(&self, app: &mut App) {
        app.world_mut()
            .register_component_hooks::<MaterialNode<TerminalUiMaterial>>()
            .on_add(on_add_material_node);
    }
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
    let mut cells_seed = ShaderBuffer::default();
    cells_seed.set_data(vec![GpuCell::default()]);
    let mut glyphs_seed = ShaderBuffer::default();
    glyphs_seed.set_data(vec![GpuGlyph::default()]);

    let (cells_buffer, glyphs_buffer) = {
        let mut buffers = world.resource_mut::<Assets<ShaderBuffer>>();
        (buffers.add(cells_seed), buffers.add(glyphs_seed))
    };

    if let Some(mut material) = world
        .resource_mut::<Assets<TerminalUiMaterial>>()
        .get_mut(&material_handle)
    {
        material.params = TerminalParams::default();
        material.cells = cells_buffer.clone();
        material.glyphs = glyphs_buffer.clone();
        material.atlas = atlas_handle;
    }

    world
        .commands()
        .entity(ctx.entity)
        .insert(TerminalMaterialState::new(cells_buffer, glyphs_buffer));
}
