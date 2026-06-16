//! ECS components mirroring tmux session/window/pane identity + geometry.

use bevy::prelude::Component;
use tmux_control_parser::{CellDims, Divider, PaneId, SessionId, WindowId};

/// The projected tmux session entity, carrying the session id and name.
#[derive(Component, Debug, Clone, PartialEq, Eq)]
pub struct TmuxSession {
    /// tmux session id (`$N`).
    pub id: SessionId,
    /// Session name, from `%session-changed`. Empty until first known.
    pub name: String,
}

/// Marker on the single active pane entity (`%window-pane-changed`).
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ActivePane;

/// Marker on the single active window entity (`%window-pane-changed`).
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ActiveWindow;

/// A projected tmux window entity.
#[derive(Component, Debug, Clone, PartialEq, Eq)]
pub struct TmuxWindow {
    /// tmux window id (`@N`).
    pub id: WindowId,
    /// tmux display index (#{window_index}).
    pub index: u32,
    /// Window name.
    pub name: String,
}

/// The active window's draggable dividers, projected for the mouse arbiter.
#[derive(Component, Debug, Clone, Default, PartialEq, Eq)]
pub struct TmuxDividers(pub Vec<Divider>);

/// A projected tmux pane entity.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct TmuxPane {
    /// tmux pane id (`%N`).
    pub id: PaneId,
    /// Cell geometry from the window layout.
    pub dims: CellDims,
}
