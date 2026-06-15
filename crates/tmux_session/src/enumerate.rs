//! Parsing the `list-windows -F` reply used to enumerate windows on attach.

use crate::input::quote;
use crate::keybindings::PromptKind;
use bevy::prelude::Resource;
use std::collections::HashMap;
use tmux_control::CommandId;
use tmux_control_parser::{PaneId, WindowId, WindowLayout};

/// The `-F` format ozmux sends to enumerate windows. Tab-separated, with the
/// free-text `window_name` LAST so a `splitn(6, '\t')` keeps it intact.
pub const LIST_WINDOWS_FORMAT: &str = "#{window_active}\t#{window_id}\t#{window_index}\t#{window_layout}\t#{window_visible_layout}\t#{window_name}";

/// One parsed row of the `list-windows` reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowRow {
    /// tmux window id (`@N`).
    pub id: WindowId,
    /// Whether this is the session's active window.
    pub active: bool,
    /// tmux display index (#{window_index}), e.g. 0, 1, 2.
    pub index: u32,
    /// Window name.
    pub name: String,
    /// Parsed structural layout (panes + geometry).
    pub layout: WindowLayout,
}

/// Parses the lines of a `list-windows -F LIST_WINDOWS_FORMAT` reply.
///
/// Each line is `active \t id \t index \t layout \t visible_layout \t name`.
/// The `visible_layout` field is currently ignored. Blank lines are skipped.
/// Returns a descriptive `Err(String)` on a malformed row.
pub fn parse_window_rows(lines: &[String]) -> Result<Vec<WindowRow>, String> {
    let mut rows = Vec::new();
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        rows.push(parse_row(line)?);
    }
    Ok(rows)
}

fn parse_row(line: &str) -> Result<WindowRow, String> {
    let mut fields = line.splitn(6, '\t');
    let active = fields.next().is_some_and(|f| f == "1");
    let id = fields
        .next()
        .and_then(parse_window_id)
        .ok_or_else(|| format!("bad window id in row: {line}"))?;
    let index = fields
        .next()
        .and_then(|f| f.parse::<u32>().ok())
        .ok_or_else(|| format!("bad window index in row: {line}"))?;
    let layout_field = fields
        .next()
        .ok_or_else(|| format!("missing layout in row: {line}"))?;
    let layout = WindowLayout::parse(layout_field.as_bytes())
        .map_err(|e| format!("bad layout in row {line}: {e}"))?;
    let _visible = fields
        .next()
        .ok_or_else(|| format!("missing visible layout in row: {line}"))?;
    let name = fields
        .next()
        .ok_or_else(|| format!("missing name in row: {line}"))?
        .to_string();
    Ok(WindowRow {
        id,
        active,
        index,
        name,
        layout,
    })
}

fn parse_window_id(field: &str) -> Option<WindowId> {
    Some(WindowId(field.strip_prefix('@')?.parse().ok()?))
}

/// Builds a `refresh-client -C <cols>,<rows>` control-mode command telling
/// tmux this client's cell size. The bare `W,H` form is accepted by all tmux
/// versions; the `WxH` form is only required for the `@id:WxH` per-window
/// variant, which Phase 2b does not use.
pub fn refresh_client_command(cols: u16, rows: u16) -> String {
    format!("refresh-client -C {cols},{rows}")
}

/// Builds `display-message -p '#{client_name}'` — prints the control
/// client's name as a one-line command reply (correlated like `list-windows`).
pub(crate) fn client_name_command() -> String {
    "display-message -p '#{client_name}'".to_string()
}

/// Builds `display-message -p '#{window_id} #{pane_id}'` — prints the attached
/// client's active window and pane as one reply line (`@N %M`).
///
/// tmux does not emit `%window-pane-changed` on attach, so the active pane is
/// queried explicitly to seed the `ActivePane` marker (which drives pane dim).
pub(crate) fn active_pane_command() -> String {
    "display-message -p '#{window_id} #{pane_id}'".to_string()
}

