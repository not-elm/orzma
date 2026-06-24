//! Resolves configured shortcut chords (logical keys) into physical
//! `KeyCode`-based entries the runtime input dispatcher matches against.
//! The translation lives here (not in `ozmux_configs`) so the config crate
//! stays free of any `bevy` dependency.

use crate::configs::OzmuxConfigsResource;
use bevy::prelude::*;
use ozma_terminal::{FineModifier, OzmaMouseConfig, ReservedChord, TerminalInputBindings};
use ozma_tty_engine::{ButtonConfig, WheelConfig};
use ozmux_configs::mouse::{FineModifier as CfgFineModifier, MouseConfig};
use ozmux_configs::shortcuts::{
    Bindings, CommandOverrides, Key as ConfigKey, Modifiers, ShortcutAction,
};
use std::time::Duration;

/// One configured shortcut resolved to a physical key: the `KeyCode` to match,
/// the exact modifier set required, and the action to run.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedShortcut {
    keycode: KeyCode,
    modifiers: Modifiers,
    action: ShortcutAction,
}

/// One configured command override resolved to a physical key: the `KeyCode` to
/// match, the exact modifier set required, and the tmux command to run.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedCommand {
    keycode: KeyCode,
    modifiers: Modifiers,
    command: String,
}

/// The startup-resolved ozmux shortcut tables. Built once from
/// `OzmuxConfigsResource`; consumed by the tmux keyboard dispatcher.
/// `actions` are the built-in GUI actions; `commands` are the tmux-command
/// overrides (`[shortcuts.commands]`).
#[derive(Resource, Default, Debug, Clone)]
pub(crate) struct ResolvedShortcuts {
    actions: Vec<ResolvedShortcut>,
    commands: Vec<ResolvedCommand>,
}

impl ResolvedShortcuts {
    /// Returns the GUI action bound to `(keycode, mods)`, if any. Excludes
    /// `ReleaseWebviewFocus`, which is meaningful only while a webview
    /// holds focus and is matched separately via `is_release_webview_focus`.
    pub(crate) fn match_gui_action(
        &self,
        keycode: KeyCode,
        mods: Modifiers,
    ) -> Option<ShortcutAction> {
        self.actions
            .iter()
            .find(|s| {
                s.action != ShortcutAction::ReleaseWebviewFocus
                    && s.keycode == keycode
                    && s.modifiers == mods
            })
            .map(|s| s.action)
    }

    /// True when `(keycode, mods)` matches the configured release-webview-focus
    /// chord.
    pub(crate) fn is_release_webview_focus(&self, keycode: KeyCode, mods: Modifiers) -> bool {
        self.actions.iter().any(|s| {
            s.action == ShortcutAction::ReleaseWebviewFocus
                && s.keycode == keycode
                && s.modifiers == mods
        })
    }

    /// Returns the tmux command bound to `(keycode, mods)` as a command
    /// override, if any. Exact modifier equality, consistent with
    /// `match_gui_action`.
    pub(crate) fn match_command(&self, keycode: KeyCode, mods: Modifiers) -> Option<&str> {
        self.commands
            .iter()
            .find(|c| c.keycode == keycode && c.modifiers == mods)
            .map(|c| c.command.as_str())
    }

    /// Derives the crate's `TerminalInputBindings` from the resolved table:
    /// the Paste chord becomes `paste`; every other resolved chord becomes a
    /// `reserved` entry the crate dispatcher skips for the host to handle.
    pub(crate) fn input_bindings(&self) -> TerminalInputBindings {
        let mut paste = None;
        let mut reserved = Vec::new();
        for s in &self.actions {
            let chord = ReservedChord {
                key_code: s.keycode,
                ctrl: s.modifiers.ctrl,
                shift: s.modifiers.shift,
                alt: s.modifiers.alt,
                meta: s.modifiers.meta,
            };
            if s.action == ShortcutAction::Paste {
                paste = Some(chord);
            } else {
                reserved.push(chord);
            }
        }
        TerminalInputBindings {
            paste: paste.unwrap_or_else(|| TerminalInputBindings::default().paste),
            reserved,
        }
    }
}

