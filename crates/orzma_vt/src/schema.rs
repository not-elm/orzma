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

mod color;
mod cursor;
mod grid;
mod modes;
mod scroll;
mod selection;
mod signal;
mod vi;
mod webview;

pub use crate::frame::{DirtyRow, Frame};
pub use crate::hyperlink::{Hyperlink, HyperlinkId, HyperlinkUri, is_allowed};
pub use crate::screen::grid::row::Row;
pub use crate::screen::grid::run::{Run, Style};
pub use crate::screen::viewport::DisplayOffset;
pub use color::*;
pub use cursor::*;
pub use grid::*;
pub use modes::*;
pub use scroll::*;
pub use selection::*;
pub use signal::*;
pub use vi::*;
pub use webview::*;