/// Builds the `list-windows` command ozmux sends on attach to enumerate the
/// session's existing windows.
///
/// The `-F` format is double-quoted so its embedded tab field-separators
/// survive tmux's control-mode command tokenizer (which otherwise splits the
/// argument on whitespace).
pub(crate) fn list_windows_command() -> String {
    format!("list-windows -F \"{LIST_WINDOWS_FORMAT}\"")
}

/// Builds `set-environment <key> <value>` (session-scoped) to set an
/// environment variable on the control client's current session, so panes the
/// session spawns afterward inherit it. Session-scoped (no `-g`) to avoid
/// polluting the server-global environment of an attached, user-owned tmux
/// server. Used to propagate `$OZMUX_SOCK` to panes created after attach —
/// already-running panes cannot be updated.
pub fn set_environment_command(key: &str, value: &str) -> String {
    format!("set-environment {} {}", quote(key), quote(value))
}

/// Builds `select-window -t @<id>` to switch the client's active window.
pub fn select_window_command(id: WindowId) -> String {
    format!("select-window -t @{}", id.0)
}

/// Builds `select-pane -t %<id>` to focus a pane.
pub fn select_pane_command(id: PaneId) -> String {
    format!("select-pane -t %{}", id.0)
}

/// Builds `capture-pane -p -e -t %<id>` to fetch a pane's current visible
/// content (with SGR escapes) as a command reply.
///
/// tmux `-CC` does not replay existing pane content on attach — it only streams
/// new `%output`. So on attach each projected pane is captured once to seed its
/// initial screen; the reply bytes are fed into the pane like ordinary output.
pub(crate) fn capture_pane_command(id: PaneId) -> String {
    format!("capture-pane -p -e -t %{}", id.0)
}

/// The tab-separated `display-message -F` format ozmux reads each refresh while
/// a pane is in copy mode. Field order is fixed; `parse_copy_state` depends on it.
pub const COPY_STATE_FORMAT: &str = "#{pane_in_mode}\t#{scroll_position}\t#{pane_height}\t#{history_size}\t#{copy_cursor_x}\t#{copy_cursor_y}\t#{selection_present}\t#{rectangle_toggle}\t#{selection_start_x}\t#{selection_start_y}\t#{selection_end_x}\t#{selection_end_y}";

/// One snapshot of a pane's copy-mode state from `COPY_STATE_FORMAT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CopyState {
    /// Whether the pane is still in a mode (`#{pane_in_mode}` != 0).
    pub pane_in_mode: bool,
    /// Lines scrolled back from the live tail.
    pub scroll_position: u32,
    /// Visible pane height in rows.
    pub pane_height: u16,
    /// Total scrollback history line count.
    pub history_size: u32,
    /// Copy cursor column (visible).
    pub cursor_x: u16,
    /// Copy cursor row (visible, 0 = top of viewport).
    pub cursor_y: u16,
    /// Whether a selection exists.
    pub selection_present: bool,
    /// Whether the selection is a rectangle (block) selection.
    pub rectangle: bool,
    /// Selection start column (visible).
    pub sel_start_x: u16,
    /// Selection start row (ABSOLUTE grid line — map with `absolute_to_visible_row`).
    pub sel_start_y: u32,
    /// Selection end column (visible).
    pub sel_end_x: u16,
    /// Selection end row (ABSOLUTE grid line).
    pub sel_end_y: u32,
}

