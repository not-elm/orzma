//! Renderer-facing schema: the Bevy-side frame events, the
//! `TerminalGrid` component, and hover state, plus the shared terminal
//! vocabulary re-exported flat from [`orzma_vt::prelude`].

mod frame;
mod grid;
mod hover;

pub use frame::*;
pub use grid::*;
pub use hover::*;
pub use orzma_vt::prelude::{
    CURSOR_VISIBLE_BIT, Color, Cursor, CursorShape, DisplayOffset, GridColumn, GridLine, GridPoint,
    Hyperlink, HyperlinkId, HyperlinkUri, Palette, PlacementId, ProjectedPlacement, Rgb, Row, Run,
    SelectionGeometry, SelectionKind, SelectionRange, Style, ViCursor, is_allowed,
};
