//! Pane / window operation commands driven by orzma-native shortcuts:
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

/// `split-window -h|-v -t %<id> -c "#{pane_current_path}"` — splits the target
/// pane and starts the new pane in the target pane's current directory.
///
/// Without `-c`, tmux 1.9+ starts the new pane in the client/session start
/// directory (where the control client attached), not the source pane's cwd, so
/// a pane that has `cd`'d away would spawn its split in the wrong directory.
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
        // NOTE: the `#{pane_current_path}` format MUST stay double-quoted. tmux's
        // command parser treats a bare leading `#` as a comment, so an unquoted
        // `-c #{pane_current_path}` drops the argument and fails with
        // "-c expects an argument". The format expands against the session's
        // active pane, which orzma always splits (the `ActivePane` target).
        format!(
            "split-window {flag} -t %{} -c \"#{{pane_current_path}}\"",
            self.pane.0
        )
    }
}

impl PaneDirection {
    /// The tmux directional flag (`-L`/`-D`/`-U`/`-R`) for this direction.
    fn tmux_flag(self) -> &'static str {
        match self {
            PaneDirection::Left => "-L",
            PaneDirection::Down => "-D",
            PaneDirection::Up => "-U",
            PaneDirection::Right => "-R",
        }
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
        format!(
            "select-pane {} -t %{}",
            self.direction.tmux_flag(),
            self.pane.0
        )
    }
}

/// `resize-pane -L|-R|-U|-D -t %<id> <amount>` — moves the target pane's border
/// by `amount` cells in the given direction.
pub struct ResizePaneTowards {
    /// The pane to resize.
    pub pane: PaneId,
    /// Which border to move.
    pub direction: PaneDirection,
    /// Adjustment in cells.
    pub amount: u32,
}
impl TmuxCommand for ResizePaneTowards {
    fn into_raw_command(self) -> String {
        // NOTE: the <amount> adjustment is a trailing positional operand and MUST
        // follow every option, including -t. tmux stops option parsing at the
        // first non-option token, so an amount placed before -t makes -t surplus
        // and tmux rejects the whole command ("too many arguments").
        format!(
            "resize-pane {} -t %{} {}",
            self.direction.tmux_flag(),
            self.pane.0,
            self.amount
        )
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

/// `new-window -c "#{pane_current_path}"` — opens a window in the client's
/// current session, starting in the active pane's current directory.
///
/// No `-t`: it carries a placement position, not a session, so the
/// current-session default is what a shortcut wants. `-c` is required because,
/// without it, tmux 1.9+ starts the window in the session start directory, not
/// the active pane's cwd.
pub struct NewWindow;
impl TmuxCommand for NewWindow {
    fn into_raw_command(self) -> String {
        // NOTE: `#{pane_current_path}` MUST stay double-quoted — tmux's command
        // parser treats a bare leading `#` as a comment, so an unquoted form
        // fails with "-c expects an argument". The format expands against the
        // current session's active pane.
        "new-window -c \"#{pane_current_path}\"".to_string()
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
            "split-window -h -t %3 -c \"#{pane_current_path}\""
        );
        assert_eq!(
            SplitWindow {
                pane: PaneId(3),
                direction: SplitDirection::Vertical
            }
            .into_raw_command(),
            "split-window -v -t %3 -c \"#{pane_current_path}\""
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
            KillWindow {
                window: WindowId(2)
            }
            .into_raw_command(),
            "kill-window -t @2"
        );
    }

    #[test]
    fn window_cycle_commands_target_session() {
        assert_eq!(
            NewWindow.into_raw_command(),
            "new-window -c \"#{pane_current_path}\""
        );
        assert_eq!(
            NextWindow {
                session: SessionId(1)
            }
            .into_raw_command(),
            "next-window -t $1"
        );
        assert_eq!(
            PreviousWindow {
                session: SessionId(1)
            }
            .into_raw_command(),
            "previous-window -t $1"
        );
    }

    #[test]
    fn zoom_pane_targets_pane() {
        assert_eq!(
            ZoomPane { pane: PaneId(4) }.into_raw_command(),
            "resize-pane -Z -t %4"
        );
    }

    #[test]
    fn resize_pane_towards_renders_amount_as_trailing_operand() {
        assert_eq!(
            ResizePaneTowards {
                pane: PaneId(9),
                direction: PaneDirection::Left,
                amount: 5
            }
            .into_raw_command(),
            "resize-pane -L -t %9 5"
        );
        assert_eq!(
            ResizePaneTowards {
                pane: PaneId(3),
                direction: PaneDirection::Down,
                amount: 5
            }
            .into_raw_command(),
            "resize-pane -D -t %3 5"
        );
        assert_eq!(
            ResizePaneTowards {
                pane: PaneId(3),
                direction: PaneDirection::Up,
                amount: 5
            }
            .into_raw_command(),
            "resize-pane -U -t %3 5"
        );
        assert_eq!(
            ResizePaneTowards {
                pane: PaneId(3),
                direction: PaneDirection::Right,
                amount: 5
            }
            .into_raw_command(),
            "resize-pane -R -t %3 5"
        );
    }
}
