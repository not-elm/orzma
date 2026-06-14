//! ECS components mirroring tmux session/window/pane identity + geometry.

use bevy::prelude::Component;
use tmux_control_parser::{CellDims, PaneId, SessionId, WindowId};

/// The projected tmux session entity.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct TmuxSession {
    /// tmux session id (`$N`).
    pub id: SessionId,
}

/// A projected tmux window entity.
#[derive(Component, Debug, Clone, PartialEq, Eq)]
pub struct TmuxWindow {
    /// tmux window id (`@N`).
    pub id: WindowId,
    /// Whether this is the session's active window.
    pub active: bool,
    /// Window name.
    pub name: String,
}

/// A projected tmux pane entity.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct TmuxPane {
    /// tmux pane id (`%N`).
    pub id: PaneId,
    /// Cell geometry from the window layout.
    pub dims: CellDims,
}
