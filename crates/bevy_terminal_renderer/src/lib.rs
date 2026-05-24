use bevy::prelude::*;

use crate::{
    glyph::TerminalGlyphPlugin, grid::TerminalGridPlugin, material::TerminalMaterialPlugin,
};

mod bundle;
mod glyph;
mod grid;
pub mod material;
pub mod schema;

pub mod prelude {
    pub use crate::TerminalRendererPlugin;
    pub use crate::bundle::TerminalRenderBundle;
    pub use crate::schema::*;
}

pub struct TerminalRendererPlugin;

impl Plugin for TerminalRendererPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            TerminalGridPlugin,
            TerminalMaterialPlugin,
            TerminalGlyphPlugin,
        ));
    }
}
