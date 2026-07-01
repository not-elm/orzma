//! Resolves configured shortcut chords (logical keys) into physical
//! `KeyCode`-based entries the runtime input dispatcher matches against.
//! The translation lives here (not in `ozmux_configs`) so the config crate
//! stays free of any `bevy` dependency.

use crate::configs::OzmuxConfigsResource;
use crate::input::bindings::{FineModifier, OzmaMouseConfig, ReservedChord, TerminalInputBindings};
use crate::mode::AppMode;
use bevy::prelude::*;
use bevy_cef::prelude::FocusedWebview;
use ozma_tty_engine::{ButtonConfig, WheelConfig};
use ozmux_configs::mouse::{FineModifier as CfgFineModifier, MouseConfig};
use ozmux_configs::shortcuts::{Key as ConfigKey, KeyChord, Modifiers, ShortcutAction};
use std::time::Duration;

pub(super) struct ShortcutsPlugin;

impl Plugin for ShortcutsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Shortcuts>()
            .init_resource::<LeaderPending>()
            .configure_sets(Update, LeaderGate::Read.before(LeaderGate::Advance))
            .add_systems(
                Startup,
                (
                    build_shortcuts,
                    populate_input_bindings,
                    populate_mouse_config,
                )
                    .chain(),
            )
            .add_systems(OnExit(AppMode::Tmux), reset_leader_pending)
            .add_systems(OnExit(AppMode::Default), reset_leader_pending)
            .add_systems(
                Update,
                reset_leader_on_webview_focus_change
                    .run_if(resource_exists_and_changed::<FocusedWebview>),
            );
    }
}

/// Shared leader-pending flag: `true` between a leader chord and its second key.
/// Owned by `ShortcutsPlugin`; advanced by both the tmux and Default keyboard
/// dispatchers and read by `dispatch_input` to suppress PTY typing mid-sequence.
#[derive(Resource, Default)]
pub(crate) struct LeaderPending(
    /// `true` while a leader chord is pending its second key.
    pub(crate) bool,
);

/// Orders the two `FocusedKey` systems that touch `LeaderPending` so
/// `dispatch_input` (`Read`) observes the end-of-previous-frame value before
/// `app_shortcut_handler` (`Advance`) steps the leader machine and clears it.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum LeaderGate {
    /// `dispatch_input`: reads `LeaderPending` to gate PTY typing.
    Read,
    /// `app_shortcut_handler`: advances the leader state machine.
    Advance,
}

/// One configured shortcut resolved to a physical key: the `KeyCode` to match,
/// the exact modifier set required, and the action to run.
#[derive(Debug, Clone, PartialEq, Eq)]
struct OzmuxShortcut {
    keycode: KeyCode,
    modifiers: Modifiers,
    action: ShortcutAction,
}

/// The startup-resolved ozmux shortcut tables. Built once from
/// `OzmuxConfigsResource`; consumed by the tmux and Default keyboard
/// dispatchers.
#[derive(Resource, Default, Debug, Clone)]
pub(crate) struct Shortcuts {
    direct: Vec<OzmuxShortcut>,
    prefix: Vec<OzmuxShortcut>,
    leader: Option<(KeyCode, Modifiers)>,
}

impl Shortcuts {
    /// Returns the GUI action bound to `(keycode, mods)` in the direct table, if
    /// any. Excludes `ReleaseWebviewFocus` (matched via `is_release_webview_focus`).
    pub(crate) fn match_gui_action(
        &self,
        keycode: KeyCode,
        mods: Modifiers,
    ) -> Option<ShortcutAction> {
        self.direct
            .iter()
            .find(|s| {
                s.action != ShortcutAction::ReleaseWebviewFocus
                    && s.keycode == keycode
                    && s.modifiers == mods
            })
            .map(|s| s.action)
    }

    /// True when `(keycode, mods)` matches the configured release-webview-focus
    /// chord in the direct table.
    pub(crate) fn is_release_webview_focus(&self, keycode: KeyCode, mods: Modifiers) -> bool {
        self.direct.iter().any(|s| {
            s.action == ShortcutAction::ReleaseWebviewFocus
                && s.keycode == keycode
                && s.modifiers == mods
        })
    }

