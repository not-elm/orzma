//! ozmux ⇄ tmux control-mode integration: owns a `tmux -CC` connection,
//! drains its transport events into the Bevy world, and tracks the
//! connection lifecycle. Phase 0 wires the plumbing and logs events; it
//! does not project entities or auto-connect.

mod connect;
mod connection;
mod enumerate;
mod event_pump;
mod model;
mod plugin;
mod select;
mod state;

pub use connect::attach_or_create;
pub use connection::TmuxConnection;
pub use enumerate::{LIST_WINDOWS_FORMAT, WindowRow, parse_window_rows};
pub use model::{PaneModel, ProjectionModel, WindowModel, pane_leaves};
pub use plugin::TmuxSessionPlugin;
pub use select::{AttachTarget, select_attach_target};
pub use state::ConnectionState;
