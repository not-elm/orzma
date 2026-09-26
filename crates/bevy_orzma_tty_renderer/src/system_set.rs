//! The system sets the renderer's per-update material work runs in.

use bevy::prelude::SystemSet;

/// Ordering anchor for the systems that write each terminal's material.
///
/// A system that resizes a terminal's grid from the layout must run
/// `.before(Self::UpdateMaterial)`. The set runs before
/// `AssetEventSystems`, so the asset writes it makes reach the render world
/// in the same update.
#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone)]
pub enum TerminalMaterialSystems {
    /// The systems that resolve the cell metrics and write each terminal's
    /// cell buffers and uniforms.
    UpdateMaterial,
}

/// Ordering stages inside [`TerminalMaterialSystems::UpdateMaterial`], run
/// in declaration order.
#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone)]
pub(crate) enum MaterialStage {
    /// Resolves the shared cell metrics.
    Metrics,
    /// Rebuilds and uploads each pane's cell and glyph buffers.
    Upload,
    /// Writes each pane's material uniforms.
    Params,
}
