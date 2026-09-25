use crate::{
    cursor::CursorPlugin, glyph::TerminalGlyphPlugin, grid::TerminalGridPlugin,
    material::TerminalMaterialPlugin, schema::HyperlinkHoverState,
};
use bevy::prelude::*;

pub mod bundled;
mod cursor;
mod error;
pub mod glyph;
mod grid;
pub mod material;
pub mod schema;

pub use crate::error::{RendererError, RendererResult};
pub use crate::glyph::font::{
    CellMetrics, FontFace, TerminalCellMetricsResource, TerminalFontInitSet, TerminalFontPlugin,
    TerminalFontSize, TerminalFonts, physical_font_size,
};
pub use material::TerminalPaddingFallback;

pub mod prelude {
    pub use crate::TerminalRendererPlugin;
    pub use crate::cursor::{CaretStyle, CursorPlugin, LastKeyInstant, NextCaretFlip};
    pub use crate::error::{RendererError, RendererResult};
    pub use crate::glyph::font::{
        CellMetrics, FontFace, TerminalCellMetricsResource, TerminalFontInitSet,
        TerminalFontPlugin, TerminalFontSize, TerminalFonts, physical_font_size,
    };
    pub use crate::grid::TerminalGridPlugin;
    pub use crate::material::{
        OVERLAY_SLOTS, PaneInactiveStyle, TerminalMaterialSystems, TerminalOverlays,
        TerminalPaddingFallback, TerminalUiMaterial,
    };
    pub use crate::schema::*;
}

pub struct TerminalRendererPlugin;

impl Plugin for TerminalRendererPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<HyperlinkHoverState>().add_plugins((
            TerminalGridPlugin,
            TerminalMaterialPlugin,
            TerminalGlyphPlugin,
            CursorPlugin,
        ));
    }
}
