//! Typed tmux command structs. Each implements `tmux_control::TmuxCommand`,
//! rendering to the same raw control-mode string its former builder produced.

mod env;
mod size;
mod target;

pub use env::{SetEnvironment, SetEnvironmentInSession};
pub use size::{RefreshClient, ResizeWindow, WindowRefreshClient};
pub use target::{
    RenameSession, RenameWindow, ResizePaneX, ResizePaneY, SelectPane, SelectWindow, SwitchClient,
};
