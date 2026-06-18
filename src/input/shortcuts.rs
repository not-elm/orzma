//! Resolves configured shortcut chords (logical keys) into physical
//! `KeyCode`-based entries the runtime input dispatcher matches against.
//! The translation lives here (not in `ozmux_configs`) so the config crate
//! stays free of any `bevy` dependency.

use crate::configs::OzmuxConfigsResource;
use bevy::prelude::*;
use ozmux_configs::shortcuts::{Bindings, Key as ConfigKey, Modifiers, ShortcutAction};

/// One configured shortcut resolved to a physical key: the `KeyCode` to match,
/// the exact modifier set required, and the action to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedShortcut {
    pub(crate) keycode: KeyCode,
    pub(crate) modifiers: Modifiers,
    pub(crate) action: ShortcutAction,
}

/// The startup-resolved ozmux shortcut table. Built once from
/// `OzmuxConfigsResource`; consumed by the tmux keyboard dispatcher.
#[derive(Resource, Default, Debug, Clone)]
pub(crate) struct ResolvedShortcuts(pub(crate) Vec<ResolvedShortcut>);

impl ResolvedShortcuts {
    /// Returns the GUI action bound to `(keycode, mods)`, if any. Excludes
    /// `ReleaseInlineFocus`, which is meaningful only while an inline webview
    /// holds focus and is matched separately via `is_release_inline_focus`.
    pub(crate) fn match_gui_action(
        &self,
        keycode: KeyCode,
        mods: Modifiers,
    ) -> Option<ShortcutAction> {
        self.0
            .iter()
            .filter(|s| s.action != ShortcutAction::ReleaseInlineFocus)
            .find(|s| s.keycode == keycode && s.modifiers == mods)
            .map(|s| s.action.clone())
    }

    /// True when `(keycode, mods)` matches the configured release-inline-focus
    /// chord.
    pub(crate) fn is_release_inline_focus(&self, keycode: KeyCode, mods: Modifiers) -> bool {
        self.0.iter().any(|s| {
            s.action == ShortcutAction::ReleaseInlineFocus
                && s.keycode == keycode
                && s.modifiers == mods
        })
    }
}

/// Resolves every bound chord in `bindings` to a `ResolvedShortcut`, skipping
/// (with a warning) any chord whose logical key has no physical `KeyCode`.
pub(crate) fn resolve_from_bindings(bindings: &Bindings) -> Vec<ResolvedShortcut> {
    let mut out = Vec::new();
    for (label, bound, action) in bindings.iter() {
        let Some(chord) = bound else { continue };
        match key_to_keycode(&chord.key) {
            Some(keycode) => out.push(ResolvedShortcut {
                keycode,
                modifiers: chord.modifiers.clone(),
                action,
            }),
            None => tracing::warn!(
                label,
                chord = %chord,
                "shortcut key has no physical KeyCode mapping; ignoring binding"
            ),
        }
    }
    out
}

/// `Startup` system: resolves the configured shortcut bindings into
/// `ResolvedShortcuts`, replacing the empty default inserted at plugin build.
pub(crate) fn build_resolved_shortcuts(mut commands: Commands, configs: Res<OzmuxConfigsResource>) {
    commands.insert_resource(ResolvedShortcuts(resolve_from_bindings(
        &configs.shortcuts.bindings,
    )));
}

