//! Pane / window operation commands driven by ozmux-native shortcuts:
//! split, kill, zoom, copy-mode entry, directional pane selection, and
//! window cycling.

use tmux_control::TmuxCommand;
use tmux_control_parser::{PaneId, SessionId, WindowId};

/// Which way `split-window` divides a pane, in tmux flag semantics.
///
/// NOTE: tmux names the layout axis, not the divider: `-h` (Horizontal) puts
/// panes side by side. The config-facing `SplitOrientation` names the divider
/// instead; the binary maps between the two.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitDirection {
    /// `-h`: panes end up side by side.
    Horizontal,
    /// `-v`: panes end up stacked.
    Vertical,
}

/// A neighbor direction for `select-pane`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneDirection {
    /// `-L`.
    Left,
    /// `-D`.
    Down,
    /// `-U`.
    Up,
    /// `-R`.
    Right,
}

/// `split-window -h|-v -t %<id>` — splits the target pane.
pub struct SplitWindow {
    /// Target pane id.
    pub pane: PaneId,
    /// Split direction (tmux flag semantics).
    pub direction: SplitDirection,
}
impl TmuxCommand for SplitWindow {
    fn into_raw_command(self) -> String {
        let flag = match self.direction {
            SplitDirection::Horizontal => "-h",
            SplitDirection::Vertical => "-v",
        };
        format!("split-window {flag} -t %{}", self.pane.0)
    }
}

/// `select-pane -L|-D|-U|-R -t %<id>` — focuses the target pane's neighbor.
pub struct SelectPaneTowards {
    /// The pane whose neighbor is selected.
    pub pane: PaneId,
    /// Which neighbor to select.
    pub direction: PaneDirection,
}
impl TmuxCommand for SelectPaneTowards {
    fn into_raw_command(self) -> String {
        let flag = match self.direction {
            PaneDirection::Left => "-L",
            PaneDirection::Down => "-D",
            PaneDirection::Up => "-U",
            PaneDirection::Right => "-R",
        };
        format!("select-pane {flag} -t %{}", self.pane.0)
    }
}

/// `kill-pane -t %<id>`.
pub struct KillPane {
    /// Target pane id.
    pub pane: PaneId,
}
impl TmuxCommand for KillPane {
    fn into_raw_command(self) -> String {
        format!("kill-pane -t %{}", self.pane.0)
    }
}

/// `kill-window -t @<id>`.
pub struct KillWindow {
    /// Target window id.
    pub window: WindowId,
}
impl TmuxCommand for KillWindow {
    fn into_raw_command(self) -> String {
        format!("kill-window -t @{}", self.window.0)
    }
}

/// `new-window` — opens a window in the client's current session. Bare on
/// purpose: `-t` carries placement semantics, so the current-session default
/// is what a shortcut wants.
pub struct NewWindow;
impl TmuxCommand for NewWindow {
    fn into_raw_command(self) -> String {
        "new-window".to_string()
    }
}

/// `next-window -t $<id>`.
pub struct NextWindow {
    /// Target session id.
    pub session: SessionId,
}
impl TmuxCommand for NextWindow {
    fn into_raw_command(self) -> String {
        format!("next-window -t ${}", self.session.0)
    }
}

/// `previous-window -t $<id>`.
pub struct PreviousWindow {
    /// Target session id.
    pub session: SessionId,
}
impl TmuxCommand for PreviousWindow {
    fn into_raw_command(self) -> String {
        format!("previous-window -t ${}", self.session.0)
    }
}

/// `resize-pane -Z -t %<id>` — toggles zoom on the target pane.
pub struct ZoomPane {
    /// Target pane id.
    pub pane: PaneId,
}
impl TmuxCommand for ZoomPane {
    fn into_raw_command(self) -> String {
        format!("resize-pane -Z -t %{}", self.pane.0)
    }
}

/// `copy-mode -t %<id>` — puts the target pane into tmux copy mode.
pub struct EnterCopyMode {
    /// Target pane id.
    pub pane: PaneId,
}
impl TmuxCommand for EnterCopyMode {
    fn into_raw_command(self) -> String {
        format!("copy-mode -t %{}", self.pane.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_window_renders_both_directions() {
        assert_eq!(
            SplitWindow {
                pane: PaneId(3),
                direction: SplitDirection::Horizontal
            }
            .into_raw_command(),
            "split-window -h -t %3"
        );
        assert_eq!(
            SplitWindow {
                pane: PaneId(3),
                direction: SplitDirection::Vertical
            }
            .into_raw_command(),
            "split-window -v -t %3"
        );
    }

    #[test]
    fn select_pane_towards_renders_all_directions() {
        for (direction, flag) in [
            (PaneDirection::Left, "-L"),
            (PaneDirection::Down, "-D"),
            (PaneDirection::Up, "-U"),
            (PaneDirection::Right, "-R"),
        ] {
            assert_eq!(
                SelectPaneTowards {
                    pane: PaneId(7),
                    direction
                }
                .into_raw_command(),
                format!("select-pane {flag} -t %7")
            );
        }
    }

    #[test]
    fn kill_commands_target_ids() {
        assert_eq!(
            KillPane { pane: PaneId(5) }.into_raw_command(),
            "kill-pane -t %5"
        );
        assert_eq!(
            KillWindow { window: WindowId(2) }.into_raw_command(),
            "kill-window -t @2"
        );
    }

    #[test]
    fn window_cycle_commands_target_session() {
        assert_eq!(NewWindow.into_raw_command(), "new-window");
        assert_eq!(
            NextWindow { session: SessionId(1) }.into_raw_command(),
            "next-window -t $1"
        );
        assert_eq!(
            PreviousWindow { session: SessionId(1) }.into_raw_command(),
            "previous-window -t $1"
        );
    }

    #[test]
    fn zoom_and_copy_mode_target_pane() {
        assert_eq!(
            ZoomPane { pane: PaneId(4) }.into_raw_command(),
            "resize-pane -Z -t %4"
        );
        assert_eq!(
            EnterCopyMode { pane: PaneId(4) }.into_raw_command(),
            "copy-mode -t %4"
        );
    }
}
