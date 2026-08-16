//! Renderer-facing schema: the Bevy-side frame events, the
//! `TerminalGrid` component, and hover state, plus the shared terminal
//! vocabulary re-exported flat from [`orzma_vt::schema`].

mod frame;
mod grid;
mod hover;

pub use frame::*;
pub use grid::*;
pub use hover::*;
pub use orzma_vt::schema::{
    CURSOR_VISIBLE_BIT, Color, Cursor, CursorShape, GridCell, GridColumn, GridLine, GridPoint,
    Hyperlink, HyperlinkId, HyperlinkUri, Row, Run, SelectionKind, SelectionRange, ViCursor,
};