/// Maps a config logical `Key` to the physical `KeyCode` ozmux matches on.
/// Returns `None` for keys with no stable physical mapping (`Plus`, `Other`,
/// non-alphanumeric chars).
fn key_to_keycode(key: &ConfigKey) -> Option<KeyCode> {
    Some(match key {
        ConfigKey::Char(c) => match c.to_ascii_lowercase() {
            'a' => KeyCode::KeyA,
            'b' => KeyCode::KeyB,
            'c' => KeyCode::KeyC,
            'd' => KeyCode::KeyD,
            'e' => KeyCode::KeyE,
            'f' => KeyCode::KeyF,
            'g' => KeyCode::KeyG,
            'h' => KeyCode::KeyH,
            'i' => KeyCode::KeyI,
            'j' => KeyCode::KeyJ,
            'k' => KeyCode::KeyK,
            'l' => KeyCode::KeyL,
            'm' => KeyCode::KeyM,
            'n' => KeyCode::KeyN,
            'o' => KeyCode::KeyO,
            'p' => KeyCode::KeyP,
            'q' => KeyCode::KeyQ,
            'r' => KeyCode::KeyR,
            's' => KeyCode::KeyS,
            't' => KeyCode::KeyT,
            'u' => KeyCode::KeyU,
            'v' => KeyCode::KeyV,
            'w' => KeyCode::KeyW,
            'x' => KeyCode::KeyX,
            'y' => KeyCode::KeyY,
            'z' => KeyCode::KeyZ,
            '0' => KeyCode::Digit0,
            '1' => KeyCode::Digit1,
            '2' => KeyCode::Digit2,
            '3' => KeyCode::Digit3,
            '4' => KeyCode::Digit4,
            '5' => KeyCode::Digit5,
            '6' => KeyCode::Digit6,
            '7' => KeyCode::Digit7,
            '8' => KeyCode::Digit8,
            '9' => KeyCode::Digit9,
            _ => return None,
        },
        ConfigKey::Escape => KeyCode::Escape,
        ConfigKey::Space => KeyCode::Space,
        ConfigKey::Enter => KeyCode::Enter,
        ConfigKey::Tab => KeyCode::Tab,
        ConfigKey::Backspace => KeyCode::Backspace,
        ConfigKey::ArrowUp => KeyCode::ArrowUp,
        ConfigKey::ArrowDown => KeyCode::ArrowDown,
        ConfigKey::ArrowLeft => KeyCode::ArrowLeft,
        ConfigKey::ArrowRight => KeyCode::ArrowRight,
        ConfigKey::Plus => return None,
        ConfigKey::Other(_) => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mods(ctrl: bool, shift: bool, alt: bool, meta: bool) -> Modifiers {
        Modifiers {
            ctrl,
            shift,
            alt,
            meta,
        }
    }

    #[test]
    fn char_letter_maps_to_keycode_case_insensitive() {
        assert_eq!(key_to_keycode(&ConfigKey::Char('v')), Some(KeyCode::KeyV));
        assert_eq!(key_to_keycode(&ConfigKey::Char('P')), Some(KeyCode::KeyP));
    }

    #[test]
    fn digit_maps_to_keycode() {
        assert_eq!(key_to_keycode(&ConfigKey::Char('1')), Some(KeyCode::Digit1));
    }

    #[test]
    fn named_keys_map() {
        assert_eq!(key_to_keycode(&ConfigKey::Escape), Some(KeyCode::Escape));
        assert_eq!(key_to_keycode(&ConfigKey::ArrowUp), Some(KeyCode::ArrowUp));
    }

    #[test]
    fn unmappable_keys_are_none() {
        assert_eq!(key_to_keycode(&ConfigKey::Plus), None);
        assert_eq!(key_to_keycode(&ConfigKey::Other("f12".into())), None);
    }

    #[test]
    fn default_bindings_resolve_to_four() {
        let r = ResolvedShortcuts(resolve_from_bindings(&Bindings::default()));
        assert_eq!(r.0.len(), 4);
    }

    #[test]
    fn match_gui_action_resolves_defaults() {
        let r = ResolvedShortcuts(resolve_from_bindings(&Bindings::default()));
        assert_eq!(
            r.match_gui_action(KeyCode::KeyV, mods(false, false, false, true)),
            Some(ShortcutAction::Paste)
        );
        assert_eq!(
            r.match_gui_action(KeyCode::KeyP, mods(false, true, false, true)),
            Some(ShortcutAction::OpenPicker)
        );
        assert_eq!(
            r.match_gui_action(KeyCode::KeyQ, mods(false, false, false, true)),
            Some(ShortcutAction::Quit)
        );
    }

    #[test]
    fn match_gui_action_requires_exact_modifiers() {
        let r = ResolvedShortcuts(resolve_from_bindings(&Bindings::default()));
        assert_eq!(
            r.match_gui_action(KeyCode::KeyV, mods(false, true, false, true)),
            None
        );
        assert_eq!(
            r.match_gui_action(KeyCode::KeyQ, mods(false, true, false, true)),
            None
        );
    }

    #[test]
    fn match_gui_action_excludes_release_inline_focus() {
        let r = ResolvedShortcuts(resolve_from_bindings(&Bindings::default()));
        assert_eq!(
            r.match_gui_action(KeyCode::Escape, mods(true, true, false, false)),
            None
        );
    }

    #[test]
    fn unmatched_chord_is_none() {
        let r = ResolvedShortcuts(resolve_from_bindings(&Bindings::default()));
        assert_eq!(
            r.match_gui_action(KeyCode::KeyH, mods(false, false, false, true)),
            None
        );
        assert_eq!(
            r.match_gui_action(KeyCode::KeyA, mods(false, false, false, false)),
            None
        );
    }

    #[test]
    fn is_release_inline_focus_matches_default_chord() {
        let r = ResolvedShortcuts(resolve_from_bindings(&Bindings::default()));
        assert!(r.is_release_inline_focus(KeyCode::Escape, mods(true, true, false, false)));
        assert!(!r.is_release_inline_focus(KeyCode::KeyV, mods(false, false, false, true)));
    }
}
