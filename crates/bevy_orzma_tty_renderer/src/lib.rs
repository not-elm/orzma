use crate::{
    cursor::CursorPlugin, glyph::TerminalGlyphPlugin, grid::TerminalGridPlugin,
    hyperlink::HyperlinkHoverState, material::TerminalMaterialPlugin,
};
use bevy::prelude::*;

pub mod bundled;
mod cursor;
mod error;
pub mod glyph;
mod grid;
mod hyperlink;
pub mod material;
mod pane_style;
mod system_set;

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
    pub use crate::grid::{TerminalCells, TerminalGridPlugin, TerminalView};
    pub use crate::hyperlink::HyperlinkHoverState;
    pub use crate::material::{
        OVERLAY_SLOTS, TerminalOverlays, TerminalPaddingFallback, TerminalUiMaterial,
    };
    pub use crate::pane_style::PaneInactiveStyle;
    pub use crate::system_set::TerminalMaterialSystems;
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
