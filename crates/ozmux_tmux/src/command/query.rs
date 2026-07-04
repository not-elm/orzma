//! Enumeration / introspection queries sent on attach and during projection:
//! list windows, client name, active pane, version, subscriptions, captures,
//! cursor, aggressive-resize.

use crate::enumerate::{LIST_WINDOWS_FORMAT, WINDOW_FLAGS_SUBSCRIPTION};
use crate::state_restore::PANE_STATE_FORMAT;
use ozma_tty_engine::TerminalHandle;
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

/// `capture-pane -peqJ -t %<id>` — the pane's visible screen (with SGR,
/// wrapped lines joined, trailing spaces preserved by `-J`).
pub(crate) struct CapturePane {
    pub id: PaneId,
}
impl TmuxCommand for CapturePane {
    fn into_raw_command(self) -> String {
        format!("capture-pane -peqJ -t %{}", self.id.0)
    }
}

/// `capture-pane -peqJ -S -<cap>` — the pane's base grid: primary history
/// plus the CURRENT visible screen (the alt screen while alternate is on).
pub(crate) struct CapturePaneWithHistory {
    pub id: PaneId,
}
impl TmuxCommand for CapturePaneWithHistory {
    fn into_raw_command(self) -> String {
        format!(
            "capture-pane -peqJ -t %{} -S -{}",
            self.id.0,
            TerminalHandle::default_scroll_cap()
        )
    }
}

/// `capture-pane -peqJa` — the saved primary screen tmux snapshotted on alt
/// entry (`saved_grid`); empty via `-q` when the pane never entered alt.
pub(crate) struct CapturePaneSavedPrimary {
    pub id: PaneId,
}
impl TmuxCommand for CapturePaneSavedPrimary {
    fn into_raw_command(self) -> String {
        format!("capture-pane -peqJa -t %{}", self.id.0)
    }
}

/// `display-message -p` over [`PANE_STATE_FORMAT`] (positional message, as
/// `CopyStateQuery` does — display-message has no `-F` flag) — one pane's
/// terminal modes, cursor, scroll region, and tab stops.
pub(crate) struct PaneStateQuery {
    pub id: PaneId,
}
impl TmuxCommand for PaneStateQuery {
    fn into_raw_command(self) -> String {
        format!(
            "display-message -p -t %{} \"{PANE_STATE_FORMAT}\"",
            self.id.0
        )
    }
}

/// `capture-pane -pPC` — pending (unflushed) pane output with control
/// characters octal-escaped.
pub(crate) struct CapturePanePending {
    pub id: PaneId,
}
impl TmuxCommand for CapturePanePending {
    fn into_raw_command(self) -> String {
        format!("capture-pane -pPC -t %{}", self.id.0)
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
    fn capture_pane_visible_uses_escape_flags() {
        assert_eq!(
            CapturePane { id: PaneId(5) }.into_raw_command(),
            "capture-pane -peqJ -t %5"
        );
    }

    #[test]
    fn capture_with_history_reaches_mirror_scroll_cap() {
        let cmd = CapturePaneWithHistory { id: PaneId(5) }.into_raw_command();
        assert_eq!(
            cmd,
            format!(
                "capture-pane -peqJ -t %5 -S -{}",
                TerminalHandle::default_scroll_cap()
            )
        );
    }

    #[test]
    fn capture_saved_primary_uses_alternate_flag() {
        assert_eq!(
            CapturePaneSavedPrimary { id: PaneId(7) }.into_raw_command(),
            "capture-pane -peqJa -t %7"
        );
    }

    #[test]
    fn pane_state_query_embeds_the_state_format() {
        let cmd = PaneStateQuery { id: PaneId(3) }.into_raw_command();
        assert!(cmd.starts_with("display-message -p -t %3 \""));
        assert!(cmd.contains("alternate_on=#{alternate_on}"));
        assert!(cmd.ends_with('"'));
    }

    #[test]
    fn capture_pending_uses_pc_flags() {
        assert_eq!(
            CapturePanePending { id: PaneId(2) }.into_raw_command(),
            "capture-pane -pPC -t %2"
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
