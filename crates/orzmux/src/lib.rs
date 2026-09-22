//! The built-in terminal multiplexer backend: a Bevy-free thread that
//! owns every pane's PTY and VT plus the cell-unit layout tree, and
//! talks to the GUI over channels with plain-data commands and events.

pub mod backend;
pub mod client;
pub mod error;
pub mod layout;
pub mod protocol;
#[cfg(test)]
pub(crate) mod test_support;

pub mod prelude {
    pub use crate::client::{OrzmuxClient, OrzmuxConfig};
    pub use crate::error::*;
    pub use crate::protocol::*;
}
