//! ECS components mirroring tmux session/window/pane identity + geometry.

use bevy::prelude::Component;
use bitflags::bitflags;
use tmux_control_parser::{CellDims, PaneId, SessionId, WindowId, WindowLayout};

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

bitflags! {
    /// tmux per-window status flags, projected from `#{window_raw_flags}`.
    #[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct WindowFlags: u8 {
        /// `Z` — the window's active pane is zoomed.
        const ZOOM = 1 << 0;
        /// `!` — a bell occurred in the window.
        const BELL = 1 << 1;
        /// `#` — monitored activity was detected.
        const ACTIVITY = 1 << 2;
        /// `~` — the window has been silent (monitor-silence).
        const SILENCE = 1 << 3;
        /// `M` — the window contains the marked pane.
        const MARKED = 1 << 4;
    }
}

impl WindowFlags {
    /// Parses a tmux `#{window_raw_flags}` string (e.g. `"*Z"`, `"!"`, `"#"`,
    /// `"~"`). Recognized characters set their bit; `*` (current), `-` (last),
    /// and any unknown character are ignored. An empty string yields an empty
    /// set.
    pub fn parse(raw: &str) -> Self {
        let mut flags = WindowFlags::empty();
        for ch in raw.chars() {
            match ch {
                'Z' => flags |= WindowFlags::ZOOM,
                '!' => flags |= WindowFlags::BELL,
                '#' => flags |= WindowFlags::ACTIVITY,
                '~' => flags |= WindowFlags::SILENCE,
                'M' => flags |= WindowFlags::MARKED,
                _ => {}
            }
        }
        flags
    }
}

/// The window's full tmux layout tree, retained for tree-driven pane sizing.
#[derive(Component, Debug, Clone)]
pub struct TmuxWindowLayout(pub WindowLayout);

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
            WindowFlags::ZOOM
                | WindowFlags::BELL
                | WindowFlags::ACTIVITY
                | WindowFlags::SILENCE
                | WindowFlags::MARKED
        );
    }

    #[test]
    fn parse_ignores_current_last_and_unknown() {
        assert_eq!(WindowFlags::parse("*-Z?"), WindowFlags::ZOOM);
    }
}
