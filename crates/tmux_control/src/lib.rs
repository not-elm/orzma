//! tmux control-mode (`tmux -CC`) client: a sans-io protocol core plus an
//! I/O-owning transport that drives a real tmux process.

pub use crate::error::{TmuxError, TmuxResult};
pub use crate::protocol::{ClientEvent, CommandId, ProtocolClient};
pub use crate::session::SessionInfo;
pub use crate::transport::{TmuxBuilder, TmuxClient, TmuxHandle, TransportEvent};
pub use tmux_control_parser::ControlEvent;
pub use tmux_control_parser::SessionId;

mod error;
mod protocol;
mod session;
mod transport;
