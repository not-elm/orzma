//! Pane terminal-state restoration for adopt-time seeding: the tmux format
//! that dumps a pane's modes, its parser, and the VT byte synthesis that
//! replays captured content + modes into the display mirror.

use std::collections::HashMap;
use tmux_control_parser::unescape_capture;

/// Tab-separated `key=#{key}` format dumping one pane's terminal state.
///
/// NOTE: unknown variables expand to the empty string on older tmux (e.g.
/// `bracket_paste_flag` before 3.7), and the parser degrades empty to off —
/// this is what makes the query version-gate-free.
pub(crate) const PANE_STATE_FORMAT: &str = "pane_id=#{pane_id}\talternate_on=#{alternate_on}\talternate_saved_x=#{alternate_saved_x}\talternate_saved_y=#{alternate_saved_y}\tcursor_x=#{cursor_x}\tcursor_y=#{cursor_y}\tscroll_region_upper=#{scroll_region_upper}\tscroll_region_lower=#{scroll_region_lower}\tpane_tabs=#{pane_tabs}\tcursor_flag=#{cursor_flag}\tinsert_flag=#{insert_flag}\tkeypad_cursor_flag=#{keypad_cursor_flag}\tkeypad_flag=#{keypad_flag}\twrap_flag=#{wrap_flag}\torigin_flag=#{origin_flag}\tmouse_standard_flag=#{mouse_standard_flag}\tmouse_button_flag=#{mouse_button_flag}\tmouse_all_flag=#{mouse_all_flag}\tmouse_utf8_flag=#{mouse_utf8_flag}\tmouse_sgr_flag=#{mouse_sgr_flag}\tbracket_paste_flag=#{bracket_paste_flag}\tpane_height=#{pane_height}";

/// One pane's terminal state parsed from a [`PANE_STATE_FORMAT`] reply line.
///
/// NOTE: fields are private — every read lives in this module (`restore_to_bytes`
/// / `append_state_bytes`); external code only holds this behind `Slot<PaneState>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PaneState {
    alternate_on: bool,
    alternate_saved_x: u16,
    alternate_saved_y: u16,
    cursor_x: u16,
    cursor_y: u16,
    scroll_region_upper: u16,
    scroll_region_lower: u16,
    tabs: Vec<u16>,
    cursor_visible: bool,
    insert: bool,
    app_cursor_keys: bool,
    app_keypad: bool,
    wrap: bool,
    origin: bool,
    mouse_standard: bool,
    mouse_button: bool,
    mouse_all: bool,
    mouse_utf8: bool,
    mouse_sgr: bool,
    bracketed_paste: bool,
    /// The pane's current row count, queried in the same reply batch as the
    /// capture. Takes precedence over `GridCapture::Full`'s `pane_height` (an
    /// ECS snapshot taken at `Added<TmuxPane>` time) for the alt-screen split
    /// in `restore_to_bytes`, since a resize between that snapshot and the
    /// capture reply landing would otherwise split `base` at a stale offset.
    pane_height: u16,
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
        pane_height: num("pane_height"),
    }
}

/// Captured grid content for one restore pass.
///
/// `Full` carries the adopt-time pair: `base` is the default capture
/// (primary history + the CURRENT visible screen — which is the alt screen
/// while alternate is active) and `saved_primary` is the `-a` capture (the
/// primary screen snapshot tmux saved on alt entry). `VisibleOnly` is the
/// light re-seed's single visible capture.
pub(crate) enum GridCapture {
    Full {
        base: Vec<String>,
        saved_primary: Vec<String>,
        pane_height: u16,
    },
    VisibleOnly {
        rows: Vec<String>,
    },
}

