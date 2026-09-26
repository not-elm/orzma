//! Per-pane cell upload: rebuilds each terminal's cell and glyph buffers
//! when its cells or its size change, the glyph atlas restarts, or the font
//! size changes.

use crate::{
    error::{RendererError, RendererResult},
    font::{TerminalCellMetricsResource, TerminalFonts},
    glyph::{GlyphAtlas, GlyphKey},
    grid::{TerminalCells, TerminalView},
    material::{GpuCell, GpuGlyph, upload::fill::fill_cells},
    system_set::MaterialStage,
};
use bevy::{platform::collections::HashMap, prelude::*, render::storage::ShaderBuffer};

mod fill;
mod palette;
#[cfg(test)]
mod test_support;

/// Registers the per-pane cell upload.
pub(crate) struct CellUploadPlugin;

impl Plugin for CellUploadPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PostUpdate,
            upload_terminal_cells
                .in_set(MaterialStage::Upload)
                .run_if(resource_exists::<TerminalCellMetricsResource>),
        );
    }
}

/// CPU-side cache mirroring what one pane's cell and glyph buffers hold.
#[derive(Component)]
pub(crate) struct TerminalMaterialState {
    cells_buffer: Handle<ShaderBuffer>,
    glyphs_buffer: Handle<ShaderBuffer>,
    glyph_index_map: HashMap<GlyphKey, u32>,
    cpu_cells: Vec<GpuCell>,
    cpu_glyphs: Vec<GpuGlyph>,
    /// `None` until the buffers hold a valid build: before the first one,
    /// after a failed upload, and after a build during which the atlas
    /// restarted.
    uploaded: Option<UploadBasis>,
}

impl TerminalMaterialState {
    /// A cache that uploads into `cells_buffer` and `glyphs_buffer`, with
    /// nothing built yet.
    pub fn new(cells_buffer: Handle<ShaderBuffer>, glyphs_buffer: Handle<ShaderBuffer>) -> Self {
        Self {
            cells_buffer,
            glyphs_buffer,
            glyph_index_map: HashMap::new(),
            cpu_cells: Vec::new(),
            cpu_glyphs: Vec::new(),
            uploaded: None,
        }
    }

    /// Whether the buffers must be rebuilt for `basis`, given whether the
    /// pane's cells changed since the last run.
    fn needs_upload(&self, basis: UploadBasis, cells_changed: bool) -> bool {
        cells_changed || self.uploaded != Some(basis)
    }

    /// Rebuilds both buffers for `cells` at `basis`, recording `basis` only
    /// when the atlas did not restart during the build.
    ///
    /// The glyph table is kept only when the recorded build used the same
    /// atlas restart count and font size as `basis`, and a kept table that
    /// gained no glyph leaves the glyph buffer unwritten. A zero in either
    /// axis of `basis.dims` uploads the one-element buffers wgpu requires
    /// instead of empty ones, and rows and columns of `cells` outside the
    /// dimensions are ignored.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::MissingShaderBuffer`] when either buffer
    /// asset is missing. The atlas and the glyph table are left untouched,
    /// and the recorded build is forgotten, so the next run rebuilds the pane.
    fn upload(
        &mut self,
        atlas: &mut GlyphAtlas,
        buffers: &mut Assets<ShaderBuffer>,
        cells: &TerminalCells,
        fonts: &TerminalFonts,
        basis: UploadBasis,
    ) -> RendererResult {
        if !buffers.contains(&self.cells_buffer) || !buffers.contains(&self.glyphs_buffer) {
            self.uploaded = None;
            return Err(RendererError::MissingShaderBuffer);
        }
        let keeps_glyphs = self
            .uploaded
            .is_some_and(|built| built.keeps_glyphs_for(basis));
        if !keeps_glyphs {
            self.glyph_index_map.clear();
            self.cpu_glyphs.clear();
        }
        let kept_glyph_count = keeps_glyphs.then_some(self.cpu_glyphs.len());
        let (cols, rows) = (u32::from(basis.dims.0), u32::from(basis.dims.1));
        self.cpu_cells.clear();
        self.cpu_cells
            .resize((cols * rows) as usize, GpuCell::default());
        if cols > 0 && rows > 0 {
            fill_cells(
                self,
                atlas,
                cells,
                fonts,
                basis.phys_font_size,
                (cols, rows),
            );
        }
        if self.cpu_cells.is_empty() {
            self.cpu_cells.push(GpuCell::default());
        }
        if self.cpu_glyphs.is_empty() {
            self.cpu_glyphs.push(GpuGlyph::default());
        }
        if let Some(mut buffer) = buffers.get_mut(&self.cells_buffer) {
            buffer.set_data(&self.cpu_cells);
        }
        // NOTE: Skipping the write relies on the glyph table being
        //       append-only between clears. A kept table is the one the
        //       recorded build wrote in full, so an unchanged length means
        //       unchanged contents.
        if kept_glyph_count != Some(self.cpu_glyphs.len())
            && let Some(mut buffer) = buffers.get_mut(&self.glyphs_buffer)
        {
            buffer.set_data(&self.cpu_glyphs);
        }
        self.uploaded = (atlas.restarts == basis.atlas_restarts).then_some(basis);
        Ok(())
    }
}

