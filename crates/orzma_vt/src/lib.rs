//! Backend-agnostic terminal emulation for orzma.
//!
//! [`schema`] declares the vocabulary the crate speaks — grid cells,
//! colors, cursors, selections, damage, signals. [`vt`] holds the
//! [`vt::OrzmaVt`] contract and its backends.

pub mod schema;
pub mod vt;

pub mod prelude {
    pub use crate::{schema::*, vt::*};
}
