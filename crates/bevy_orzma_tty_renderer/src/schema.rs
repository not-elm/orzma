//! Renderer-facing schema: the `TerminalGrid` component and hover
//! state, plus the shared terminal vocabulary re-exported flat from
//! [`orzma_vt::prelude`].

mod grid;
mod hover;

pub use grid::*;
pub use hover::*;
pub use orzma_vt::prelude::{
    AnchoredPlacement, CURSOR_VISIBLE_BIT, Color, Cursor, CursorShape, DisplayOffset, GridColumn,
    GridLine, GridPoint, Hyperlink, HyperlinkId, HyperlinkUri, InstanceId, Palette, PlacementSize,
    Rgb, Row, Run, SelectionGeometry, SelectionKind, SelectionRange, Style, ViCursor, is_allowed,
};
