use crate::{
    glyph::{AtlasImage, font::{CellMetrics, GlyphKey}},
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
    /// Last physical font size used for glyph rasterization. Reset to 0 in
    /// `on_add_material_node` so the first `update_terminal_material` for
    /// the entity sees `phys_size_changed == true` and triggers
    /// `invalidate_all()`.
    pub last_phys_font_size: u16,
    /// Schema version of the cells/glyphs SSBO data. `SCHEMA_VERSION_TIER1`
    /// is bumped whenever the renderer changes the meaning of `GpuGlyph`
    /// fields (e.g. logical-px → physical-px in this Tier 1 fix), forcing
    /// a one-time rebuild on first frame after upgrade.
    pub last_schema_version: u32,
    /// Cached output of `TerminalFonts::cell_metrics_px(last_phys_font_size)`
    /// to avoid re-parsing the `post` table on every frame.
    pub cached_metrics: Option<CellMetrics>,
    pub initialized: bool,
}

impl TerminalMaterialState {
    /// Resets all glyph-cache state so the next `update_terminal_material`
    /// invocation fully reuploads the atlas LUT, glyph rects, and atlas
    /// generation marker. Called on DPR change (`phys_font_size` changed)
    /// or schema upgrade (`last_schema_version != SCHEMA_VERSION_TIER1`).
    ///
    /// Deliberately does NOT touch:
    /// - `last_phys_font_size` / `last_schema_version` — the caller writes
    ///   those after invalidation so the next frame's diff detection still
    ///   works.
    /// - `cpu_cells` — re-populated wholesale by `rebuild_cells` on the
    ///   next rebuild (forced here by `last_grid_seq = 0`).
    /// - `initialized` — stays `true`; the rebuild path is re-entered via
    ///   `grid_changed`, not via `!initialized`.
    pub(crate) fn invalidate_all(&mut self) {
        self.glyph_index_map.clear();
        self.cpu_glyphs.clear();
        self.last_atlas_generation = 0;
        self.last_grid_seq = 0;
        self.cached_metrics = None;
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
            last_phys_font_size: 0,
            last_schema_version: 0,
            cached_metrics: None,
            initialized: false,
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glyph::font::{CellMetrics, FontFace, GlyphKey};

    fn populated_state() -> TerminalMaterialState {
        let mut state = TerminalMaterialState {
            glyph_index_map: HashMap::new(),
            cpu_cells: Vec::new(),
            cpu_glyphs: vec![GpuGlyph::default(), GpuGlyph::default()],
            last_atlas_generation: 42,
            last_grid_seq: 99,
            last_grid_dims: (80, 24),
            last_phys_font_size: 24,
            last_schema_version: 1,
            cached_metrics: Some(CellMetrics {
                advance_phys: 5.5,
                line_height_phys: 14.4,
                ascent_phys: 10.0,
                descent_phys: 2.4,
                underline_position_phys: -1.5,
                underline_thickness_phys: 1.0,
            }),
            initialized: true,
        };
        state.glyph_index_map.insert(
            GlyphKey { face: FontFace::Regular, codepoint: 'A' as u32, size_px: 24 },
            7,
        );
        state
    }

    #[test]
    fn invalidate_all_clears_lut_and_atlas_markers() {
        let mut state = populated_state();
        state.invalidate_all();
        assert!(state.glyph_index_map.is_empty());
        assert!(state.cpu_glyphs.is_empty());
        assert_eq!(state.last_atlas_generation, 0);
        assert_eq!(state.last_grid_seq, 0);
        assert!(state.cached_metrics.is_none());
    }

    #[test]
    fn invalidate_all_preserves_phys_font_size_and_schema_version() {
        let mut state = populated_state();
        state.invalidate_all();
        assert_eq!(state.last_phys_font_size, 24);
        assert_eq!(state.last_schema_version, 1);
        assert!(state.initialized);
    }
}
