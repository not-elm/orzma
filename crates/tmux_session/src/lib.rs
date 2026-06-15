//! ozmux ⇄ tmux control-mode integration: owns a `tmux -CC` connection,
//! drains its transport events into the Bevy world, tracks the connection
//! lifecycle, and projects tmux session/window/pane state as ECS entities
//! (`TmuxSession`/`TmuxWindow`/`TmuxPane`). The drain system translates each
//! transport batch into global projection events that observers apply directly
//! to the world. Rendering is not done here.

mod components;
mod connect;
mod connection;
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

pub use components::{ActivePane, ActiveWindow, TmuxPane, TmuxSession, TmuxWindow};
pub use connect::attach_or_create;
pub use connection::TmuxConnection;
pub use enumerate::{
    COPY_STATE_FORMAT, CopyState, LIST_WINDOWS_FORMAT, WindowRow, absolute_to_visible_row,
    capture_offsets, copy_mode_capture_command, copy_state_query_command, mode_keys_command,
    parse_copy_state, parse_window_rows, prompt_command, refresh_client_command,
    select_pane_command, select_window_command, set_environment_command, show_buffer_command,
};
pub use input::{KeyMods, bevy_key_to_tmux_name, send_bytes_command, send_pane_keys_command};
pub use keybindings::{
    CopyAction, Forwarded, KeyBindings, PromptKind, copy_mode_dispatch, plan_forward,
};
pub use output::PaneOutput;
pub use plugin::{TmuxPresence, TmuxProjectionSet, TmuxSessionPlugin};
pub use select::{AttachTarget, select_attach_target};
pub use state::ConnectionState;
pub use tmux_control_parser::{PaneId, WindowId};
