//! Renderer-facing schema: the grid and hover state the renderer draws
//! from, in the terminal vocabulary of [`orzma_vt::prelude`].

mod grid;
mod hover;

pub use grid::*;
pub use hover::*;
pub use orzma_vt::prelude::{
    AnchoredPlacement, CURSOR_VISIBLE_BIT, Color, Cursor, CursorShape, DisplayOffset, GridColumn,
    GridLine, GridPoint, Hyperlink, HyperlinkId, HyperlinkUri, InstanceId, Palette, PlacementSize,
    Rgb, Row, Run, SelectionGeometry, SelectionKind, SelectionRange, Style, ViCursor, is_allowed,
};