/// Parses one `COPY_STATE_FORMAT` reply line. Returns `None` if any field is
/// missing or unparseable (one malformed refresh is dropped, not fatal).
pub fn parse_copy_state(line: &str) -> Option<CopyState> {
    let mut f = line.split('\t');
    let pane_in_mode = f.next()?.trim().parse::<u32>().ok()? != 0;
    let scroll_position = f.next()?.trim().parse::<u32>().ok()?;
    let pane_height = f.next()?.trim().parse::<u32>().ok()? as u16;
    let history_size = f.next()?.trim().parse::<u32>().ok()?;
    let cursor_x = f.next()?.trim().parse::<u32>().ok()? as u16;
    let cursor_y = f.next()?.trim().parse::<u32>().ok()? as u16;
    let selection_present = f.next()?.trim().parse::<u32>().ok()? != 0;
    let rectangle = f.next()?.trim().parse::<u32>().ok()? != 0;
    let sel_start_x = f.next()?.trim().parse::<u32>().ok()? as u16;
    let sel_start_y = f.next()?.trim().parse::<u32>().ok()?;
    let sel_end_x = f.next()?.trim().parse::<u32>().ok()? as u16;
    let sel_end_y = f.next()?.trim().parse::<u32>().ok()?;
    Some(CopyState {
        pane_in_mode,
        scroll_position,
        pane_height,
        history_size,
        cursor_x,
        cursor_y,
        selection_present,
        rectangle,
        sel_start_x,
        sel_start_y,
        sel_end_x,
        sel_end_y,
    })
}

/// Returns the `capture-pane -S/-E` offsets for the scrolled copy-mode view:
/// `(-scroll_position, pane_height - 1 - scroll_position)`. Verified against
/// tmux 3.6a.
pub fn capture_offsets(scroll_position: u32, pane_height: u16) -> (i32, i32) {
    let start = -(scroll_position as i32);
    let end = pane_height as i32 - 1 - scroll_position as i32;
    (start, end)
}

/// Builds `capture-pane -p -e -t %N -S {start} -E {end}` for the scrolled view.
pub fn copy_mode_capture_command(pane: PaneId, scroll_position: u32, pane_height: u16) -> String {
    let (start, end) = capture_offsets(scroll_position, pane_height);
    format!("capture-pane -p -e -t %{} -S {start} -E {end}", pane.0)
}

/// Maps an absolute (history-relative) grid line to a visible viewport row:
/// `absolute_y - (history_size - scroll_position)`. Negative = above viewport.
pub fn absolute_to_visible_row(absolute_y: u32, history_size: u32, scroll_position: u32) -> i32 {
    let top = history_size as i32 - scroll_position as i32;
    absolute_y as i32 - top
}

/// Builds the per-refresh `display-message -p -t %N "<COPY_STATE_FORMAT>"`.
pub fn copy_state_query_command(pane: PaneId) -> String {
    format!("display-message -p -t %{} \"{COPY_STATE_FORMAT}\"", pane.0)
}

/// Builds `display-message -p '#{mode-keys}'` to read the active copy table.
pub fn mode_keys_command() -> String {
    "display-message -p '#{mode-keys}'".to_string()
}

/// Builds `send-keys -X -t %N <copy-command> -- '<text>'` for an ozmux prompt
/// submit (search regex or jump char). The text is tmux-quoted.
pub fn prompt_command(pane: PaneId, kind: PromptKind, text: &str) -> String {
    format!(
        "send-keys -X -t %{} {} -- {}",
        pane.0,
        kind.copy_command(),
        quote(text)
    )
}

/// Builds `show-buffer` to read tmux's top paste buffer for the clipboard bridge.
pub fn show_buffer_command() -> String {
    "show-buffer".to_string()
}

/// Tracks the in-flight `list-windows` enumeration command so its reply can
/// be correlated by [`CommandId`] and seeded into the projection.
#[derive(Resource, Default)]
pub(crate) struct EnumerationState {
    /// The id of the in-flight `list-windows` command, if any.
    pub(crate) pending: Option<CommandId>,
    /// The id of the in-flight `display-message` client-name query, if any.
    pub(crate) client_name_pending: Option<CommandId>,
    /// The id of the in-flight `display-message` active-pane query, if any.
    pub(crate) active_pane_pending: Option<CommandId>,
    /// The id of the in-flight `list-keys -T root` command, if any.
    pub(crate) keys_root_pending: Option<CommandId>,
    /// The id of the in-flight `list-keys -T prefix` command, if any.
    pub(crate) keys_prefix_pending: Option<CommandId>,
    /// The id of the in-flight `display-message` prefix-key query, if any.
    pub(crate) prefix_keys_pending: Option<CommandId>,
    /// In-flight `list-keys -T copy-mode` command, if any.
    pub(crate) keys_copy_mode_pending: Option<CommandId>,
    /// In-flight `list-keys -T copy-mode-vi` command, if any.
    pub(crate) keys_copy_mode_vi_pending: Option<CommandId>,
    /// In-flight `#{mode-keys}` query, if any.
    pub(crate) mode_keys_pending: Option<CommandId>,
    /// In-flight `capture-pane` commands → the pane each reply seeds.
    pub(crate) capture_pending: HashMap<CommandId, PaneId>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_is_tab_separated_with_name_last() {
        assert!(LIST_WINDOWS_FORMAT.contains('\t'));
        assert!(LIST_WINDOWS_FORMAT.ends_with("#{window_name}"));
    }

