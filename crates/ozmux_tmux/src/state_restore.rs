//! Pane terminal-state restoration for adopt-time seeding: the tmux format
//! that dumps a pane's modes, its parser, and the VT byte synthesis that
//! replays captured content + modes into the display mirror.

use std::collections::HashMap;

/// Tab-separated `key=#{key}` format dumping one pane's terminal state.
///
/// NOTE: unknown variables expand to the empty string on older tmux (e.g.
/// `bracket_paste_flag` before 3.7), and the parser degrades empty to off —
/// this is what makes the query version-gate-free.
pub(crate) const PANE_STATE_FORMAT: &str = "pane_id=#{pane_id}\talternate_on=#{alternate_on}\talternate_saved_x=#{alternate_saved_x}\talternate_saved_y=#{alternate_saved_y}\tcursor_x=#{cursor_x}\tcursor_y=#{cursor_y}\tscroll_region_upper=#{scroll_region_upper}\tscroll_region_lower=#{scroll_region_lower}\tpane_tabs=#{pane_tabs}\tcursor_flag=#{cursor_flag}\tinsert_flag=#{insert_flag}\tkeypad_cursor_flag=#{keypad_cursor_flag}\tkeypad_flag=#{keypad_flag}\twrap_flag=#{wrap_flag}\torigin_flag=#{origin_flag}\tmouse_standard_flag=#{mouse_standard_flag}\tmouse_button_flag=#{mouse_button_flag}\tmouse_all_flag=#{mouse_all_flag}\tmouse_utf8_flag=#{mouse_utf8_flag}\tmouse_sgr_flag=#{mouse_sgr_flag}\tbracket_paste_flag=#{bracket_paste_flag}";

/// One pane's terminal state parsed from a [`PANE_STATE_FORMAT`] reply line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PaneState {
    pub(crate) alternate_on: bool,
    pub(crate) alternate_saved_x: u16,
    pub(crate) alternate_saved_y: u16,
    pub(crate) cursor_x: u16,
    pub(crate) cursor_y: u16,
    pub(crate) scroll_region_upper: u16,
    pub(crate) scroll_region_lower: u16,
    pub(crate) tabs: Vec<u16>,
    pub(crate) cursor_visible: bool,
    pub(crate) insert: bool,
    pub(crate) app_cursor_keys: bool,
    pub(crate) app_keypad: bool,
    pub(crate) wrap: bool,
    pub(crate) origin: bool,
    pub(crate) mouse_standard: bool,
    pub(crate) mouse_button: bool,
    pub(crate) mouse_all: bool,
    pub(crate) mouse_utf8: bool,
    pub(crate) mouse_sgr: bool,
    pub(crate) bracketed_paste: bool,
}

/// Parses one [`PANE_STATE_FORMAT`] reply line. Missing, empty, or
/// non-numeric fields degrade to zero/off; `cursor_flag` and `wrap_flag`
/// degrade to ON (the terminal defaults). Never fails.
pub(crate) fn parse_pane_state(line: &str) -> PaneState {
    let fields = split_state_fields(line);
    let flag = |key: &str| fields.get(key).is_some_and(|v| *v == "1");
    let flag_default_on = |key: &str| fields.get(key).is_none_or(|v| v.is_empty() || *v == "1");
    let num = |key: &str| -> u16 {
        fields
            .get(key)
            .and_then(|v| v.parse().ok())
            .unwrap_or_default()
    };
    PaneState {
        alternate_on: flag("alternate_on"),
        alternate_saved_x: num("alternate_saved_x"),
        alternate_saved_y: num("alternate_saved_y"),
        cursor_x: num("cursor_x"),
        cursor_y: num("cursor_y"),
        scroll_region_upper: num("scroll_region_upper"),
        scroll_region_lower: num("scroll_region_lower"),
        tabs: fields
            .get("pane_tabs")
            .map(|v| v.split(',').filter_map(|t| t.parse().ok()).collect())
            .unwrap_or_default(),
        cursor_visible: flag_default_on("cursor_flag"),
        insert: flag("insert_flag"),
        app_cursor_keys: flag("keypad_cursor_flag"),
        app_keypad: flag("keypad_flag"),
        wrap: flag_default_on("wrap_flag"),
        origin: flag("origin_flag"),
        mouse_standard: flag("mouse_standard_flag"),
        mouse_button: flag("mouse_button_flag"),
        mouse_all: flag("mouse_all_flag"),
        mouse_utf8: flag("mouse_utf8_flag"),
        mouse_sgr: flag("mouse_sgr_flag"),
        bracketed_paste: flag("bracket_paste_flag"),
    }
}

