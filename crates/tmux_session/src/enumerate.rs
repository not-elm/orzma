//! Parsing the `list-windows -F` reply used to enumerate windows on attach.

use bevy::prelude::Resource;
use tmux_control::CommandId;
use tmux_control_parser::{WindowId, WindowLayout};

/// The `-F` format ozmux sends to enumerate windows. Tab-separated, with the
/// free-text `window_name` LAST so a `splitn(5, '\t')` keeps it intact.
pub const LIST_WINDOWS_FORMAT: &str =
    "#{window_active}\t#{window_id}\t#{window_layout}\t#{window_visible_layout}\t#{window_name}";

/// One parsed row of the `list-windows` reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowRow {
    /// tmux window id (`@N`).
    pub id: WindowId,
    /// Whether this is the session's active window.
    pub active: bool,
    /// Window name.
    pub name: String,
    /// Parsed structural layout (panes + geometry).
    pub layout: WindowLayout,
}

/// Parses the lines of a `list-windows -F LIST_WINDOWS_FORMAT` reply.
///
/// Each line is `active \t window_id \t layout \t visible_layout \t name`.
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
    let mut fields = line.splitn(5, '\t');
    let active = fields.next().is_some_and(|f| f == "1");
    let id = fields
        .next()
        .and_then(parse_window_id)
        .ok_or_else(|| format!("bad window id in row: {line}"))?;
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
pub fn client_name_command() -> String {
    "display-message -p '#{client_name}'".to_string()
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

/// Tracks the in-flight `list-windows` enumeration command so its reply can
/// be correlated by [`CommandId`] and seeded into the projection.
#[derive(Resource, Default)]
pub(crate) struct EnumerationState {
    /// The id of the in-flight `list-windows` command, if any.
    pub(crate) pending: Option<CommandId>,
    /// The id of the in-flight `display-message` client-name query, if any.
    pub(crate) client_name_pending: Option<CommandId>,
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
        let lines = vec!["1\t@1\tb25f,80x24,0,0,0\tb25f,80x24,0,0,0\tmain".to_string()];
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
            "0\t@1\tb25f,80x24,0,0,0\tb25f,80x24,0,0,0\tone".to_string(),
            "1\t@2\tb25f,80x24,0,0,1\tb25f,80x24,0,0,1\ttwo".to_string(),
        ];
        let rows = parse_window_rows(&lines).unwrap();
        assert_eq!((rows[0].active, rows[1].active), (false, true));
        assert_eq!((rows[0].id, rows[1].id), (WindowId(1), WindowId(2)));
    }

    #[test]
    fn name_with_tabs_is_preserved_as_last_field() {
        let lines = vec!["1\t@1\tb25f,80x24,0,0,0\tb25f,80x24,0,0,0\tmy\tnamed\twin".to_string()];
        let rows = parse_window_rows(&lines).unwrap();
        assert_eq!(rows[0].name, "my\tnamed\twin");
    }

    #[test]
    fn bad_window_id_errors() {
        let lines = vec!["1\t1\tb25f,80x24,0,0,0\tb25f,80x24,0,0,0\tx".to_string()];
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
}
