//! Vocabulary layer: type declarations and the impls closed over those
//! types.
//!
//! Every module here holds declarations plus their own inherent /
//! trait / `From` impls and associated-function constructors. Nothing
//! here carries state across calls, owns I/O, or mediates several
//! vocabulary types at once — that belongs to implementors of
//! [`crate::Vt`].
//!
//! Submodules are private and re-exported flat, so callers inside the
//! crate write `crate::schema::Color`, never `crate::schema::color::Color`.

mod signal;

pub use crate::device::color::{Color, Palette, Rgb};
pub use crate::device::modes::{MouseEncoding, MouseTracking, ScreenKind, VtModes};
pub use crate::frame::{DirtyRow, Frame};
pub use crate::hyperlink::{Hyperlink, HyperlinkId, HyperlinkUri, is_allowed};
pub use crate::interpreter::apc::ApcWebviewVerb;
pub use crate::placement::{PlacementId, ProjectedPlacement};
pub use crate::screen::cursor::{CURSOR_VISIBLE_BIT, Cursor, CursorShape};
pub use crate::screen::grid::GridSize;
pub use crate::screen::grid::coords::{GridColumn, GridLine, GridPoint, ScreenLine};
pub use crate::screen::grid::row::Row;
pub use crate::screen::grid::run::{Run, Style};
pub use crate::screen::viewport::{DisplayOffset, Scroll, ViewportLine};
pub use crate::selection::{CellSide, SelectionGeometry, SelectionKind, SelectionRange};
pub use crate::vi::{ViCursor, ViModeSwitch};
pub use signal::*;
