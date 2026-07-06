//! Typed tmux command structs. Each implements `tmux_control::TmuxCommand`,
//! rendering to the same raw control-mode string its former builder produced.

mod env;
mod io;
mod ops;
mod query;
mod size;
mod target;
mod vi_mode;

pub use env::{SetEnvironmentGlobal, SetEnvironmentInSession, UnsetEnvironmentGlobal};
pub use io::{SendBytes, SendPaneKeys};
pub use ops::{
    KillPane, KillWindow, NewWindow, NextWindow, PaneDirection, PreviousWindow, ResizePaneTowards,
    SelectPaneTowards, SplitDirection, SplitWindow, SwitchClientNext, SwitchClientPrevious, ZoomPane,
};
pub(crate) use query::{
    ActivePane, AggressiveResize, CapturePane, CapturePanePending, CapturePaneSavedPrimary,
    CapturePaneWithHistory, ClientName, ListWindows, PaneStateQuery, SubscribeWindowFlags, Version,
};
pub use size::{RefreshClient, ResizeWindow, WindowRefreshClient};
pub use target::{RenameSession, RenameWindow, ResizePaneX, ResizePaneY, SelectPane, SelectWindow};
pub use vi_mode::{Prompt, PromptKind};
