//! tmux control-mode (`tmux -CC`) client: a sans-io protocol core plus an
//! I/O-owning transport that drives a real tmux process.

pub use crate::command::TmuxCommand;
pub use crate::error::{TmuxError, TmuxResult};
pub use crate::protocol::{ClientEvent, CommandId, ProtocolClient};
pub use crate::session::SessionInfo;
pub use crate::transport::{TmuxClient, TmuxHandle, TmuxServer, TransportEvent};
pub use crate::window_list::WindowEntry;
pub use tmux_control_parser::ControlEvent;
pub use tmux_control_parser::SessionId;
pub use tmux_control_parser::WindowId;

mod command;
mod error;
mod protocol;
mod session;
mod transport;
mod window_list;
