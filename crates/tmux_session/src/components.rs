//! ECS components mirroring tmux session/window/pane identity + geometry.

use bevy::prelude::Component;
use tmux_control_parser::{CellDims, PaneId, SessionId, WindowId};

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

/// tmux per-window status flags, projected from `#{window_raw_flags}`.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WindowFlags {
    /// `Z` — the window's active pane is zoomed.
    pub zoom: bool,
    /// `!` — a bell occurred in the window.
    pub bell: bool,
    /// `#` — monitored activity was detected.
    pub activity: bool,
    /// `~` — the window has been silent (monitor-silence).
    pub silence: bool,
    /// `M` — the window contains the marked pane.
    pub marked: bool,
}

impl WindowFlags {
    /// Parses a tmux `#{window_raw_flags}` string (e.g. `"*Z"`, `"!"`, `"#"`,
    /// `"~"`). Recognized characters set their field; `*` (current), `-`
    /// (last), and any unknown character are ignored. An empty string yields
    /// all-false.
    pub fn parse(raw: &str) -> Self {
        let mut flags = WindowFlags::default();
        for ch in raw.chars() {
            match ch {
                'Z' => flags.zoom = true,
                '!' => flags.bell = true,
                '#' => flags.activity = true,
                '~' => flags.silence = true,
                'M' => flags.marked = true,
                _ => {}
            }
        }
        flags
    }
}

/// A projected tmux pane entity.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct TmuxPane {
    /// tmux pane id (`%N`).
    pub id: PaneId,
    /// Cell geometry from the window layout.
    pub dims: CellDims,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_empty_is_all_false() {
        assert_eq!(WindowFlags::parse(""), WindowFlags::default());
    }

    #[test]
    fn parse_recognizes_each_flag() {
        assert_eq!(
            WindowFlags::parse("Z!#~M"),
            WindowFlags {
                zoom: true,
                bell: true,
                activity: true,
                silence: true,
                marked: true,
            }
        );
    }

    #[test]
    fn parse_ignores_current_last_and_unknown() {
        assert_eq!(
            WindowFlags::parse("*-Z?"),
            WindowFlags {
                zoom: true,
                ..WindowFlags::default()
            }
        );
    }
}
