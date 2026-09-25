//! The GPU terminal renderer: mirrors each pane's frames into components
//! and draws them through a UI material.

use crate::{
    cursor::CursorPlugin, font::TerminalFontPlugin, glyph::TerminalGlyphPlugin,
    grid::TerminalGridPlugin, hyperlink::HyperlinkHoverState, material::TerminalMaterialPlugin,
};
use bevy::prelude::*;

pub mod bundled;
mod cursor;
mod error;
mod font;
mod glyph;
mod grid;
mod hyperlink;
mod material;
mod pane_style;
mod system_set;

/// The renderer's public vocabulary, gathered for downstream crates.
pub mod prelude {
    pub use crate::TerminalRendererPlugin;
    pub use crate::cursor::{CaretStyle, CursorPlugin, LastKeyInstant, NextCaretFlip};
    pub use crate::error::{RendererError, RendererResult};
    pub use crate::font::{
        CellMetrics, FontFace, TerminalCellMetricsResource, TerminalFontInitSet,
        TerminalFontPlugin, TerminalFontSize, TerminalFonts, physical_font_size,
    };
    pub use crate::grid::{TerminalCells, TerminalGridPlugin, TerminalView};
    pub use crate::hyperlink::HyperlinkHoverState;
    pub use crate::material::{
        OVERLAY_SLOTS, TerminalOverlays, TerminalPaddingFallback, TerminalUiMaterial,
    };
    pub use crate::pane_style::PaneInactiveStyle;
    pub use crate::system_set::TerminalMaterialSystems;
}

/// Renders every terminal pane: mirrors its frames into components,
/// rasterizes its glyphs, and keeps its material and caret current.
pub struct TerminalRendererPlugin;

impl Plugin for TerminalRendererPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<HyperlinkHoverState>().add_plugins((
            TerminalGridPlugin,
            TerminalMaterialPlugin,
            TerminalGlyphPlugin,
            TerminalFontPlugin,
            CursorPlugin,
        ));
    }
}
