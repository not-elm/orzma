//! `Term::mode()` before/after diff helper.
//!
//! Compares `TermMode` bitflags and produces the add/remove lists of
//! wire mode strings (alt-screen, bracketed-paste, mouse-vt200, etc.).

use alacritty_terminal::term::TermMode;

/// Mode flag transition between two Term states.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModeChange {
    /// Mode names that transitioned from unset to set.
    pub added: Vec<&'static str>,
    /// Mode names that transitioned from set to unset.
    pub removed: Vec<&'static str>,
}

impl ModeChange {
    /// Returns true when no flags transitioned.
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty()
    }
}

// NOTE: TermMode constant names follow alacritty_terminal 0.26
// (term/mod.rs bitflags definition). Only wire spec § 4.7 modes are
// tracked here; alacritty-internal flags like LINE_WRAP are
// intentionally excluded.
pub(crate) const TRACKED_MODES: &[(TermMode, &str)] = &[
    (TermMode::ALT_SCREEN, "alt-screen"),
    (TermMode::BRACKETED_PASTE, "bracketed-paste"),
    (TermMode::APP_CURSOR, "app-cursor-keys"),
    (TermMode::FOCUS_IN_OUT, "focus-events"),
    (TermMode::MOUSE_REPORT_CLICK, "mouse-vt200"),
    (TermMode::MOUSE_DRAG, "mouse-btn-event"),
    (TermMode::MOUSE_MOTION, "mouse-any-event"),
    (TermMode::SGR_MOUSE, "mouse-sgr-1006"),
    (TermMode::ALTERNATE_SCROLL, "alternate-scroll"),
];

/// Reconstructs the `TermMode` bitset from the mode NAMES carried on the wire
/// (`FrameSnapshot.modes` / `VtEvent::ModeChanged`). Inverse of `diff_mode`'s
/// naming. Unknown names are ignored.
pub fn modes_from_names(names: &[String]) -> TermMode {
    let mut modes = TermMode::empty();
    for name in names {
        if let Some(&(flag, _)) = TRACKED_MODES.iter().find(|&&(_, n)| n == name) {
            modes.insert(flag);
        }
    }
    modes
}

/// Computes the transition between two `TermMode` snapshots.
pub fn diff_mode(before: TermMode, after: TermMode) -> ModeChange {
    let mut added = Vec::new();
    let mut removed = Vec::new();
    for &(flag, name) in TRACKED_MODES {
        let was = before.contains(flag);
        let now = after.contains(flag);
        if !was && now {
            added.push(name);
        } else if was && !now {
            removed.push(name);
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
        assert_eq!(change.added, vec!["alt-screen"]);
        assert!(change.removed.is_empty());
    }

    #[test]
    fn alt_screen_exit_detected() {
        let change = diff_mode(TermMode::ALT_SCREEN, TermMode::empty());
        assert!(change.added.is_empty());
        assert_eq!(change.removed, vec!["alt-screen"]);
    }

    #[test]
    fn multiple_modes_change_simultaneously() {
        let before = TermMode::ALT_SCREEN;
        let after = TermMode::BRACKETED_PASTE | TermMode::SGR_MOUSE;
        let change = diff_mode(before, after);
        assert_eq!(change.removed, vec!["alt-screen"]);
        let mut added_sorted = change.added.clone();
        added_sorted.sort();
        assert_eq!(added_sorted, vec!["bracketed-paste", "mouse-sgr-1006"]);
    }

    #[test]
    fn mouse_mode_names_match_alacritty_decset_mapping() {
        let change = diff_mode(TermMode::empty(), TermMode::MOUSE_REPORT_CLICK);
        assert_eq!(change.added, vec!["mouse-vt200"]);

        let change = diff_mode(TermMode::empty(), TermMode::MOUSE_DRAG);
        assert_eq!(change.added, vec!["mouse-btn-event"]);

        let change = diff_mode(TermMode::empty(), TermMode::MOUSE_MOTION);
        assert_eq!(change.added, vec!["mouse-any-event"]);
    }

    #[test]
    fn modes_from_names_round_trips_every_tracked_mode() {
        for &(flag, name) in TRACKED_MODES {
            let got = modes_from_names(std::slice::from_ref(&name.to_string()));
            assert!(got.contains(flag), "{name} -> {flag:?} must round-trip");
        }
    }

    #[test]
    fn tracked_modes_names_every_router_checked_flag() {
        use alacritty_terminal::term::TermMode;
        // Every flag the GUI routers / encode_key / paste consult must be NAMED,
        // else the thin-client can't reconstruct it from FrameSnapshot.modes.
        for flag in [
            TermMode::APP_CURSOR,
            TermMode::BRACKETED_PASTE,
            TermMode::ALT_SCREEN,
            TermMode::ALTERNATE_SCROLL,
            TermMode::SGR_MOUSE,
            TermMode::MOUSE_REPORT_CLICK,
            TermMode::MOUSE_DRAG,
            TermMode::MOUSE_MOTION,
        ] {
            assert!(
                TRACKED_MODES.iter().any(|&(f, _)| f == flag),
                "{flag:?} must be tracked"
            );
        }
    }
}
