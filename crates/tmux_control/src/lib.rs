//! tmux control-mode (`tmux -CC`) client: a sans-io protocol core that turns
//! tmux's control-stream bytes into typed events and encodes outgoing commands.
//! The host owns the actual I/O (it adopts the user's own `tmux -CC` process
//! and pumps its PTY), so no process-spawning transport lives here.

pub use crate::command::TmuxCommand;
pub use crate::error::{TmuxError, TmuxResult};
pub use crate::protocol::{ClientEvent, CommandId, ProtocolClient, TransportEvent};
pub use tmux_control_parser::ControlEvent;
pub use tmux_control_parser::SessionId;
pub use tmux_control_parser::WindowId;

mod command;
mod error;
mod protocol;
