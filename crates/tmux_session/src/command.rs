//! Typed tmux command structs. Each implements `tmux_control::TmuxCommand`,
//! rendering to the same raw control-mode string its former builder produced.

mod target;

pub use target::{
    RenameSession, RenameWindow, ResizePaneX, ResizePaneY, SelectPane, SelectWindow, SwitchClient,
};