/// Resolves every bound chord in `bindings` to a `ResolvedShortcut`, skipping
/// (with a warning) any chord whose logical key has no physical `KeyCode`.
fn resolve_from_bindings(bindings: &Bindings) -> Vec<ResolvedShortcut> {
    let mut out = Vec::new();
    for (label, bound, action) in bindings.iter() {
        let Some(chord) = bound else { continue };
        match key_to_keycode(&chord.key) {
            Some(keycode) => out.push(ResolvedShortcut {
                keycode,
                modifiers: chord.modifiers,
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

/// Resolves every override chord in `commands` to a `ResolvedCommand`, skipping
/// (with a warning) any chord whose logical key has no physical `KeyCode`.
fn resolve_from_commands(commands: &CommandOverrides) -> Vec<ResolvedCommand> {
    let mut out = Vec::new();
    for (chord, command) in commands.iter() {
        match key_to_keycode(&chord.key) {
            Some(keycode) => out.push(ResolvedCommand {
                keycode,
                modifiers: chord.modifiers,
                command: command.to_string(),
            }),
            None => tracing::warn!(
                chord = %chord,
                command,
                "command-override key has no physical KeyCode mapping; ignoring"
            ),
        }
    }
    out
}

/// `Startup` system: resolves the configured shortcut bindings into
/// `ResolvedShortcuts`, replacing the empty default inserted at plugin build.
///
/// Writes through `ResMut` (an immediate change, unlike a deferred
/// `Commands::insert_resource`) so the table is populated the moment this
/// system runs, with no window in which a same-schedule reader could observe
/// the empty default.
pub(crate) fn build_resolved_shortcuts(
    mut resolved: ResMut<ResolvedShortcuts>,
    configs: Res<OzmuxConfigsResource>,
) {
    *resolved = ResolvedShortcuts {
        actions: resolve_from_bindings(&configs.shortcuts.bindings),
        commands: resolve_from_commands(&configs.shortcuts.commands),
    };
}

/// `Startup` system: inserts `TerminalInputBindings` derived from the resolved
/// shortcut table, replacing the crate default. Runs after
/// `build_resolved_shortcuts`.
pub(crate) fn populate_input_bindings(mut commands: Commands, resolved: Res<ResolvedShortcuts>) {
    commands.insert_resource(resolved.input_bindings());
}

/// Maps the resolved `[mouse]` config block to the terminal crate's
/// `OzmaMouseConfig`.
fn ozma_mouse_config(mc: &MouseConfig) -> OzmaMouseConfig {
    OzmaMouseConfig {
        buttons: ButtonConfig {
            max_protocol_events_per_frame: mc.max_protocol_events_per_frame,
        },
        wheel: WheelConfig {
            lines_per_notch: mc.lines_per_notch,
            fine_lines: mc.fine_lines,
            max_protocol_events_per_frame: mc.max_protocol_events_per_frame,
        },
        cells_per_notch: mc.cells_per_notch,
        double_click_timeout: Duration::from_millis(mc.double_click_timeout_ms as u64),
        click_drift_px: mc.click_drift_px,
        fine_modifier: match mc.fine_modifier {
            CfgFineModifier::Shift => FineModifier::Shift,
            CfgFineModifier::Ctrl => FineModifier::Ctrl,
            CfgFineModifier::Alt => FineModifier::Alt,
            CfgFineModifier::None => FineModifier::None,
        },
    }
}

/// `Startup` system: inserts `OzmaMouseConfig` from the resolved `[mouse]` block.
pub(crate) fn populate_mouse_config(mut commands: Commands, configs: Res<OzmuxConfigsResource>) {
    commands.insert_resource(ozma_mouse_config(&configs.mouse));
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
    use ozmux_configs::shortcuts::{CommandOverrides as ConfigCommandOverrides, parse_key_chord};

    fn from_bindings(b: &Bindings) -> ResolvedShortcuts {
        ResolvedShortcuts {
            actions: resolve_from_bindings(b),
            commands: Vec::new(),
        }
    }

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
    fn default_bindings_resolve_to_five() {
        let r = from_bindings(&Bindings::default());
        assert_eq!(r.actions.len(), 5);
    }

    #[test]
    fn match_gui_action_resolves_defaults() {
        let r = from_bindings(&Bindings::default());
        assert_eq!(
            r.match_gui_action(KeyCode::KeyV, mods(false, false, false, true)),
            Some(ShortcutAction::Paste)
        );
        assert_eq!(
            r.match_gui_action(KeyCode::KeyQ, mods(false, false, false, true)),
            Some(ShortcutAction::Quit)
        );
        assert_eq!(
            r.match_gui_action(KeyCode::KeyS, mods(false, false, false, true)),
            Some(ShortcutAction::EnterCopyMode)
        );
    }

    #[test]
    fn match_gui_action_requires_exact_modifiers() {
        let r = from_bindings(&Bindings::default());
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
    fn match_gui_action_excludes_release_webview_focus() {
        let r = from_bindings(&Bindings::default());
        assert_eq!(
            r.match_gui_action(KeyCode::Escape, mods(true, true, false, false)),
            None
        );
    }

    #[test]
    fn unmatched_chord_is_none() {
        let r = from_bindings(&Bindings::default());
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
    fn is_release_webview_focus_matches_default_chord() {
        let r = from_bindings(&Bindings::default());
        assert!(r.is_release_webview_focus(KeyCode::Escape, mods(true, true, false, false)));
        assert!(!r.is_release_webview_focus(KeyCode::KeyV, mods(false, false, false, true)));
    }

    #[test]
    fn mouse_config_maps_from_ozmux_config() {
        use ozmux_configs::mouse::{FineModifier as CfgFine, MouseConfig};
        let mc = MouseConfig {
            fine_modifier: CfgFine::Ctrl,
            max_protocol_events_per_frame: 5,
            cells_per_notch: 1.0,
            ..MouseConfig::default()
        };
        let out = ozma_mouse_config(&mc);
        assert_eq!(out.buttons.max_protocol_events_per_frame, 5);
        assert_eq!(out.wheel.max_protocol_events_per_frame, 5);
        assert_eq!(out.wheel.lines_per_notch, mc.lines_per_notch);
        assert_eq!(out.cells_per_notch, 1.0);
        assert_eq!(out.fine_modifier, ozma_terminal::FineModifier::Ctrl);
        assert_eq!(
            out.double_click_timeout,
            std::time::Duration::from_millis(mc.double_click_timeout_ms as u64)
        );
        assert_eq!(out.click_drift_px, mc.click_drift_px);
    }

    #[test]
    fn input_bindings_excludes_paste_from_reserved() {
        let r = from_bindings(&Bindings::default());
        let b = r.input_bindings();
        assert_eq!(b.paste.key_code, KeyCode::KeyV);
        assert!(b.paste.meta && !b.paste.ctrl && !b.paste.shift && !b.paste.alt);
        assert_eq!(
            b.reserved.len(),
            4,
            "Quit, ReleaseWebviewFocus, DetachSession, EnterCopyMode"
        );
        assert!(
            !b.reserved
                .iter()
                .any(|c| c.key_code == KeyCode::KeyV && c.meta),
            "the paste chord must not appear in reserved",
        );
    }

    #[test]
    fn resolve_from_commands_maps_chord_to_command() {
        let commands: ConfigCommandOverrides = [(
            parse_key_chord("Cmd+D").unwrap(),
            "split-window -h".to_string(),
        )]
        .into_iter()
        .collect();
        let resolved = resolve_from_commands(&commands);
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].keycode, KeyCode::KeyD);
        assert!(resolved[0].modifiers.meta);
        assert_eq!(resolved[0].command, "split-window -h");
    }

    #[test]
    fn resolve_from_commands_skips_unmappable_key() {
        let commands: ConfigCommandOverrides =
            [(parse_key_chord("Cmd+Plus").unwrap(), "x".to_string())]
                .into_iter()
                .collect();
        assert!(resolve_from_commands(&commands).is_empty());
    }

    #[test]
    fn match_command_requires_exact_modifiers() {
        let commands: ConfigCommandOverrides = [(
            parse_key_chord("Alt+I").unwrap(),
            "split-window -h".to_string(),
        )]
        .into_iter()
        .collect();
        let r = ResolvedShortcuts {
            actions: Vec::new(),
            commands: resolve_from_commands(&commands),
        };
        assert_eq!(
            r.match_command(KeyCode::KeyI, mods(false, false, true, false)),
            Some("split-window -h")
        );
        assert_eq!(
            r.match_command(KeyCode::KeyI, mods(true, false, true, false)),
            None
        );
        assert_eq!(
            r.match_command(KeyCode::KeyI, mods(false, false, false, false)),
            None
        );
    }
}