    #[test]
    fn parses_one_active_window() {
        let lines = vec!["1\t@1\t0\tb25f,80x24,0,0,0\tb25f,80x24,0,0,0\tmain".to_string()];
        let rows = parse_window_rows(&lines).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, WindowId(1));
        assert!(rows[0].active);
        assert_eq!(rows[0].name, "main");
        assert_eq!(rows[0].layout.root.dims().width, 80);
    }

    #[test]
    fn parses_multiple_windows_active_flag() {
        let lines = vec![
            "0\t@1\t0\tb25f,80x24,0,0,0\tb25f,80x24,0,0,0\tone".to_string(),
            "1\t@2\t1\tb25f,80x24,0,0,1\tb25f,80x24,0,0,1\ttwo".to_string(),
        ];
        let rows = parse_window_rows(&lines).unwrap();
        assert_eq!((rows[0].active, rows[1].active), (false, true));
        assert_eq!((rows[0].id, rows[1].id), (WindowId(1), WindowId(2)));
    }

    #[test]
    fn name_with_tabs_is_preserved_as_last_field() {
        let lines =
            vec!["1\t@1\t0\tb25f,80x24,0,0,0\tb25f,80x24,0,0,0\tmy\tnamed\twin".to_string()];
        let rows = parse_window_rows(&lines).unwrap();
        assert_eq!(rows[0].name, "my\tnamed\twin");
    }

    #[test]
    fn bad_window_id_errors() {
        let lines = vec!["1\t1\t0\tb25f,80x24,0,0,0\tb25f,80x24,0,0,0\tx".to_string()];
        assert!(parse_window_rows(&lines).is_err());
    }

    #[test]
    fn empty_input_is_empty() {
        assert_eq!(parse_window_rows(&[]).unwrap(), vec![]);
    }

    #[test]
    fn list_windows_command_quotes_the_format() {
        let cmd = list_windows_command();
        assert!(cmd.starts_with("list-windows -F \""));
        assert!(cmd.ends_with('"'));
        assert!(cmd.contains(LIST_WINDOWS_FORMAT));
    }

    #[test]
    fn refresh_client_command_uses_comma_size_form() {
        assert_eq!(refresh_client_command(80, 24), "refresh-client -C 80,24");
    }

    #[test]
    fn client_name_command_has_expected_format() {
        assert_eq!(client_name_command(), "display-message -p '#{client_name}'");
    }

    #[test]
    fn active_pane_command_queries_window_and_pane() {
        assert_eq!(
            active_pane_command(),
            "display-message -p '#{window_id} #{pane_id}'"
        );
    }

    #[test]
    fn parse_row_captures_window_index() {
        // Format order: active \t id \t index \t layout \t visible \t name
        let line = "1\t@2\t3\tb25d,80x24,0,0,0\tb25d,80x24,0,0,0\tmy-win";
        let rows = parse_window_rows(&[line.to_string()]).expect("parse");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].index, 3);
        assert_eq!(rows[0].name, "my-win");
        assert!(rows[0].active);
    }

    #[test]
    fn select_window_command_targets_at_id() {
        assert_eq!(select_window_command(WindowId(4)), "select-window -t @4");
    }

    #[test]
    fn select_pane_command_targets_at_id() {
        assert_eq!(select_pane_command(PaneId(3)), "select-pane -t %3");
    }

    #[test]
    fn capture_pane_command_targets_at_id_with_escapes() {
        assert_eq!(capture_pane_command(PaneId(5)), "capture-pane -p -e -t %5");
    }

    #[test]
    fn set_environment_command_is_session_scoped() {
        assert_eq!(
            set_environment_command("OZMUX_SOCK", "/tmp/ctl.sock"),
            "set-environment OZMUX_SOCK /tmp/ctl.sock"
        );
    }

    #[test]
    fn set_environment_command_quotes_paths_with_spaces() {
        assert_eq!(
            set_environment_command("OZMUX_SOCK", "/tmp/a b/ctl.sock"),
            "set-environment OZMUX_SOCK '/tmp/a b/ctl.sock'"
        );
    }

    #[test]
    fn capture_offsets_match_verified_formula() {
        assert_eq!(capture_offsets(12, 8), (-12, -5));
        assert_eq!(capture_offsets(0, 8), (0, 7));
    }

    #[test]
    fn copy_mode_capture_command_uses_scroll_offsets() {
        assert_eq!(
            copy_mode_capture_command(PaneId(3), 12, 8),
            "capture-pane -p -e -t %3 -S -12 -E -5"
        );
    }

    #[test]
    fn absolute_to_visible_row_matches_verified_mapping() {
        assert_eq!(absolute_to_visible_row(57, 53, 3), 7);
        assert_eq!(absolute_to_visible_row(54, 53, 3), 4);
        assert_eq!(absolute_to_visible_row(10, 53, 3), 10i32 - 50);
    }

    #[test]
    fn copy_state_format_is_tab_separated() {
        assert!(COPY_STATE_FORMAT.contains('\t'));
        assert!(COPY_STATE_FORMAT.starts_with("#{pane_in_mode}"));
    }

    #[test]
    fn parse_copy_state_reads_all_fields() {
        let line = "1\t3\t8\t53\t6\t7\t1\t0\t2\t54\t6\t57";
        let s = parse_copy_state(line).expect("parse");
        assert_eq!(s.pane_in_mode, true);
        assert_eq!(s.scroll_position, 3);
        assert_eq!(s.pane_height, 8);
        assert_eq!(s.history_size, 53);
        assert_eq!((s.cursor_x, s.cursor_y), (6, 7));
        assert_eq!(s.selection_present, true);
        assert_eq!(s.rectangle, false);
        assert_eq!((s.sel_start_x, s.sel_start_y), (2, 54));
        assert_eq!((s.sel_end_x, s.sel_end_y), (6, 57));
    }

    #[test]
    fn parse_copy_state_detects_exited_mode() {
        let s = parse_copy_state("0\t0\t8\t53\t0\t0\t0\t0\t0\t0\t0\t0").expect("parse");
        assert!(!s.pane_in_mode);
    }

    #[test]
    fn parse_copy_state_returns_none_on_short_or_garbage_line() {
        assert!(parse_copy_state("1\t3\t8").is_none());
        assert!(parse_copy_state("not-a-number\t0\t8\t53\t0\t0\t0\t0\t0\t0\t0\t0").is_none());
    }

    #[test]
    fn copy_state_query_command_targets_pane() {
        assert_eq!(
            copy_state_query_command(PaneId(4)),
            format!("display-message -p -t %4 \"{COPY_STATE_FORMAT}\"")
        );
    }

    #[test]
    fn prompt_search_command_quotes_text_and_targets_pane() {
        assert_eq!(
            prompt_command(PaneId(2), PromptKind::SearchForward, "foo bar"),
            "send-keys -X -t %2 search-forward -- 'foo bar'"
        );
    }

    #[test]
    fn mode_keys_query_reads_format() {
        assert_eq!(mode_keys_command(), "display-message -p '#{mode-keys}'");
    }
}
