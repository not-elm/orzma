//! Targeting / rename / resize-pane commands: select a window or pane, switch
//! client, rename a window or session, resize a pane along one axis.

use crate::enumerate::rename_command;
use crate::input::quote;
use tmux_control::TmuxCommand;
use tmux_control_parser::{PaneId, SessionId, WindowId};

/// `select-window -t @<id>` — switches the client's active window.
pub struct SelectWindow {
    pub id: WindowId,
}
impl TmuxCommand for SelectWindow {
    fn into_raw_command(self) -> String {
        format!("select-window -t @{}", self.id.0)
    }
}

/// `select-pane -t %<id>` — focuses a pane.
pub struct SelectPane {
    pub id: PaneId,
}
impl TmuxCommand for SelectPane {
    fn into_raw_command(self) -> String {
        format!("select-pane -t %{}", self.id.0)
    }
}

/// `switch-client -t <name>` — repoints the attached control client at another session.
pub struct SwitchClient<'a> {
    pub name: &'a str,
}
impl TmuxCommand for SwitchClient<'_> {
    fn into_raw_command(self) -> String {
        format!("switch-client -t {}", quote(self.name))
    }
}

/// `rename-window -t @<id> -- <name>` (name tmux-quoted; `--` guards a leading `-`).
pub struct RenameWindow<'a> {
    pub id: WindowId,
    pub name: &'a str,
}
impl TmuxCommand for RenameWindow<'_> {
    fn into_raw_command(self) -> String {
        rename_command("rename-window", '@', self.id.0, self.name)
    }
}

/// `rename-session -t $<id> -- <name>` (name tmux-quoted).
pub struct RenameSession<'a> {
    pub id: SessionId,
    pub name: &'a str,
}
impl TmuxCommand for RenameSession<'_> {
    fn into_raw_command(self) -> String {
        rename_command("rename-session", '$', self.id.0, self.name)
    }
}

/// `resize-pane -t %<id> -x <width>` (absolute, idempotent).
pub struct ResizePaneX {
    pub id: PaneId,
    pub width: u32,
}
impl TmuxCommand for ResizePaneX {
    fn into_raw_command(self) -> String {
        format!("resize-pane -t %{} -x {}", self.id.0, self.width)
    }
}

/// `resize-pane -t %<id> -y <height>` (absolute, idempotent).
pub struct ResizePaneY {
    pub id: PaneId,
    pub height: u32,
}
impl TmuxCommand for ResizePaneY {
    fn into_raw_command(self) -> String {
        format!("resize-pane -t %{} -y {}", self.id.0, self.height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_window_targets_at_id() {
        assert_eq!(
            SelectWindow { id: WindowId(4) }.into_raw_command(),
            "select-window -t @4"
        );
    }

    #[test]
    fn select_pane_targets_at_id() {
        assert_eq!(
            SelectPane { id: PaneId(3) }.into_raw_command(),
            "select-pane -t %3"
        );
    }

    #[test]
    fn switch_client_quotes_name() {
        assert_eq!(
            SwitchClient { name: "main" }.into_raw_command(),
            "switch-client -t main"
        );
        assert_eq!(
            SwitchClient { name: "my work" }.into_raw_command(),
            "switch-client -t 'my work'"
        );
    }

    #[test]
    fn rename_window_targets_at_id_and_quotes_name() {
        assert_eq!(
            RenameWindow {
                id: WindowId(2),
                name: "editor"
            }
            .into_raw_command(),
            "rename-window -t @2 -- editor"
        );
        assert_eq!(
            RenameWindow {
                id: WindowId(2),
                name: "my editor"
            }
            .into_raw_command(),
            "rename-window -t @2 -- 'my editor'"
        );
        assert_eq!(
            RenameWindow {
                id: WindowId(2),
                name: ""
            }
            .into_raw_command(),
            "rename-window -t @2 -- ''"
        );
        assert_eq!(
            RenameWindow {
                id: WindowId(7),
                name: "it's"
            }
            .into_raw_command(),
            r"rename-window -t @7 -- 'it'\''s'"
        );
    }

    #[test]
    fn rename_session_targets_dollar_id_and_quotes_name() {
        assert_eq!(
            RenameSession {
                id: SessionId(0),
                name: "work"
            }
            .into_raw_command(),
            "rename-session -t $0 -- work"
        );
        assert_eq!(
            RenameSession {
                id: SessionId(3),
                name: "my work"
            }
            .into_raw_command(),
            "rename-session -t $3 -- 'my work'"
        );
        assert_eq!(
            RenameSession {
                id: SessionId(3),
                name: ""
            }
            .into_raw_command(),
            "rename-session -t $3 -- ''"
        );
    }

    #[test]
    fn resize_pane_axes_are_absolute() {
        assert_eq!(
            ResizePaneX {
                id: PaneId(3),
                width: 80
            }
            .into_raw_command(),
            "resize-pane -t %3 -x 80"
        );
        assert_eq!(
            ResizePaneY {
                id: PaneId(3),
                height: 24
            }
            .into_raw_command(),
            "resize-pane -t %3 -y 24"
        );
    }
}
