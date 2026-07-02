//! Enumeration / introspection queries sent on attach and during projection:
//! list windows, client name, active pane, version, subscriptions, captures,
//! cursor, aggressive-resize.

use crate::enumerate::{LIST_WINDOWS_FORMAT, WINDOW_FLAGS_SUBSCRIPTION};
use tmux_control::TmuxCommand;
use tmux_control_parser::{PaneId, WindowId};

/// `list-windows -F "<fmt>"` — enumerates the session's windows on attach.
pub(crate) struct ListWindows;
impl TmuxCommand for ListWindows {
    fn into_raw_command(self) -> String {
        format!("list-windows -F \"{LIST_WINDOWS_FORMAT}\"")
    }
}

/// `display-message -p '#{client_name}'` — the control client's name.
pub(crate) struct ClientName;
impl TmuxCommand for ClientName {
    fn into_raw_command(self) -> String {
        "display-message -p '#{client_name}'".to_string()
    }
}

/// `display-message -p '#{window_id} #{pane_id}'` — the attached active window+pane.
pub(crate) struct ActivePane;
impl TmuxCommand for ActivePane {
    fn into_raw_command(self) -> String {
        "display-message -p '#{window_id} #{pane_id}'".to_string()
    }
}

/// `display-message -p '#{version}'` — the tmux server version.
pub(crate) struct Version;
impl TmuxCommand for Version {
    fn into_raw_command(self) -> String {
        "display-message -p '#{version}'".to_string()
    }
}

/// `refresh-client -B …:@*:#{window_raw_flags}` — subscribes to every window's flags.
pub(crate) struct SubscribeWindowFlags;
impl TmuxCommand for SubscribeWindowFlags {
    fn into_raw_command(self) -> String {
        format!("refresh-client -B {WINDOW_FLAGS_SUBSCRIPTION}:@*:#{{window_raw_flags}}")
    }
}

/// `capture-pane -p -e -t %<id>` — a pane's current visible content (with SGR).
pub(crate) struct CapturePane {
    pub id: PaneId,
}
impl TmuxCommand for CapturePane {
    fn into_raw_command(self) -> String {
        format!("capture-pane -p -e -t %{}", self.id.0)
    }
}

/// `display-message -p -t %<id> '#{cursor_x} #{cursor_y}'` — a pane's real cursor.
pub(crate) struct CursorQuery {
    pub id: PaneId,
}
impl TmuxCommand for CursorQuery {
    fn into_raw_command(self) -> String {
        format!(
            "display-message -p -t %{} '#{{cursor_x}} #{{cursor_y}}'",
            self.id.0
        )
    }
}

/// `show-options -wqv -t @<win> aggressive-resize` — the per-window option value.
pub(crate) struct AggressiveResize {
    pub win: WindowId,
}
impl TmuxCommand for AggressiveResize {
    fn into_raw_command(self) -> String {
        format!("show-options -wqv -t @{} aggressive-resize", self.win.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_windows_quotes_the_format() {
        let cmd = ListWindows.into_raw_command();
        assert!(cmd.starts_with("list-windows -F \""));
        assert!(cmd.ends_with('"'));
        assert!(cmd.contains(LIST_WINDOWS_FORMAT));
    }

    #[test]
    fn client_name_has_expected_format() {
        assert_eq!(
            ClientName.into_raw_command(),
            "display-message -p '#{client_name}'"
        );
    }

    #[test]
    fn active_pane_queries_window_and_pane() {
        assert_eq!(
            ActivePane.into_raw_command(),
            "display-message -p '#{window_id} #{pane_id}'"
        );
    }

    #[test]
    fn version_has_expected_format() {
        assert_eq!(
            Version.into_raw_command(),
            "display-message -p '#{version}'"
        );
    }

    #[test]
    fn subscribe_window_flags_uses_all_windows_raw_flags() {
        assert_eq!(
            SubscribeWindowFlags.into_raw_command(),
            format!("refresh-client -B {WINDOW_FLAGS_SUBSCRIPTION}:@*:#{{window_raw_flags}}")
        );
    }

    #[test]
    fn capture_pane_targets_at_id_with_escapes() {
        assert_eq!(
            CapturePane { id: PaneId(5) }.into_raw_command(),
            "capture-pane -p -e -t %5"
        );
    }

    #[test]
    fn cursor_query_targets_pane() {
        assert_eq!(
            CursorQuery { id: PaneId(4) }.into_raw_command(),
            "display-message -p -t %4 '#{cursor_x} #{cursor_y}'"
        );
    }

    #[test]
    fn aggressive_resize_targets_window() {
        assert_eq!(
            AggressiveResize { win: WindowId(1) }.into_raw_command(),
            "show-options -wqv -t @1 aggressive-resize"
        );
    }
}
