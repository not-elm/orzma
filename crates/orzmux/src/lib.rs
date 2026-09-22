//! The built-in terminal multiplexer backend: a Bevy-free thread that
//! owns every pane's PTY and VT plus the cell-unit layout tree, and
//! talks to the GUI over channels with plain-data commands and events.

pub(crate) mod backend;
pub mod client;
pub mod error;
pub(crate) mod event_loop;
#[cfg(test)]
pub(crate) mod test_support;

pub mod prelude {
    pub use crate::backend::{
        CloseReason, CommandSeq, Layout, NewPaneAt, OrzmuxEvent, PaneDirection, PaneId, PaneRect,
        PaneTarget, RequestId, Separator, SplitId, SplitOrientation,
    };
    pub use crate::client::{OrzmuxClient, OrzmuxConfig};
    pub use crate::error::{OrzmuxError, OrzmuxResult};
    pub use crate::event_loop::OrzmuxCommand;
}
