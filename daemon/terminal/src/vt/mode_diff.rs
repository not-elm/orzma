//! Term::mode() before/after diff helper.
//!
//! alacritty_terminal の `TermMode` bitflags を比較し、wire spec
//! § 4.7 で定義された mode 文字列 (alt-screen, bracketed-paste 等)
//! の追加/削除リストを生成する。

use alacritty_terminal::term::TermMode;

/// Mode flag transition between two Term states.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModeChange {
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

impl ModeChange {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty()
    }
}

/// 観測対象の mode flag と wire spec 文字列の対応表。
///
/// 定数名は alacritty_terminal 0.26 の `term::TermMode` 実体に一致する
/// (src/term/mod.rs L55-87 で `bitflags!` 定義)。wire spec § 4.7 に
/// 列挙された mode のみを採用しており、`LINE_WRAP` 等 alacritty 固有の
/// flag は意図的に除外している。
const TRACKED_MODES: &[(TermMode, &str)] = &[
    (TermMode::ALT_SCREEN, "alt-screen"),
    (TermMode::BRACKETED_PASTE, "bracketed-paste"),
    (TermMode::APP_CURSOR, "app-cursor-keys"),
    (TermMode::FOCUS_IN_OUT, "focus-events"),
    (TermMode::MOUSE_REPORT_CLICK, "mouse-x10"),
    (TermMode::MOUSE_DRAG, "mouse-vt200"),
    (TermMode::MOUSE_MOTION, "mouse-btn-event"),
    (TermMode::SGR_MOUSE, "mouse-sgr-1006"),
];

/// 2 つの `TermMode` の差分を計算する。
pub fn diff_mode(before: TermMode, after: TermMode) -> ModeChange {
    let mut added = Vec::new();
    let mut removed = Vec::new();
    for &(flag, name) in TRACKED_MODES {
        let was = before.contains(flag);
        let now = after.contains(flag);
        if !was && now {
            added.push(name.to_string());
        } else if was && !now {
            removed.push(name.to_string());
        }
    }
    ModeChange { added, removed }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_change_yields_empty() {
        let m = TermMode::ALT_SCREEN;
        assert!(diff_mode(m, m).is_empty());
    }

    #[test]
    fn alt_screen_enter_detected() {
        let change = diff_mode(TermMode::empty(), TermMode::ALT_SCREEN);
        assert_eq!(change.added, vec!["alt-screen".to_string()]);
        assert!(change.removed.is_empty());
    }

    #[test]
    fn alt_screen_exit_detected() {
        let change = diff_mode(TermMode::ALT_SCREEN, TermMode::empty());
        assert!(change.added.is_empty());
        assert_eq!(change.removed, vec!["alt-screen".to_string()]);
    }

    #[test]
    fn multiple_modes_change_simultaneously() {
        let before = TermMode::ALT_SCREEN;
        let after = TermMode::BRACKETED_PASTE | TermMode::SGR_MOUSE;
        let change = diff_mode(before, after);
        assert_eq!(change.removed, vec!["alt-screen".to_string()]);
        let mut added_sorted = change.added.clone();
        added_sorted.sort();
        assert_eq!(
            added_sorted,
            vec!["bracketed-paste".to_string(), "mouse-sgr-1006".to_string()]
        );
    }
}