/// Synthesizes one VT byte stream restoring content, terminal state, and
/// pending output, in that order, for replay through the pane mirror.
///
/// NOTE: the reset prefix MUST precede the ESC[2J erase and the row replay —
/// stale SGR floods the erase (the blue-pane bug) and a stale DECSTBM/DECOM
/// scrolls the CRLF replay within wrong margins. Content is replayed before
/// modes so `-J`-joined long lines re-wrap under the forced wrap=on the reset
/// prefix now includes (a light re-seed reuses a long-lived mirror that can
/// still carry a PRIOR restore's wrap=off, which would otherwise stop the
/// replayed long line from re-wrapping before the real `wrap` flag is
/// reapplied afterward).
pub(crate) fn restore_to_bytes(
    grid: &GridCapture,
    state: Option<&PaneState>,
    pending: &[String],
) -> Vec<u8> {
    let mut bytes = b"\x1b[r\x1b[?6l\x1b[?7h\x1b[0m\x1b[H\x1b[2J".to_vec();
    match grid {
        GridCapture::Full {
            base,
            saved_primary,
            pane_height,
        } => {
            let alt = state.is_some_and(|s| s.alternate_on);
            // NOTE: state.pane_height is queried in the same reply batch as
            // the capture, so it reflects the pane's dims far more closely
            // than `pane_height` (an ECS snapshot taken at `Added<TmuxPane>`
            // time, which can go stale if the pane resizes before the reply
            // lands) — falling back to it only when the state query itself
            // failed.
            let height = state
                .map(|s| s.pane_height)
                .filter(|h| *h > 0)
                .unwrap_or(*pane_height) as usize;
            if alt && base.len() >= height {
                let (history, alt_rows) = base.split_at(base.len() - height);
                append_rows(&mut bytes, history);
                if !history.is_empty() && !saved_primary.is_empty() {
                    bytes.extend_from_slice(b"\r\n");
                }
                append_rows(&mut bytes, saved_primary);
                let s = state.expect("alt implies state");
                bytes.extend_from_slice(
                    format!(
                        "\x1b[{};{}H",
                        s.alternate_saved_y.saturating_add(1),
                        s.alternate_saved_x.saturating_add(1)
                    )
                    .as_bytes(),
                );
                bytes.extend_from_slice(b"\x1b[?1049h\x1b[H\x1b[2J");
                append_rows(&mut bytes, alt_rows);
            } else {
                append_rows(&mut bytes, base);
            }
        }
        GridCapture::VisibleOnly { rows } => {
            // NOTE: alacritty guards 1049 (set no-ops while already in alt,
            // unset no-ops outside alt), so re-asserting membership is
            // idempotent in the synced case and converges a desynced mirror.
            if let Some(state) = state {
                bytes.extend_from_slice(if state.alternate_on {
                    b"\x1b[?1049h"
                } else {
                    b"\x1b[?1049l"
                });
                bytes.extend_from_slice(b"\x1b[H\x1b[2J");
            }
            append_rows(&mut bytes, rows);
        }
    }
    if let Some(state) = state {
        append_state_bytes(&mut bytes, state);
    }
    if !pending.is_empty() {
        bytes.extend_from_slice(&unescape_capture(pending.join("\n").as_bytes()));
    }
    bytes
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

/// Appends the DECSET/DECSTBM/tabstop/cursor tail restoring `state` onto a
/// content-replay byte stream. Every mode is emitted in BOTH directions
/// (`h` when set, `l` when clear) so the light re-seed path — where the
/// mirror still carries the previous seed's modes — converges without
/// assuming a reset baseline. Ends with the cursor CUP.
///
/// NOTE: DECSTBM and DECOM home the cursor, so the cursor CUP MUST stay
/// after both; under DECOM the CUP row is region-relative and is converted
/// from tmux's absolute `cursor_y` here.
fn append_state_bytes(bytes: &mut Vec<u8>, state: &PaneState) {
    let mode = |bytes: &mut Vec<u8>, seq: &str, on: bool| {
        bytes.extend_from_slice(seq.as_bytes());
        bytes.push(if on { b'h' } else { b'l' });
    };
    mode(bytes, "\x1b[?1000", state.mouse_standard);
    mode(bytes, "\x1b[?1002", state.mouse_button);
    mode(bytes, "\x1b[?1003", state.mouse_all);
    mode(bytes, "\x1b[?1005", state.mouse_utf8);
    mode(bytes, "\x1b[?1006", state.mouse_sgr);
    mode(bytes, "\x1b[?1", state.app_cursor_keys);
    bytes.extend_from_slice(if state.app_keypad { b"\x1b=" } else { b"\x1b>" });
    mode(bytes, "\x1b[4", state.insert);
    mode(bytes, "\x1b[?2004", state.bracketed_paste);
    mode(bytes, "\x1b[?25", state.cursor_visible);
    mode(bytes, "\x1b[?7", state.wrap);
    mode(bytes, "\x1b[?6", state.origin);
    if state.scroll_region_lower > state.scroll_region_upper {
        bytes.extend_from_slice(
            format!(
                "\x1b[{};{}r",
                state.scroll_region_upper.saturating_add(1),
                state.scroll_region_lower.saturating_add(1)
            )
            .as_bytes(),
        );
    }
    if !state.tabs.is_empty() {
        bytes.extend_from_slice(b"\x1b[3g");
        for col in &state.tabs {
            bytes.extend_from_slice(format!("\x1b[{}G\x1bH", col.saturating_add(1)).as_bytes());
        }
    }
    let row = if state.origin {
        state
            .cursor_y
            .saturating_sub(state.scroll_region_upper)
            .saturating_add(1)
    } else {
        state.cursor_y.saturating_add(1)
    };
    bytes.extend_from_slice(
        format!("\x1b[{};{}H", row, state.cursor_x.saturating_add(1)).as_bytes(),
    );
}

fn append_rows(bytes: &mut Vec<u8>, rows: &[String]) {
    bytes.extend_from_slice(rows.join("\r\n").as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use ozma_tty_engine::{
        CellCoord, Column, Line, Point, SelectionType, Side, TermMode, TerminalHandle, WheelAction,
        WheelConfig, WheelModifiers,
    };

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
            "pane_height",
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

    fn state_bytes(state: &PaneState) -> Vec<u8> {
        let mut bytes = Vec::new();
        append_state_bytes(&mut bytes, state);
        bytes
    }

    fn default_state() -> PaneState {
        parse_pane_state("")
    }

    #[test]
    fn mouse_flags_emit_exclusive_tracking_and_encoding() {
        let mut s = default_state();
        s.mouse_button = true;
        s.mouse_sgr = true;
        let b = state_bytes(&s);
        let text = String::from_utf8_lossy(&b);
        assert!(text.contains("\x1b[?1000l") && text.contains("\x1b[?1003l"));
        assert!(text.contains("\x1b[?1002h") && text.contains("\x1b[?1006h"));
        assert!(text.contains("\x1b[?1005l"));
    }

    #[test]
    fn all_off_state_emits_explicit_resets() {
        let b = state_bytes(&default_state());
        let text = String::from_utf8_lossy(&b);
        for seq in [
            "\x1b[?1000l",
            "\x1b[?1002l",
            "\x1b[?1003l",
            "\x1b[?1006l",
            "\x1b[?1l",
            "\x1b[4l",
            "\x1b[?2004l",
            "\x1b>",
        ] {
            assert!(text.contains(seq), "missing {seq:?}");
        }
        assert!(text.contains("\x1b[?25h") && text.contains("\x1b[?7h"));
    }

    #[test]
    fn hidden_cursor_and_no_wrap_emit_low() {
        let mut s = default_state();
        s.cursor_visible = false;
        s.wrap = false;
        let b = state_bytes(&s);
        let text = String::from_utf8_lossy(&b);
        assert!(text.contains("\x1b[?25l") && text.contains("\x1b[?7l"));
    }

    #[test]
    fn cursor_cup_is_absolute_without_origin() {
        let mut s = default_state();
        s.cursor_x = 4;
        s.cursor_y = 9;
        let b = state_bytes(&s);
        let text = String::from_utf8_lossy(&b);
        assert!(text.ends_with("\x1b[10;5H"), "got {text:?}");
    }

    #[test]
    fn cursor_cup_converts_to_region_relative_under_origin() {
        let mut s = default_state();
        s.origin = true;
        s.scroll_region_upper = 3;
        s.scroll_region_lower = 20;
        s.cursor_x = 0;
        s.cursor_y = 5;
        let b = state_bytes(&s);
        let text = String::from_utf8_lossy(&b);
        // DECOM makes CUP region-relative: row = cursor_y - upper + 1 = 3.
        assert!(text.contains("\x1b[?6h"));
        assert!(text.contains("\x1b[4;21r"));
        assert!(text.ends_with("\x1b[3;1H"), "got {text:?}");
    }

    #[test]
    fn tabstops_clear_then_set_each_column() {
        let mut s = default_state();
        s.tabs = vec![8, 16];
        let b = state_bytes(&s);
        let text = String::from_utf8_lossy(&b);
        let clear = text.find("\x1b[3g").expect("clear-all tabs");
        assert!(text[clear..].contains("\x1b[9G\x1bH"));
        assert!(text[clear..].contains("\x1b[17G\x1bH"));
    }

    #[test]
    fn scroll_region_precedes_cursor_and_follows_modes() {
        let mut s = default_state();
        s.scroll_region_upper = 2;
        s.scroll_region_lower = 30;
        s.cursor_y = 4;
        let b = state_bytes(&s);
        let text = String::from_utf8_lossy(&b);
        let region = text.find("\x1b[3;31r").expect("DECSTBM");
        let cup = text.rfind("\x1b[").expect("cursor CUP");
        assert!(region < cup);
    }

    fn rows(prefix: &str, n: usize) -> Vec<String> {
        (0..n).map(|i| format!("{prefix}{i}")).collect()
    }

    #[test]
    fn visible_only_matches_reset_prefix_plus_rows() {
        let grid = GridCapture::VisibleOnly {
            rows: vec!["hello".into(), "world".into()],
        };
        let b = restore_to_bytes(&grid, None, &[]);
        assert!(b.starts_with(b"\x1b[r\x1b[?6l\x1b[?7h\x1b[0m\x1b[H\x1b[2J"));
        assert!(String::from_utf8_lossy(&b).contains("hello\r\nworld"));
    }

    #[test]
    fn visible_only_reasserts_alt_membership_from_state() {
        let mut state = parse_pane_state("");
        state.alternate_on = true;
        let grid = GridCapture::VisibleOnly {
            rows: vec!["v".into()],
        };
        let text =
            String::from_utf8_lossy(&restore_to_bytes(&grid, Some(&state), &[])).into_owned();
        assert!(text.find("\x1b[?1049h").expect("alt reasserted") < text.find('v').unwrap());
    }

    #[test]
    fn full_without_alt_replays_base_wholesale() {
        let grid = GridCapture::Full {
            base: rows("h", 5),
            saved_primary: vec![],
            pane_height: 3,
        };
        let state = parse_pane_state("");
        let text =
            String::from_utf8_lossy(&restore_to_bytes(&grid, Some(&state), &[])).into_owned();
        assert!(text.contains("h0\r\nh1\r\nh2\r\nh3\r\nh4"));
        assert!(!text.contains("\x1b[?1049h"));
    }

    #[test]
    fn full_with_alt_splits_base_tail_into_alt_screen() {
        // base = 5 rows of history + 3 rows of CURRENT (alt) screen;
        // saved_primary = the primary screen snapshot taken at alt entry.
        let grid = GridCapture::Full {
            base: [rows("hist", 5), rows("alt", 3)].concat(),
            saved_primary: rows("prim", 3),
            pane_height: 3,
        };
        let mut state = parse_pane_state("");
        state.alternate_on = true;
        state.alternate_saved_x = 2;
        state.alternate_saved_y = 1;
        let text =
            String::from_utf8_lossy(&restore_to_bytes(&grid, Some(&state), &[])).into_owned();
        let hist = text.find("hist4").expect("history replayed");
        let prim = text.find("prim0").expect("saved primary replayed");
        let saved_cup = text.find("\x1b[2;3H").expect("saved cursor placed");
        let alt_on = text.find("\x1b[?1049h").expect("alt entered");
        let alt = text.find("alt0").expect("alt screen replayed");
        assert!(
            hist < prim && prim < saved_cup && saved_cup < alt_on && alt_on < alt,
            "wrong order in {text:?}"
        );
    }

    #[test]
    fn full_with_alt_but_short_base_degrades_to_primary_only() {
        let grid = GridCapture::Full {
            base: rows("h", 2),
            saved_primary: rows("prim", 3),
            pane_height: 3,
        };
        let mut state = parse_pane_state("");
        state.alternate_on = true;
        let text =
            String::from_utf8_lossy(&restore_to_bytes(&grid, Some(&state), &[])).into_owned();
        assert!(!text.contains("\x1b[?1049h"));
        assert!(text.contains("h0\r\nh1"));
    }

    #[test]
    fn full_with_alt_prefers_state_pane_height_over_stale_snapshot() {
        // Regression: `pane_height` on `GridCapture::Full` is an ECS snapshot
        // taken at `Added<TmuxPane>` time and can go stale if the pane resizes
        // before the capture reply lands. `state.pane_height`, queried in the
        // same reply batch as the capture, must win the split so a stale
        // snapshot cannot misplace the alt/history boundary.
        let grid = GridCapture::Full {
            base: [rows("hist", 5), rows("alt", 3)].concat(),
            saved_primary: rows("prim", 3),
            pane_height: 10, // stale: would swallow all of `base` as "history"
        };
        let mut state = parse_pane_state("");
        state.alternate_on = true;
        state.pane_height = 3; // authoritative: the real current height
        let text =
            String::from_utf8_lossy(&restore_to_bytes(&grid, Some(&state), &[])).into_owned();
        assert!(text.contains("\x1b[?1049h"), "must still enter alt screen");
        let hist = text.find("hist4").expect("history replayed");
        let alt = text.find("alt0").expect("alt screen replayed");
        assert!(hist < alt, "wrong order in {text:?}");
    }

    #[test]
    fn pending_output_is_decoded_and_last() {
        let grid = GridCapture::VisibleOnly {
            rows: vec!["x".into()],
        };
        let state = parse_pane_state("");
        let b = restore_to_bytes(&grid, Some(&state), &["tail\\033[".to_string()]);
        assert!(b.ends_with(b"tail\x1b["));
    }

    #[test]
    fn state_section_comes_after_content() {
        let grid = GridCapture::VisibleOnly {
            rows: vec!["content".into()],
        };
        let mut state = parse_pane_state("");
        state.mouse_sgr = true;
        let text =
            String::from_utf8_lossy(&restore_to_bytes(&grid, Some(&state), &[])).into_owned();
        assert!(text.find("content").unwrap() < text.find("\x1b[?1006h").unwrap());
    }

    fn nvim_like_restore() -> Vec<u8> {
        let grid = GridCapture::Full {
            base: [rows("hist", 10), rows("alt", 4)].concat(),
            saved_primary: rows("prim", 4),
            pane_height: 4,
        };
        let mut state = parse_pane_state("");
        state.alternate_on = true;
        state.mouse_button = true;
        state.mouse_sgr = true;
        state.app_cursor_keys = true;
        restore_to_bytes(&grid, Some(&state), &[])
    }

    #[test]
    fn adopted_nvim_pane_routes_wheel_to_pty_not_scrollback() {
        let mut handle = TerminalHandle::detached(80, 4);
        handle.advance(&nvim_like_restore());
        let modes = handle.current_modes();
        assert!(modes.contains(TermMode::MOUSE_DRAG), "1002 must be set");
        assert!(modes.contains(TermMode::SGR_MOUSE), "1006 must be set");
        assert!(modes.contains(TermMode::ALT_SCREEN), "1049 must be set");
        assert!(modes.contains(TermMode::APP_CURSOR), "DECCKM must be set");
        let action = WheelAction::route(
            modes,
            -1,
            CellCoord { col: 1, row: 1 },
            WheelModifiers::default(),
            &WheelConfig::default(),
        );
        assert!(
            matches!(action, WheelAction::WriteToPty(ref b) if b.starts_with(b"\x1b[<64;")),
            "wheel must be forwarded as an SGR report, got {action:?}"
        );
    }

    #[test]
    fn adopted_shell_pane_restores_history_into_scrollback() {
        let grid = GridCapture::Full {
            base: rows("line", 20),
            saved_primary: vec![],
            pane_height: 4,
        };
        let state = parse_pane_state("");
        let mut handle = TerminalHandle::detached(80, 4);
        handle.advance(&restore_to_bytes(&grid, Some(&state), &[]));
        assert!(!handle.current_modes().contains(TermMode::ALT_SCREEN));
        let snapshot = handle.vi_indicator_snapshot();
        assert!(
            snapshot.history_size >= 10,
            "history must land in scrollback"
        );
    }

    fn visible_line(handle: &mut TerminalHandle, row: i32) -> String {
        handle.selection_start_at_vt_only(
            Point::new(Line(row), Column(0)),
            Side::Left,
            SelectionType::Lines,
        );
        let text = handle.selection_to_string().unwrap_or_default();
        handle.selection_clear_vt_only();
        text.trim_end().to_string()
    }

    #[test]
    fn adopted_nvim_pane_places_alt_and_primary_grids_correctly() {
        // Regression guard for the capture-semantics inversion (spec §2/§7):
        // cmd1's tail must land in the ALT grid, cmd2 in the PRIMARY grid.
        let mut handle = TerminalHandle::detached(80, 4);
        handle.advance(&nvim_like_restore());
        assert!(handle.is_in_alt_screen());
        assert_eq!(
            visible_line(&mut handle, 0),
            "alt0",
            "alt grid must show cmd1 tail"
        );
        handle.advance(b"\x1b[?1049l");
        assert_eq!(
            visible_line(&mut handle, 0),
            "prim0",
            "primary grid must show cmd2 rows"
        );
    }
}
