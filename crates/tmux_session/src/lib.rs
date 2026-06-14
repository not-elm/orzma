//! ozmux ⇄ tmux control-mode integration: owns a `tmux -CC` connection,
//! drains its transport events into the Bevy world, tracks the connection
//! lifecycle, and projects tmux session/window/pane state as ECS entities
//! (`TmuxSession`/`TmuxWindow`/`TmuxPane`) reconciled from a `ProjectionModel`
//! maintained by the control-event reducer. Rendering is not done here.

mod components;
mod connect;
mod connection;
mod enumerate;
mod event_pump;
mod model;
mod plugin;
mod reconcile;
mod select;
mod state;

pub use components::{TmuxPane, TmuxSession, TmuxWindow};
pub use connect::attach_or_create;
pub use connection::TmuxConnection;
pub use enumerate::{LIST_WINDOWS_FORMAT, WindowRow, parse_window_rows};
pub use model::{PaneModel, ProjectionModel, WindowModel, pane_leaves};
pub use plugin::TmuxSessionPlugin;
pub use reconcile::TmuxProjection;
pub use select::{AttachTarget, select_attach_target};
pub use state::ConnectionState;