/// What a pane's cell and glyph buffers were built from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct UploadBasis {
    /// The `(cols, rows)` of the pane's `TerminalView`.
    dims: (u16, u16),
    /// The `GlyphAtlas::restarts` every glyph index in the buffers is valid
    /// for.
    atlas_restarts: u64,
    /// The physical font size the glyphs were keyed at.
    phys_font_size: u16,
}

impl UploadBasis {
    /// The basis a pane showing `view` would be built from now.
    fn current(
        view: &TerminalView,
        atlas: &GlyphAtlas,
        metrics: &TerminalCellMetricsResource,
    ) -> Self {
        Self {
            dims: (view.cols, view.rows),
            atlas_restarts: atlas.restarts,
            phys_font_size: metrics.phys_font_size,
        }
    }

    /// Whether a glyph table built at this basis still indexes the atlas
    /// correctly at `other`.
    fn keeps_glyphs_for(self, other: Self) -> bool {
        self.atlas_restarts == other.atlas_restarts && self.phys_font_size == other.phys_font_size
    }
}

/// Rebuilds the buffers of every pane whose cells, size, atlas restart
/// count or font size changed; when a rebuild restarted the atlas, rebuilds
/// once more every pane that restart left stale. A failed upload is logged
/// and retried on the next run.
///
/// A restart during that second pass, which happens when the visible glyphs
/// of all panes do not fit the atlas together, leaves the panes the pass
/// already rebuilt stale until the next run. A pane whose own glyphs do not
/// fit the atlas stays unrecorded, so every run rebuilds it and restarts the
/// atlas.
///
/// TODO: grow the atlas instead of restarting it when it is full, so such a
/// pane settles.
fn upload_terminal_cells(
    mut atlas: ResMut<GlyphAtlas>,
    mut buffers: ResMut<Assets<ShaderBuffer>>,
    mut terminals: Query<(
        Entity,
        &mut TerminalMaterialState,
        Ref<TerminalCells>,
        &TerminalView,
    )>,
    fonts: Res<TerminalFonts>,
    metrics: Res<TerminalCellMetricsResource>,
) {
    let restarts_before = atlas.restarts;
    for after_restart in [false, true] {
        if after_restart && atlas.restarts == restarts_before {
            break;
        }
        for (entity, mut state, cells, view) in &mut terminals {
            let basis = UploadBasis::current(view, &atlas, &metrics);
            let cells_changed = !after_restart && cells.is_changed();
            if state.needs_upload(basis, cells_changed)
                && let Err(err) = state.upload(&mut atlas, &mut buffers, &cells, &fonts, basis)
            {
                warn!(terminal = ?entity, %err, "cell upload failed; the next run retries it");
            }
        }
    }
}

#[cfg(test)]
mod tests;