/// Splits a state reply into `key -> value`, re-splitting on a literal
/// `\t` sequence when the transport escaped the tab separator (a known
/// tmux transport quirk iTerm2 also works around).
fn split_state_fields(line: &str) -> HashMap<&str, &str> {
    let mut parts: Vec<&str> = line.split('\t').collect();
    if parts.len() == 1 && line.contains("\\t") {
        parts = line.split("\\t").collect();
    }
    parts.iter().filter_map(|p| p.split_once('=')).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kv(pairs: &[(&str, &str)]) -> String {
        pairs
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("\t")
    }

    #[test]
    fn format_lists_every_key_tab_separated() {
        for key in [
            "pane_id",
            "alternate_on",
            "alternate_saved_x",
            "alternate_saved_y",
            "cursor_x",
            "cursor_y",
            "scroll_region_upper",
            "scroll_region_lower",
            "pane_tabs",
            "cursor_flag",
            "insert_flag",
            "keypad_cursor_flag",
            "keypad_flag",
            "wrap_flag",
            "origin_flag",
            "mouse_standard_flag",
            "mouse_button_flag",
            "mouse_all_flag",
            "mouse_utf8_flag",
            "mouse_sgr_flag",
            "bracket_paste_flag",
        ] {
            assert!(
                PANE_STATE_FORMAT.contains(&format!("{key}=#{{{key}}}")),
                "missing {key}"
            );
        }
        assert!(PANE_STATE_FORMAT.contains('\t'));
    }

    #[test]
    fn parses_nvim_like_state() {
        let line = kv(&[
            ("pane_id", "%3"),
            ("alternate_on", "1"),
            ("alternate_saved_x", "12"),
            ("alternate_saved_y", "40"),
            ("cursor_x", "5"),
            ("cursor_y", "2"),
            ("scroll_region_upper", "0"),
            ("scroll_region_lower", "48"),
            ("pane_tabs", "8,16,24"),
            ("cursor_flag", "1"),
            ("insert_flag", "0"),
            ("keypad_cursor_flag", "1"),
            ("keypad_flag", "1"),
            ("wrap_flag", "0"),
            ("origin_flag", "0"),
            ("mouse_standard_flag", "0"),
            ("mouse_button_flag", "1"),
            ("mouse_all_flag", "0"),
            ("mouse_utf8_flag", "0"),
            ("mouse_sgr_flag", "1"),
            ("bracket_paste_flag", "1"),
        ]);
        let s = parse_pane_state(&line);
        assert!(s.alternate_on && s.mouse_button && s.mouse_sgr && s.bracketed_paste);
        assert!(s.app_cursor_keys && s.app_keypad && !s.wrap && !s.mouse_standard);
        assert_eq!((s.alternate_saved_x, s.alternate_saved_y), (12, 40));
        assert_eq!((s.cursor_x, s.cursor_y), (5, 2));
        assert_eq!((s.scroll_region_upper, s.scroll_region_lower), (0, 48));
        assert_eq!(s.tabs, vec![8, 16, 24]);
    }

    #[test]
    fn missing_empty_and_garbage_fields_degrade_to_off() {
        // bracket_paste_flag missing entirely (old tmux expands unknown to ""),
        // mouse_sgr_flag empty, cursor_x garbage.
        let line = kv(&[
            ("alternate_on", "0"),
            ("mouse_sgr_flag", ""),
            ("cursor_x", "abc"),
        ]);
        let s = parse_pane_state(&line);
        assert!(!s.bracketed_paste && !s.mouse_sgr && !s.alternate_on);
        assert_eq!(s.cursor_x, 0);
        assert!(s.tabs.is_empty());
    }

    #[test]
    fn resplits_on_escaped_tab_transport_bug() {
        // Some tmux/transport combinations escape the tab separator as \\t.
        let line = kv(&[("alternate_on", "1"), ("cursor_y", "7")]).replace('\t', "\\t");
        let s = parse_pane_state(&line);
        assert!(s.alternate_on);
        assert_eq!(s.cursor_y, 7);
    }

    #[test]
    fn cursor_visible_and_wrap_default_on_when_missing() {
        let s = parse_pane_state("");
        assert!(s.cursor_visible && s.wrap);
        assert!(!s.alternate_on);
    }
}
