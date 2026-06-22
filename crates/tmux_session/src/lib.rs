//! ozmux ⇄ tmux control-mode integration: owns a `tmux -CC` connection,
//! drains its transport events into the Bevy world, tracks the connection
//! lifecycle, and projects tmux session/window/pane state as ECS entities
//! (`TmuxSession`/`TmuxWindow`/`TmuxPane`). The drain system translates each
//! transport batch into global projection events that observers apply directly
//! to the world. Rendering is not done here.

mod command;
mod components;
mod connect;
mod connection;
mod copy_queries;
mod enumerate;
mod event_pump;
mod events;
mod input;
mod keybindings;
mod observers;
mod output;
mod plugin;
mod select;
mod state;

pub use command::{
    CopyModeCapture, CopyStateQuery, Prompt, RefreshClient, RenameSession, RenameWindow,
    ResizePaneX, ResizePaneY, ResizeWindow, SelectPane, SelectWindow, SendBytes, SendPaneKeys,
    SetEnvironmentInSession, ShowBuffer, SwitchClient, WindowRefreshClient,
};
pub use components::{
    ActivePane, ActiveWindow, TmuxPane, TmuxSession, TmuxWindow, TmuxWindowLayout, WindowFlags,
};
pub use connect::attach_or_create;
pub use connection::{AdoptedHandle, TmuxConnection};
pub use copy_queries::{CopyModeQueries, CopyModeReply, CopyQueryKind};
pub use enumerate::{
    CopyState, LIST_WINDOWS_FORMAT, WindowRow, absolute_to_visible_row, parse_copy_state,
    parse_window_rows,
};
pub use events::{TmuxConnectionClosed, TmuxConnectionReset};
pub use input::{KeyMods, bevy_key_to_tmux_name};
pub use keybindings::{
    CopyAction, Forwarded, KeyBindings, PromptKind, copy_mode_dispatch, plan_forward,
};
pub use output::PaneOutput;
pub use plugin::{TmuxEventBatch, TmuxPresence, TmuxProjectionSet, TmuxSessionPlugin};
pub use select::{AttachTarget, select_attach_target};
pub use state::ConnectionState;
pub use tmux_control::{ClientEvent, ControlEvent, TmuxCommand, TransportEvent};
pub use tmux_control_parser::{PaneId, SessionId, WindowId};
