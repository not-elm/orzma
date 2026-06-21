//! Parsing the `list-windows -F` reply used to enumerate windows on attach.

use crate::components::WindowFlags;
use crate::input::quote;
use bevy::prelude::Resource;
use std::collections::{HashMap, HashSet};
use tmux_control::{CommandId, TmuxResult};
use tmux_control_parser::{PaneId, WindowId, WindowLayout};

/// The `-F` format ozmux sends to enumerate windows. Tab-separated, with the
/// free-text `window_name` LAST so a `splitn(7, '\t')` keeps it intact.
pub const LIST_WINDOWS_FORMAT: &str = "#{window_active}\t#{window_id}\t#{window_index}\t#{window_layout}\t#{window_visible_layout}\t#{window_raw_flags}\t#{window_name}";

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
    /// tmux per-window flags (`#{window_raw_flags}`).
    pub flags: WindowFlags,
    /// Parsed structural layout (panes + geometry). Sourced from
    /// `window_visible_layout` when non-empty; falls back to `window_layout`.
    pub layout: WindowLayout,
}

/// Parses the lines of a `list-windows -F LIST_WINDOWS_FORMAT` reply.
///
/// Each line is `active \t id \t index \t layout \t visible_layout \t raw_flags \t name`.
/// When `visible_layout` is non-empty it is used for `WindowRow.layout`; otherwise
/// `layout` is the fallback. Blank lines are skipped.
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
    let mut fields = line.splitn(7, '\t');
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
    let visible_field = fields
        .next()
        .ok_or_else(|| format!("missing visible layout in row: {line}"))?;
    let chosen = if visible_field.trim().is_empty() {
        layout_field
    } else {
        visible_field
    };
    let layout = WindowLayout::parse(chosen.as_bytes())
        .map_err(|e| format!("bad layout in row {line}: {e}"))?;
    let flags = WindowFlags::parse(
        fields
            .next()
            .ok_or_else(|| format!("missing flags in row: {line}"))?,
    );
    let name = fields
        .next()
        .ok_or_else(|| format!("missing name in row: {line}"))?
        .to_string();
    Ok(WindowRow {
        id,
        active,
        index,
        name,
        flags,
        layout,
    })
}

fn parse_window_id(field: &str) -> Option<WindowId> {
    Some(WindowId(field.strip_prefix('@')?.parse().ok()?))
}

/// Returns whether `version` supports per-window `refresh-client -C @win:WxH`
/// (tmux ≥ 3.4). Parses leniently: the leading `major.minor`, tolerating a
/// `next-` prefix and a trailing letter suffix like `3.6a`.
pub(crate) fn version_supports_per_window_refresh(version: &str) -> bool {
    parse_major_minor(version).is_some_and(|mm| mm >= (3, 4))
}

fn parse_major_minor(version: &str) -> Option<(u32, u32)> {
    let trimmed = version
        .trim()
        .trim_start_matches(|c: char| !c.is_ascii_digit());
    let mut parts = trimmed.split('.');
    let major: u32 = parts.next()?.parse().ok()?;
    let minor_digits: String = parts
        .next()?
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    let minor: u32 = minor_digits.parse().ok()?;
    Some((major, minor))
}

/// The name of the control-mode subscription that streams every window's
/// `#{window_raw_flags}` back as `%subscription-changed`.
pub(crate) const WINDOW_FLAGS_SUBSCRIPTION: &str = "ozmux-window-flags";

pub(crate) fn rename_command(verb: &str, sigil: char, id: u32, name: &str) -> String {
    format!("{verb} -t {sigil}{id} -- {}", quote(name))
}

/// The tab-separated `display-message -F` format ozmux reads each refresh while
/// a pane is in copy mode. Field order is fixed; `parse_copy_state` depends on it.
pub(crate) const COPY_STATE_FORMAT: &str = "#{pane_in_mode}\t#{scroll_position}\t#{pane_height}\t#{history_size}\t#{copy_cursor_x}\t#{copy_cursor_y}\t#{selection_present}\t#{rectangle_toggle}\t#{selection_start_x}\t#{selection_start_y}\t#{selection_end_x}\t#{selection_end_y}";

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
    // NOTE: a field can be EMPTY (not "0") — tmux expands #{selection_start_x}
    // and the other selection_* vars to "" when there is no active selection,
    // which is the normal state while scrolling/reading without selecting.
    // Treat an empty numeric field as 0 so the refresh snapshot still parses; a
    // MISSING field (too few) or non-numeric text still returns None.
    let mut next = || -> Option<u32> {
        let raw = f.next()?.trim();
        if raw.is_empty() {
            Some(0)
        } else {
            raw.parse::<u32>().ok()
        }
    };
    let pane_in_mode = next()? != 0;
    let scroll_position = next()?;
    let pane_height = next()? as u16;
    let history_size = next()?;
    let cursor_x = next()? as u16;
    let cursor_y = next()? as u16;
    let selection_present = next()? != 0;
    let rectangle = next()? != 0;
    let sel_start_x = next()? as u16;
    let sel_start_y = next()?;
    let sel_end_x = next()? as u16;
    let sel_end_y = next()?;
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
pub(crate) fn capture_offsets(scroll_position: u32, pane_height: u16) -> (i32, i32) {
    let start = -(scroll_position as i32);
    let end = pane_height as i32 - 1 - scroll_position as i32;
    (start, end)
}

