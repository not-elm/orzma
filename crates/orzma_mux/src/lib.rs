//! The built-in terminal multiplexer backend: a Bevy-free thread that
//! owns every pane's PTY and VT plus the cell-unit layout tree, and
//! talks to the GUI over channels with plain-data commands and events.

pub mod layout;
pub mod protocol;

pub mod prelude {
    pub use crate::protocol::*;
}