    /// Derives the crate's `TerminalInputBindings` from the direct table: the
    /// Paste chord becomes `paste`; every other direct chord — plus the leader
    /// chord — becomes a `reserved` entry the crate dispatcher skips for the
    /// host to handle. Reserving the leader keeps `dispatch_input` from typing
    /// it into the PTY while the leader engages.
    pub(crate) fn input_bindings(&self) -> TerminalInputBindings {
        let mut paste = None;
        let mut reserved = Vec::new();
        for s in &self.direct {
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
        if let Some((keycode, modifiers)) = self.leader {
            reserved.push(ReservedChord {
                key_code: keycode,
                ctrl: modifiers.ctrl,
                shift: modifiers.shift,
                alt: modifiers.alt,
                meta: modifiers.meta,
            });
        }
        TerminalInputBindings {
            paste: paste.unwrap_or_else(|| TerminalInputBindings::default().paste),
            reserved,
        }
    }

    /// True when `(keycode, mods)` is the configured leader chord.
    fn is_leader(&self, keycode: KeyCode, mods: Modifiers) -> bool {
        self.leader == Some((keycode, mods))
    }

    /// Returns the leader-scoped action bound to `(keycode, mods)`, if any.
    /// Excludes `ReleaseWebviewFocus` (mirrors `match_gui_action`): leader
    /// dispatch only runs when no webview is focused, so a leader-scoped
    /// release-webview-focus could never fire — resolving it to `Swallow`
    /// avoids a dead `RunAction`.
    fn match_prefix_action(&self, keycode: KeyCode, mods: Modifiers) -> Option<ShortcutAction> {
        self.prefix
            .iter()
            .find(|s| {
                s.action != ShortcutAction::ReleaseWebviewFocus
                    && s.keycode == keycode
                    && s.modifiers == mods
            })
            .map(|s| s.action)
    }
}

/// Outcome of one `step_leader` call for a single pressed key. `Passthrough`
/// means the key is not leader-related and the caller proceeds with its normal
/// dispatch.
pub(crate) enum LeaderStep {
    /// A leader-scoped binding matched; run this action.
    RunAction(ShortcutAction),
    /// Consume the key with no effect (the leader itself, or an unmatched second
    /// key that abandons the sequence).
    Swallow,
    /// Not leader-related; fall through to the caller's normal dispatch.
    Passthrough,
}

/// Advances the ozmux leader state machine for one pressed key, threading
/// `pending` across frames (mirrors the tmux `plan_forward` prefix dispatch).
/// Swallows the leader itself and any unmatched second key; returns
/// `Passthrough` for unrelated keys.
pub(crate) fn step_leader(
    pending: &mut bool,
    shortcuts: &Shortcuts,
    keycode: KeyCode,
    mods: Modifiers,
) -> LeaderStep {
    // NOTE: a bare modifier press must NOT touch `pending`. The second chord's
    // modifier (e.g. Ctrl) emits its own `Pressed` event ahead of the main key;
    // stepping on it would consume the pending leader by parity and abort the
    // sequence before the real key arrives.
    if is_modifier_key(keycode) {
        return LeaderStep::Passthrough;
    }
    if *pending {
        *pending = false;
        return match shortcuts.match_prefix_action(keycode, mods) {
            Some(action) => LeaderStep::RunAction(action),
            None => LeaderStep::Swallow,
        };
    }
    if shortcuts.is_leader(keycode, mods) {
        *pending = true;
        return LeaderStep::Swallow;
    }
    LeaderStep::Passthrough
}

/// Clears `LeaderPending` on an `AppMode` transition so a leader engaged in one
/// mode never fires its second key after switching modes.
fn reset_leader_pending(mut leader_pending: ResMut<LeaderPending>) {
    leader_pending.0 = false;
}

/// Clears `LeaderPending` whenever webview focus changes. Webview focus moves on
/// mouse clicks (no `KeyboardInput`), so the keyboard dispatchers never see the
/// round-trip; without this a leader engaged before a mouse-only webview
/// focus/blur would consume the next terminal keystroke as its second key.
fn reset_leader_on_webview_focus_change(mut leader_pending: ResMut<LeaderPending>) {
    leader_pending.0 = false;
}

/// `Startup` system: resolves the configured shortcut bindings into
/// `Shortcuts`, replacing the empty default inserted at plugin build.
///
/// Writes through `ResMut` (an immediate change, unlike a deferred
/// `Commands::insert_resource`) so the table is populated the moment this
/// system runs, with no window in which a same-schedule reader could observe
/// the empty default.
fn build_shortcuts(mut resolved: ResMut<Shortcuts>, configs: Res<OzmuxConfigsResource>) {
    let sc = &configs.shortcuts;
    resolved.direct = resolve_from_bindings(sc.bindings.iter());
    resolved.prefix = resolve_from_bindings(sc.prefix_bindings.iter());
    resolved.leader = sc.prefix.as_ref().and_then(|chord| match key_to_keycode(&chord.key) {
        Some(keycode) => Some((keycode, chord.modifiers)),
        None => {
            tracing::warn!(chord = %chord, "shortcut leader key has no physical KeyCode mapping; prefix disabled");
            None
        }
    });
    if resolved.leader.is_none() && !resolved.prefix.is_empty() {
        tracing::warn!(
            "shortcuts.prefix_bindings are set but the leader is unset or unmappable; prefix table is unreachable"
        );
    }
}

/// `Startup` system: inserts `TerminalInputBindings` derived from the resolved
/// shortcut table, replacing the crate default. Runs after
/// `build_shortcuts`.
fn populate_input_bindings(mut commands: Commands, resolved: Res<Shortcuts>) {
    commands.insert_resource(resolved.input_bindings());
}

/// `Startup` system: inserts `OzmaMouseConfig` from the resolved `[mouse]` block.
fn populate_mouse_config(mut commands: Commands, configs: Res<OzmuxConfigsResource>) {
    commands.insert_resource(ozma_mouse_config(&configs.mouse));
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
        axis_lock_ratio: mc.axis_lock_ratio,
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

/// Resolves every bound chord in `bindings` to an `OzmuxShortcut`, skipping
/// (with a warning) any chord whose logical key has no physical `KeyCode`.
fn resolve_from_bindings<'a>(
    bindings: impl Iterator<Item = (&'static str, &'a Option<KeyChord>, ShortcutAction)>,
) -> Vec<OzmuxShortcut> {
    let mut out = Vec::new();
    for (label, bound, action) in bindings {
        let Some(chord) = bound else { continue };
        match key_to_keycode(&chord.key) {
            Some(keycode) => out.push(OzmuxShortcut {
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

/// True for the bare left/right modifier keys, which emit their own `Pressed`
/// events ahead of a chord's main key.
pub(crate) fn is_modifier_key(keycode: KeyCode) -> bool {
    matches!(
        keycode,
        KeyCode::ControlLeft
            | KeyCode::ControlRight
            | KeyCode::ShiftLeft
            | KeyCode::ShiftRight
            | KeyCode::AltLeft
            | KeyCode::AltRight
            | KeyCode::SuperLeft
            | KeyCode::SuperRight
    )
}

/// Maps a config logical `Key` to the physical `KeyCode` ozmux matches on.
/// Returns `None` for keys with no stable physical mapping (`Plus`, `Other`,
/// non-alphanumeric chars).
fn key_to_keycode(key: &ConfigKey) -> Option<KeyCode> {
    // NOTE: keep this accepted domain in lockstep with
    // `ozmux_configs::shortcuts::Key::maps_to_physical_key`; a divergence lets
    // an unmappable leader pass config validation yet resolve to no `KeyCode`,
    // silently disabling the whole prefix table.
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
    use ozmux_configs::shortcuts::Bindings;

    fn mods(ctrl: bool, shift: bool, alt: bool, meta: bool) -> Modifiers {
        Modifiers {
            ctrl,
            shift,
            alt,
            meta,
        }
    }

    fn direct_only(bindings: &Bindings) -> Shortcuts {
        Shortcuts {
            direct: resolve_from_bindings(bindings.iter()),
            prefix: Vec::new(),
            leader: None,
        }
    }

    #[test]
    fn leader_resolves_from_config_chord() {
        let s = Shortcuts {
            direct: Vec::new(),
            prefix: Vec::new(),
            leader: Some((KeyCode::KeyA, mods(true, false, false, false))),
        };
        assert!(s.is_leader(KeyCode::KeyA, mods(true, false, false, false)));
        assert!(!s.is_leader(KeyCode::KeyA, mods(false, false, false, false)));
        assert!(!s.is_leader(KeyCode::KeyB, mods(true, false, false, false)));
    }

    #[test]
    fn match_prefix_action_excludes_release_webview_focus() {
        let s = Shortcuts {
            direct: Vec::new(),
            prefix: vec![OzmuxShortcut {
                keycode: KeyCode::KeyR,
                modifiers: mods(false, false, false, false),
                action: ShortcutAction::ReleaseWebviewFocus,
            }],
            leader: Some((KeyCode::KeyA, mods(true, false, false, false))),
        };
        assert_eq!(
            s.match_prefix_action(KeyCode::KeyR, mods(false, false, false, false)),
            None,
            "a leader-scoped release-webview-focus resolves to Swallow, not a dead RunAction",
        );
    }

    #[test]
    fn input_bindings_reserves_the_leader_chord() {
        let mut s = direct_only(&Bindings::default());
        s.leader = Some((KeyCode::KeyA, mods(true, false, false, false)));
        let b = s.input_bindings();
        assert!(
            b.reserved.iter().any(|c| c.key_code == KeyCode::KeyA
                && c.ctrl
                && !c.shift
                && !c.alt
                && !c.meta),
            "the leader chord must be reserved so dispatch_input never types it into the PTY",
        );
    }

    #[test]
    fn step_leader_ignores_bare_modifier_and_survives_to_second_chord() {
        // Reproduces [0]: the second chord's Ctrl modifier emits its own Pressed
        // event before KeyD; it must not consume `pending`. Leader Ctrl+B,
        // prefix detach-session = Ctrl+D.
        let sc = Shortcuts {
            direct: Vec::new(),
            prefix: vec![OzmuxShortcut {
                keycode: KeyCode::KeyD,
                modifiers: mods(true, false, false, false),
                action: ShortcutAction::DetachSession,
            }],
            leader: Some((KeyCode::KeyB, mods(true, false, false, false))),
        };
        let mut pending = false;
        assert!(matches!(
            step_leader(
                &mut pending,
                &sc,
                KeyCode::ControlLeft,
                mods(true, false, false, false)
            ),
            LeaderStep::Passthrough
        ));
        assert!(!pending, "a bare modifier must not engage the leader");
        assert!(matches!(
            step_leader(
                &mut pending,
                &sc,
                KeyCode::KeyB,
                mods(true, false, false, false)
            ),
            LeaderStep::Swallow
        ));
        assert!(pending, "the leader chord engages pending");
        assert!(matches!(
            step_leader(
                &mut pending,
                &sc,
                KeyCode::ControlLeft,
                mods(true, false, false, false)
            ),
            LeaderStep::Passthrough
        ));
        assert!(
            pending,
            "a bare modifier must NOT clear pending mid-sequence"
        );
        assert!(matches!(
            step_leader(
                &mut pending,
                &sc,
                KeyCode::KeyD,
                mods(true, false, false, false)
            ),
            LeaderStep::RunAction(ShortcutAction::DetachSession)
        ));
        assert!(!pending);
    }

    #[test]
    fn match_prefix_action_resolves_and_requires_exact_mods() {
        let s = Shortcuts {
            direct: Vec::new(),
            prefix: vec![OzmuxShortcut {
                keycode: KeyCode::KeyS,
                modifiers: mods(false, false, false, false),
                action: ShortcutAction::EnterCopyMode,
            }],
            leader: Some((KeyCode::KeyA, mods(true, false, false, false))),
        };
        assert_eq!(
            s.match_prefix_action(KeyCode::KeyS, mods(false, false, false, false)),
            Some(ShortcutAction::EnterCopyMode)
        );
        assert_eq!(
            s.match_prefix_action(KeyCode::KeyS, mods(false, true, false, false)),
            None
        );
        assert_eq!(
            s.match_prefix_action(KeyCode::KeyD, mods(false, false, false, false)),
            None
        );
    }

    #[test]
    fn resolve_from_bindings_accepts_prefix_bindings_iter() {
        use ozmux_configs::shortcuts::PrefixBindings;
        let p = PrefixBindings {
            detach_session: Some(ozmux_configs::shortcuts::parse_key_chord("d").unwrap()),
            ..Default::default()
        };
        let resolved = resolve_from_bindings(p.iter());
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].keycode, KeyCode::KeyD);
        assert_eq!(resolved[0].action, ShortcutAction::DetachSession);
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
        let r = direct_only(&Bindings::default());
        assert_eq!(r.direct.len(), 5);
    }

    #[test]
    fn match_gui_action_resolves_defaults() {
        let r = direct_only(&Bindings::default());
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
        let r = direct_only(&Bindings::default());
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
        let r = direct_only(&Bindings::default());
        assert_eq!(
            r.match_gui_action(KeyCode::Escape, mods(true, true, false, false)),
            None
        );
    }

    #[test]
    fn unmatched_chord_is_none() {
        let r = direct_only(&Bindings::default());
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
        let r = direct_only(&Bindings::default());
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
            axis_lock_ratio: 0.5,
            ..MouseConfig::default()
        };
        let out = ozma_mouse_config(&mc);
        assert_eq!(out.buttons.max_protocol_events_per_frame, 5);
        assert_eq!(out.wheel.max_protocol_events_per_frame, 5);
        assert_eq!(out.wheel.lines_per_notch, mc.lines_per_notch);
        assert_eq!(out.cells_per_notch, 1.0);
        assert_eq!(
            out.axis_lock_ratio, 0.5,
            "non-default value must flow through"
        );
        assert_eq!(out.fine_modifier, FineModifier::Ctrl);
        assert_eq!(
            out.double_click_timeout,
            std::time::Duration::from_millis(mc.double_click_timeout_ms as u64)
        );
        assert_eq!(out.click_drift_px, mc.click_drift_px);
    }

    #[test]
    fn input_bindings_excludes_paste_from_reserved() {
        let r = direct_only(&Bindings::default());
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

    fn leader_fixture() -> Shortcuts {
        Shortcuts {
            direct: Vec::new(),
            prefix: vec![OzmuxShortcut {
                keycode: KeyCode::KeyS,
                modifiers: mods(false, false, false, false),
                action: ShortcutAction::EnterCopyMode,
            }],
            leader: Some((KeyCode::KeyA, mods(true, false, false, false))),
        }
    }

    #[test]
    fn leader_press_sets_pending_and_swallows() {
        let sc = leader_fixture();
        let mut pending = false;
        let step = step_leader(
            &mut pending,
            &sc,
            KeyCode::KeyA,
            mods(true, false, false, false),
        );
        assert!(matches!(step, LeaderStep::Swallow));
        assert!(pending);
    }

    #[test]
    fn pending_plus_bound_key_runs_action_and_clears_pending() {
        let sc = leader_fixture();
        let mut pending = true;
        let step = step_leader(
            &mut pending,
            &sc,
            KeyCode::KeyS,
            mods(false, false, false, false),
        );
        assert!(matches!(
            step,
            LeaderStep::RunAction(ShortcutAction::EnterCopyMode)
        ));
        assert!(!pending);
    }

    #[test]
    fn pending_plus_unbound_key_swallows_and_clears_pending() {
        let sc = leader_fixture();
        let mut pending = true;
        let step = step_leader(
            &mut pending,
            &sc,
            KeyCode::KeyZ,
            mods(false, false, false, false),
        );
        assert!(matches!(step, LeaderStep::Swallow));
        assert!(!pending);
    }

    #[test]
    fn unrelated_key_passes_through() {
        let sc = leader_fixture();
        let mut pending = false;
        let step = step_leader(
            &mut pending,
            &sc,
            KeyCode::KeyB,
            mods(false, false, false, false),
        );
        assert!(matches!(step, LeaderStep::Passthrough));
        assert!(!pending);
    }

    #[test]
    fn sequential_leader_then_bound_key_threads_pending() {
        // NOTE: the dispatch loop calls step_leader per event with one shared
        // `pending` local; this verifies the leader press then the bound key
        // thread that state correctly across two calls.
        let sc = leader_fixture();
        let mut pending = false;
        let first = step_leader(
            &mut pending,
            &sc,
            KeyCode::KeyA,
            mods(true, false, false, false),
        );
        assert!(matches!(first, LeaderStep::Swallow));
        let second = step_leader(
            &mut pending,
            &sc,
            KeyCode::KeyS,
            mods(false, false, false, false),
        );
        assert!(matches!(
            second,
            LeaderStep::RunAction(ShortcutAction::EnterCopyMode)
        ));
        assert!(!pending);
    }
}