/// Maps an absolute (history-relative) grid line to a visible viewport row:
/// `absolute_y - (history_size - scroll_position)`. Negative = above viewport.
pub fn absolute_to_visible_row(absolute_y: u32, history_size: u32, scroll_position: u32) -> i32 {
    let top = history_size as i32 - scroll_position as i32;
    absolute_y as i32 - top
}

/// What an in-flight command's reply will populate, keyed by its `CommandId`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum PendingReply {
    /// `list-windows` enumeration → per-row projection seed.
    ListWindows,
    /// `display-message #{client_name}`.
    ClientName,
    /// `display-message #{version}`.
    Version,
    /// `display-message #{window_id} #{pane_id}` active-pane query.
    ActivePane,
    /// Any `list-keys -T <table>` reply → `KeyBindings::install`.
    KeyBindings,
    /// Prefix-options query → `set_prefix_keys`.
    PrefixKeys,
    /// `#{mode-keys}` → `set_mode_keys`.
    ModeKeys,
    /// `aggressive-resize` option query → warn if `on`.
    AggressiveResize,
    /// `capture-pane` of a pane's screen.
    Capture { pane: PaneId },
    /// Cursor-position query paired with a [`PendingReply::Capture`].
    Cursor { pane: PaneId },
}

/// Correlates in-flight enumeration/query commands by [`CommandId`] and the
/// capture/cursor pairing buffers, so each drained reply routes to its handler.
#[derive(Resource, Default)]
pub(crate) struct EnumerationState {
    pub(crate) pending: HashMap<CommandId, PendingReply>,
    pub(crate) aggressive_resize_checked: bool,
    pub(crate) capture_awaiting_cursor: HashMap<PaneId, Vec<String>>,
    pub(crate) panes_with_cursor_pending: HashSet<PaneId>,
}

impl EnumerationState {
    /// Records `reply` under the id `send` returned, logging on send failure.
    pub(crate) fn register(&mut self, send: TmuxResult<CommandId>, reply: PendingReply) {
        match send {
            Ok(id) => {
                // NOTE: singleton query kinds keep the old `Option` last-write-wins
                // — a re-issued query must supersede any still-in-flight one of the
                // same kind, or BOTH ids stay in `pending` and dispatch twice (a
                // re-sent list-windows on %window-add while the attach enumeration
                // is still in flight would fire trigger_seed twice, and a re-queried
                // active-pane would fire TmuxActivePaneChanged twice). The four
                // concurrent KeyBindings tables and the per-pane Capture/Cursor kinds
                // are legitimately multi and exempt.
                if !matches!(
                    reply,
                    PendingReply::KeyBindings
                        | PendingReply::Capture { .. }
                        | PendingReply::Cursor { .. }
                ) {
                    self.pending.retain(|_, r| *r != reply);
                }
                self.pending.insert(id, reply);
            }
            Err(error) => tracing::warn!(?error, ?reply, "failed to send tmux query"),
        }
    }

    /// Whether a reply of `reply`'s kind is already in flight (replaces the old
    /// `Option::is_some` singleton guard for client-name / aggressive-resize).
    pub(crate) fn has_pending(&self, reply: PendingReply) -> bool {
        self.pending.values().any(|r| *r == reply)
    }

