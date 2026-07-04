//! Typed tmux command structs. Each implements `tmux_control::TmuxCommand`,
//! rendering to the same raw control-mode string its former builder produced.

mod copymode;
mod env;
mod io;
mod ops;
mod query;
mod size;
mod target;

pub use copymode::{CopyModeCapture, CopyStateQuery, Prompt, PromptKind, ShowBuffer};
pub use env::{SetEnvironmentGlobal, SetEnvironmentInSession, UnsetEnvironmentGlobal};
pub use io::{SendBytes, SendPaneKeys};
pub use ops::{
    EnterCopyMode, KillPane, KillWindow, NewWindow, NextWindow, PaneDirection, PreviousWindow,
    SelectPaneTowards, SplitDirection, SplitWindow, ZoomPane,
};
pub(crate) use query::{
    ActivePane, AggressiveResize, CapturePane, CapturePanePending, CapturePaneSavedPrimary,
    CapturePaneWithHistory, ClientName, ListWindows, PaneStateQuery, SubscribeWindowFlags, Version,
};
pub use size::{RefreshClient, ResizeWindow, WindowRefreshClient};
pub use target::{RenameSession, RenameWindow, ResizePaneX, ResizePaneY, SelectPane, SelectWindow};
