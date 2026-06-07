use bevy::prelude::*;

mod bundle;
pub mod bundled;
pub mod glyph;
mod grid;
pub mod material;
pub mod schema;

pub use crate::glyph::font::{
    CellMetrics, FONT_SIZE_PX, FontFace, FontLoadError, TerminalCellMetricsResource,
    TerminalFontInitSet, TerminalFontPlugin, TerminalFonts,
};
pub use crate::grid::TerminalGridPlugin;
use crate::{
    glyph::TerminalGlyphPlugin, material::TerminalMaterialPlugin, schema::HyperlinkHoverState,
};

pub mod prelude {
    pub use crate::TerminalRendererPlugin;
    pub use crate::bundle::TerminalRenderBundle;
    pub use crate::material::PaneDim;
    pub use crate::schema::*;
}

pub struct TerminalRendererPlugin;

impl Plugin for TerminalRendererPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<HyperlinkHoverState>().add_plugins((
            TerminalGridPlugin,
            TerminalMaterialPlugin,
            TerminalGlyphPlugin,
        ));
    }
}