    /// Drops the in-flight entries a session switch invalidates: the
    /// capture/cursor pairs and the enumeration ids `send_session_enumeration`
    /// re-issues. A `HashMap` keyed by `CommandId` does not get the old
    /// `Option` fields' free last-write-wins overwrite, so a stale pre-switch
    /// `list-windows`/active-pane reply would otherwise mis-seed the new session.
    pub(crate) fn clear_for_session_switch(&mut self) {
        self.pending.retain(|_, r| {
            !matches!(
                r,
                PendingReply::Capture { .. }
                    | PendingReply::Cursor { .. }
                    | PendingReply::ListWindows
                    | PendingReply::ActivePane
                    | PendingReply::AggressiveResize
            )
        });
        self.capture_awaiting_cursor.clear();
        self.panes_with_cursor_pending.clear();
        self.aggressive_resize_checked = false;
    }
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
        let lines = vec!["1\t@1\t0\tb25f,80x24,0,0,0\tb25f,80x24,0,0,0\t\tmain".to_string()];
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
            "0\t@1\t0\tb25f,80x24,0,0,0\tb25f,80x24,0,0,0\t\tone".to_string(),
            "1\t@2\t1\tb25f,80x24,0,0,1\tb25f,80x24,0,0,1\t*\ttwo".to_string(),
        ];
        let rows = parse_window_rows(&lines).unwrap();
        assert_eq!((rows[0].active, rows[1].active), (false, true));
        assert_eq!((rows[0].id, rows[1].id), (WindowId(1), WindowId(2)));
    }

    #[test]
    fn name_with_tabs_is_preserved_as_last_field() {
        let lines =
            vec!["1\t@1\t0\tb25f,80x24,0,0,0\tb25f,80x24,0,0,0\t\tmy\tnamed\twin".to_string()];
        let rows = parse_window_rows(&lines).unwrap();
        assert_eq!(rows[0].name, "my\tnamed\twin");
    }

    #[test]
    fn bad_window_id_errors() {
        let lines = vec!["1\t1\t0\tb25f,80x24,0,0,0\tb25f,80x24,0,0,0\t\tx".to_string()];
        assert!(parse_window_rows(&lines).is_err());
    }

    #[test]
    fn empty_input_is_empty() {
        assert_eq!(parse_window_rows(&[]).unwrap(), vec![]);
    }

    #[test]
    fn parse_row_captures_window_index() {
        // Format order: active \t id \t index \t layout \t visible \t raw_flags \t name
        let line = "1\t@2\t3\tb25d,80x24,0,0,0\tb25d,80x24,0,0,0\t\tmy-win";
        let rows = parse_window_rows(&[line.to_string()]).expect("parse");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].index, 3);
        assert_eq!(rows[0].name, "my-win");
        assert!(rows[0].active);
    }

    #[test]
    fn capture_offsets_match_verified_formula() {
        assert_eq!(capture_offsets(12, 8), (-12, -5));
        assert_eq!(capture_offsets(0, 8), (0, 7));
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
        assert!(s.pane_in_mode);
        assert_eq!(s.scroll_position, 3);
        assert_eq!(s.pane_height, 8);
        assert_eq!(s.history_size, 53);
        assert_eq!((s.cursor_x, s.cursor_y), (6, 7));
        assert!(s.selection_present);
        assert!(!s.rectangle);
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
    fn parse_copy_state_treats_empty_selection_fields_as_zero() {
        // tmux expands #{selection_start_x} etc. to EMPTY (not "0") when there is
        // no active selection — the normal state while scrolling/reading without
        // selecting. The line has all 12 tab fields; the last 4 are empty.
        let s = parse_copy_state("1\t0\t10\t31\t0\t9\t0\t0\t\t\t\t")
            .expect("must parse with empty (no-selection) selection fields");
        assert!(s.pane_in_mode);
        assert_eq!(s.scroll_position, 0);
        assert_eq!((s.cursor_x, s.cursor_y), (0, 9));
        assert!(!s.selection_present);
        assert_eq!((s.sel_start_x, s.sel_start_y), (0, 0));
        assert_eq!((s.sel_end_x, s.sel_end_y), (0, 0));
    }

    #[test]
    fn parse_row_reads_raw_flags_before_name() {
        // active, id, index, layout, visible_layout, raw_flags, name
        let line = "1\t@2\t0\tb25f,80x24,0,0,1\tb25f,80x24,0,0,1\t*Z\tmy editor".to_string();
        let rows = parse_window_rows(&[line]).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, WindowId(2));
        assert!(rows[0].active);
        assert_eq!(rows[0].name, "my editor");
        assert_eq!(rows[0].flags, WindowFlags::ZOOM);
    }

    #[test]
    fn parse_row_prefers_visible_layout_when_present() {
        // field order: active id index window_layout window_visible_layout flags name
        // window_layout = 80x24, visible_layout = 40x12; must adopt visible dims.
        // The parser is lenient about checksum mismatches — 0000 is accepted for both.
        let row = "1\t@1\t0\t0000,80x24,0,0,1\t0000,40x12,0,0,1\t*\tbash";
        let parsed = parse_window_rows(&[row.to_string()]).expect("row parses");
        let dims = parsed[0].layout.root.dims();
        assert_eq!(
            (dims.width, dims.height),
            (40, 12),
            "must use visible_layout"
        );
    }

    #[test]
    fn parse_row_falls_back_to_window_layout_when_visible_empty() {
        // visible_layout field is empty — must fall back to window_layout (80x24).
        let row = "1\t@1\t0\t0000,80x24,0,0,1\t\t*\tbash";
        let parsed = parse_window_rows(&[row.to_string()]).expect("row parses");
        let dims = parsed[0].layout.root.dims();
        assert_eq!(
            (dims.width, dims.height),
            (80, 24),
            "fallback to window_layout"
        );
    }

    #[test]
    fn version_supports_per_window_refresh_is_lenient_about_suffixes() {
        assert!(version_supports_per_window_refresh("3.6a"));
        assert!(version_supports_per_window_refresh("3.4"));
        assert!(version_supports_per_window_refresh("next-3.7"));
        assert!(!version_supports_per_window_refresh("3.3"));
        assert!(!version_supports_per_window_refresh("2.9"));
        assert!(!version_supports_per_window_refresh("garbage"));
    }

    #[test]
    fn clear_for_session_switch_drops_enumeration_but_keeps_keybindings() {
        let mut state = EnumerationState::default();
        state
            .pending
            .insert(CommandId(1), PendingReply::ListWindows);
        state.pending.insert(CommandId(2), PendingReply::ActivePane);
        state
            .pending
            .insert(CommandId(3), PendingReply::KeyBindings);
        state
            .pending
            .insert(CommandId(4), PendingReply::Capture { pane: PaneId(7) });
        state
            .pending
            .insert(CommandId(5), PendingReply::AggressiveResize);
        state.aggressive_resize_checked = true;
        state.clear_for_session_switch();
        assert_eq!(
            state.pending.get(&CommandId(3)),
            Some(&PendingReply::KeyBindings),
            "keybindings entry must survive"
        );
        assert!(
            !state.pending.contains_key(&CommandId(1)),
            "stale list-windows dropped"
        );
        assert!(
            !state.pending.contains_key(&CommandId(2)),
            "stale active-pane dropped"
        );
        assert!(
            !state.pending.contains_key(&CommandId(4)),
            "capture dropped"
        );
        assert!(
            !state.pending.contains_key(&CommandId(5)),
            "stale aggressive-resize dropped so new session is re-checked"
        );
        assert!(!state.aggressive_resize_checked, "aggressive guard reset");
    }

    #[test]
    fn register_supersedes_in_flight_singleton_but_keeps_concurrent_kinds() {
        let mut state = EnumerationState::default();
        state.register(Ok(CommandId(1)), PendingReply::ListWindows);
        state.register(Ok(CommandId(2)), PendingReply::ListWindows);
        assert!(
            !state.pending.contains_key(&CommandId(1)),
            "the superseded list-windows id is dropped (old Option last-write-wins)"
        );
        assert_eq!(
            state.pending.get(&CommandId(2)),
            Some(&PendingReply::ListWindows),
            "only the latest list-windows id remains, so trigger_seed fires once"
        );

        state.register(Ok(CommandId(3)), PendingReply::ActivePane);
        state.register(Ok(CommandId(4)), PendingReply::ActivePane);
        assert!(
            !state.pending.contains_key(&CommandId(3)),
            "stale active-pane dropped"
        );
        assert_eq!(
            state.pending.get(&CommandId(4)),
            Some(&PendingReply::ActivePane)
        );

        state.register(Ok(CommandId(5)), PendingReply::KeyBindings);
        state.register(Ok(CommandId(6)), PendingReply::KeyBindings);
        assert_eq!(
            state.pending.get(&CommandId(5)),
            Some(&PendingReply::KeyBindings),
            "the four list-keys tables are concurrent — earlier KeyBindings entries are kept"
        );
        assert_eq!(
            state.pending.get(&CommandId(6)),
            Some(&PendingReply::KeyBindings)
        );

        state.register(Ok(CommandId(7)), PendingReply::Capture { pane: PaneId(1) });
        state.register(Ok(CommandId(8)), PendingReply::Capture { pane: PaneId(2) });
        assert_eq!(
            state.pending.get(&CommandId(7)),
            Some(&PendingReply::Capture { pane: PaneId(1) }),
            "per-pane captures are independent — both kept"
        );
        assert_eq!(
            state.pending.get(&CommandId(8)),
            Some(&PendingReply::Capture { pane: PaneId(2) })
        );
    }
}
