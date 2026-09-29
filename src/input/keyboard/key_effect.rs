//! Pure decision layer for keyboard-shortcut dispatch: decides each
//! pressed key's effect, with no ECS handles.

use crate::action::vi::ResolvedViModeKeys;
use crate::input::keyboard::held_modifiers::{AltPolicy, HeldModifiers};
use crate::input::shortcuts::{
    LeaderPhase, LeaderStep, Shortcuts, is_modifier_key, refire_held_repeat, step_leader,
};
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyCode, KeyboardInput};
use bevy_orzma_webview::{ChordKey, NormalizedChord};
use orzma_configs::shortcuts::{Modifiers, Shortcut};
use orzma_configs::vi_mode::ViModeAction;
use std::time::Duration;

/// One decided effect of a single pressed key. The appliers interpret
/// each variant; this type carries no ECS handles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum KeyEffect {
    /// Run a bound `Shortcut`. `via_leader` distinguishes a leader-scoped
    /// firing from a direct GUI chord — appliers suppress a different subset
    /// of each (e.g. a leader `Paste` fires in vi mode, a direct `Paste`
    /// does not).
    Shortcut {
        /// The action to run.
        action: Shortcut,
        /// Whether the action was reached through the leader (prefix table)
        /// rather than a direct chord.
        via_leader: bool,
    },
    /// Run a matched `[vi-mode]` key.
    ViMode(ViModeAction),
    /// Type the key into the focused terminal's PTY directly.
    ///
    /// A chord the focused webview declared in its `forward_keys` is typed
    /// this way, and the page does not receive it.
    Type {
        /// The logical key, for text/printable-key mapping.
        logical: Key,
        /// The physical key, for named-key mapping.
        key_code: KeyCode,
        /// The modifiers the key is typed with.
        mods: Modifiers,
    },
}

/// Per-batch context needed to classify one frame's pressed keys, beyond
/// the leader/shortcut state threaded through `leader_phase`.
pub(crate) struct BatchContext<'a> {
    /// The frame's held modifier keys, shared by every event in the batch.
    pub(crate) held: HeldModifiers,
    /// Which held Alt / Option keys count as Alt for a key that composes a
    /// character.
    pub(crate) alt_policy: AltPolicy,
    /// The caller's `Time<Real>::elapsed()`, for the repeat-window deadline.
    pub(crate) now: Duration,
    /// Whether the focused terminal is currently in vi mode.
    pub(crate) in_vi_mode: bool,
    /// Whether the focused terminal currently holds a selection.
    pub(crate) has_selection: bool,
    /// Whether a webview currently owns the keyboard.
    pub(crate) webview_focused: bool,
    /// The focused webview's declared forward-key chords (empty when none).
    pub(crate) forward_chords: &'a [NormalizedChord],
}

impl BatchContext<'_> {
    /// Whether a direct chord bound to `action` claims a key pressed with
    /// `mods`, rather than leaving it to vi-mode resolution or to the PTY.
    ///
    /// A direct `Copy` on a Ctrl-only chord claims the key only while a
    /// selection exists, so `Ctrl+C` still reaches the PTY as `0x03` with
    /// nothing to copy. A direct `Paste` does not claim the key in vi mode,
    /// where the action is inert.
    fn claims_direct_chord(&self, action: Shortcut, mods: Modifiers) -> bool {
        match action {
            Shortcut::Copy => {
                let ctrl_only = mods.ctrl && !mods.shift && !mods.alt && !mods.meta;
                self.has_selection || !ctrl_only
            }
            Shortcut::Paste => !self.in_vi_mode,
            _ => true,
        }
    }
}

/// The result of classifying one frame's pressed keys: the per-key
/// `KeyEffect`s, plus the physical keys withheld from the focused webview —
/// those the leader claimed, the direct chords that fired, and those that
/// matched a forward chord. The caller applies the frame's modifier snapshot
/// when withholding them from CEF via `CefKeyboardFilter`; it is empty on the
/// non-webview path.
pub(crate) struct ClassifiedKeys {
    pub(crate) effects: Vec<KeyEffect>,
    pub(crate) webview_suppressed: Vec<KeyCode>,
}

/// Classifies one frame's pressed `KeyboardInput` events into `KeyEffect`s,
/// threading the shared leader state machine across the batch. Pure: no
/// ECS handles.
///
/// A stale repeat window is closed before the batch is processed whenever
/// `ctx.in_vi_mode` is set, so a repeat-marked key that doubles as a
/// vi-mode key resolves against `resolved_vi_mode` instead of re-firing its
/// leader-scoped action.
pub(crate) fn classify_key_batch<'a>(
    leader_phase: &mut LeaderPhase,
    held_repeat: &mut Option<KeyCode>,
    shortcuts: &Shortcuts,
    resolved_vi_mode: &ResolvedViModeKeys,
    events: impl Iterator<Item = &'a KeyboardInput>,
    ctx: BatchContext<'a>,
) -> ClassifiedKeys {
    // NOTE: an open repeat window must not intercept vi-mode keys — a
    // repeat-marked key doubling as a vi-mode key would re-fire its bound
    // action into the hidden live terminal instead of being resolved as a
    // vi-mode key below. Close the window (and disarm hold-to-repeat) before
    // the batch is processed.
    if ctx.in_vi_mode {
        *held_repeat = None;
        if matches!(*leader_phase, LeaderPhase::Repeat { .. }) {
            *leader_phase = LeaderPhase::Idle;
        }
    }
    let mut effects = Vec::new();
    let mut webview_suppressed = Vec::new();
    for ev in events.filter(|ev| ev.state == ButtonState::Pressed) {
        let mods = ctx.held.for_key(&ev.logical_key, ctx.alt_policy);
        let step = step_with_repeat(leader_phase, held_repeat, shortcuts, ev, mods, ctx.now);
        let abandoned = step == LeaderStep::Abandoned;
        if ctx.webview_focused {
            // NOTE: the leader, and the direct chords `match_over_webview`
            // admits, run even while a webview owns the keyboard, ahead of the
            // chords the webview declared as forward keys. Every key they claim
            // (the leader chord itself, an abandoned second key, a direct
            // chord, including its repeats, or a forwarded chord) is recorded
            // in `webview_suppressed` so the caller withholds it from CEF; any
            // other key still reaches the page.
            match step {
                LeaderStep::Swallow => {
                    webview_suppressed.push(ev.key_code);
                }
                LeaderStep::RunAction(action) => {
                    webview_suppressed.push(ev.key_code);
                    effects.push(KeyEffect::Shortcut {
                        action,
                        via_leader: true,
                    });
                }
                LeaderStep::Passthrough | LeaderStep::Abandoned => {
                    if let Some(hit) = shortcuts.match_over_webview(ev.key_code, mods) {
                        webview_suppressed.push(ev.key_code);
                        if hit.fires(ev.repeat) {
                            effects.push(KeyEffect::Shortcut {
                                action: hit.action,
                                via_leader: false,
                            });
                        }
                    } else if ctx
                        .forward_chords
                        .iter()
                        .any(|chord| chord_matches(chord, ev.key_code, &ev.logical_key, mods))
                    {
                        webview_suppressed.push(ev.key_code);
                        effects.push(KeyEffect::Type {
                            logical: ev.logical_key.clone(),
                            key_code: ev.key_code,
                            mods,
                        });
                    } else if abandoned {
                        webview_suppressed.push(ev.key_code);
                    }
                }
            }
            continue;
        }
        let action = match step {
            LeaderStep::Swallow => continue,
            LeaderStep::RunAction(action) => Some((action, true)),
            LeaderStep::Passthrough | LeaderStep::Abandoned => {
                match shortcuts.match_gui_action(ev.key_code, mods) {
                    None if abandoned => continue,
                    Some(hit) if ctx.claims_direct_chord(hit.action, mods) => {
                        if !hit.fires(ev.repeat) {
                            continue;
                        }
                        Some((hit.action, false))
                    }
                    _ => None,
                }
            }
        };
        if let Some((action, via_leader)) = action {
            effects.push(KeyEffect::Shortcut { action, via_leader });
            continue;
        }
        // NOTE: vi-mode keys resolve only after leader and GUI-shortcut
        // dispatch declined the key, and a vi-mode key never falls through
        // to Type — an unmatched key in vi mode is swallowed, not typed.
        if ctx.in_vi_mode {
            if let Some(vi_action) = resolved_vi_mode.resolve(&ev.logical_key, ev.key_code, mods) {
                effects.push(KeyEffect::ViMode(vi_action));
            }
            continue;
        }
        if is_modifier_key(ev.key_code) || mods.meta {
            continue;
        }
        effects.push(KeyEffect::Type {
            logical: ev.logical_key.clone(),
            key_code: ev.key_code,
            mods,
        });
    }
    ClassifiedKeys {
        effects,
        webview_suppressed,
    }
}

/// Advances the leader machine for one pressed event, honoring OS auto-repeat.
/// A key still held after firing a repeat-marked binding keeps re-firing on
/// every OS auto-repeat via `held_repeat`, independent of the
/// `LeaderPhase::Repeat` time window (which is too short to bridge the OS
/// initial-repeat delay). A fresh press arms `held_repeat` when it opens or
/// renews the window and clears it otherwise; outside both, a repeat event does
/// NOT step the machine.
fn step_with_repeat(
    leader_phase: &mut LeaderPhase,
    held_repeat: &mut Option<KeyCode>,
    shortcuts: &Shortcuts,
    ev: &KeyboardInput,
    mods: Modifiers,
    now: Duration,
) -> LeaderStep {
    if ev.repeat {
        if *held_repeat == Some(ev.key_code)
            && let Some(action) =
                refire_held_repeat(leader_phase, shortcuts, ev.key_code, mods, now)
        {
            return LeaderStep::RunAction(action);
        }
        return match *leader_phase {
            LeaderPhase::Pending => LeaderStep::Swallow,
            LeaderPhase::Repeat { .. } => {
                step_leader(leader_phase, shortcuts, ev.key_code, mods, now)
            }
            LeaderPhase::Idle => LeaderStep::Passthrough,
        };
    }
    let step = step_leader(leader_phase, shortcuts, ev.key_code, mods, now);
    *held_repeat = match (&step, &*leader_phase) {
        (LeaderStep::RunAction(_), LeaderPhase::Repeat { .. }) => Some(ev.key_code),
        _ => None,
    };
    step
}

/// True when `chord` (a focused webview's declared forward-key entry)
/// matches a pressed event.
///
/// A [`ChordKey::Code`] chord compares the physical key and the exact
/// modifier set. A [`ChordKey::Char`] chord compares the character the key
/// produced and every modifier except Shift, which the character already
/// reflects.
fn chord_matches(
    chord: &NormalizedChord,
    key_code: KeyCode,
    logical: &Key,
    mods: Modifiers,
) -> bool {
    let others_match = chord.ctrl == mods.ctrl && chord.alt == mods.alt && chord.logo == mods.meta;
    match chord.key {
        ChordKey::Code(code) => others_match && code == key_code && chord.shift == mods.shift,
        ChordKey::Char(c) => {
            let mut buf = [0u8; 4];
            let expected: &str = c.encode_utf8(&mut buf);
            others_match && matches!(logical, Key::Character(text) if text.as_str() == expected)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::keyboard::held_modifiers::{AltPolicy, HeldModifiers};
    use crate::input::shortcuts::{
        Shortcuts, test_shortcuts_with_direct_chord, test_shortcuts_with_repeat_prefix,
    };
    use bevy::prelude::Entity;
    use orzma_configs::keyboard::OptionAsAlt;
    use orzma_configs::shortcuts::{FontSizeStep, PaneDirection};
    use orzma_configs::vi_mode::ViModeSelection;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn mods(ctrl: bool, shift: bool, alt: bool, meta: bool) -> Modifiers {
        Modifiers {
            ctrl,
            shift,
            alt,
            meta,
        }
    }

    fn no_mods() -> Modifiers {
        mods(false, false, false, false)
    }

    fn press(key_code: KeyCode, logical: Key) -> KeyboardInput {
        KeyboardInput {
            key_code,
            logical_key: logical,
            state: ButtonState::Pressed,
            text: None,
            repeat: false,
            window: Entity::PLACEHOLDER,
        }
    }

    fn press_repeat(key_code: KeyCode, logical: Key) -> KeyboardInput {
        KeyboardInput {
            repeat: true,
            ..press(key_code, logical)
        }
    }

    fn ctx(mods: Modifiers, now: Duration) -> BatchContext<'static> {
        BatchContext {
            held: HeldModifiers::from(mods),
            alt_policy: AltPolicy::default(),
            now,
            in_vi_mode: false,
            has_selection: false,
            webview_focused: false,
            forward_chords: &[],
        }
    }

    fn policy_ctx(held: HeldModifiers, alt_policy: AltPolicy) -> BatchContext<'static> {
        BatchContext {
            held,
            alt_policy,
            ..ctx(no_mods(), ms(0))
        }
    }

    fn alt() -> Modifiers {
        mods(false, false, true, false)
    }

    fn run<'a>(
        leader_phase: &mut LeaderPhase,
        shortcuts: &Shortcuts,
        resolved_vi_mode: &ResolvedViModeKeys,
        events: &'a [KeyboardInput],
        ctx: BatchContext<'a>,
    ) -> Vec<KeyEffect> {
        let mut held = None;
        classify_key_batch(
            leader_phase,
            &mut held,
            shortcuts,
            resolved_vi_mode,
            events.iter(),
            ctx,
        )
        .effects
    }

    fn run_full<'a>(
        leader_phase: &mut LeaderPhase,
        shortcuts: &Shortcuts,
        resolved_vi_mode: &ResolvedViModeKeys,
        events: &'a [KeyboardInput],
        ctx: BatchContext<'a>,
    ) -> ClassifiedKeys {
        let mut held = None;
        classify_key_batch(
            leader_phase,
            &mut held,
            shortcuts,
            resolved_vi_mode,
            events.iter(),
            ctx,
        )
    }

    #[test]
    fn leader_press_swallows_and_no_type() {
        let sc = test_shortcuts_with_repeat_prefix(KeyCode::KeyS, Shortcut::EnterViMode, ms(500));
        let resolved_vi_mode = ResolvedViModeKeys::default();
        let mut phase = LeaderPhase::Idle;
        let events = [press(KeyCode::KeyA, Key::Character("a".into()))];
        let effects = run(
            &mut phase,
            &sc,
            &resolved_vi_mode,
            &events,
            ctx(mods(true, false, false, false), ms(0)),
        );
        assert_eq!(
            effects,
            vec![],
            "the leader itself must swallow with no Type"
        );
        assert_eq!(
            phase,
            LeaderPhase::Pending,
            "the leader chord must engage pending"
        );
    }

    #[test]
    fn leader_then_bound_key_emits_action() {
        let sc =
            test_shortcuts_with_repeat_prefix(KeyCode::KeyS, Shortcut::EnterViMode, Duration::ZERO);
        let resolved_vi_mode = ResolvedViModeKeys::default();
        let mut phase = LeaderPhase::Pending;
        let events = [press(KeyCode::KeyS, Key::Character("s".into()))];
        let effects = run(
            &mut phase,
            &sc,
            &resolved_vi_mode,
            &events,
            ctx(no_mods(), ms(0)),
        );
        assert_eq!(
            effects,
            vec![KeyEffect::Shortcut {
                action: Shortcut::EnterViMode,
                via_leader: true,
            }]
        );
        assert_eq!(phase, LeaderPhase::Idle);
    }

    #[test]
    fn direct_gui_chord_emits_action_not_leader() {
        let sc = test_shortcuts_with_direct_chord(
            KeyCode::KeyQ,
            mods(false, false, false, true),
            Shortcut::Quit,
        );
        let resolved_vi_mode = ResolvedViModeKeys::default();
        let mut phase = LeaderPhase::Idle;
        let events = [press(KeyCode::KeyQ, Key::Character("q".into()))];
        let effects = run(
            &mut phase,
            &sc,
            &resolved_vi_mode,
            &events,
            ctx(mods(false, false, false, true), ms(0)),
        );
        assert_eq!(
            effects,
            vec![KeyEffect::Shortcut {
                action: Shortcut::Quit,
                via_leader: false,
            }]
        );
        assert_eq!(
            phase,
            LeaderPhase::Idle,
            "a direct GUI chord must never engage the leader"
        );
    }

    #[test]
    fn plain_key_emits_type() {
        let sc = Shortcuts::default();
        let resolved_vi_mode = ResolvedViModeKeys::default();
        let mut phase = LeaderPhase::Idle;
        let events = [press(KeyCode::KeyA, Key::Character("a".into()))];
        let effects = run(
            &mut phase,
            &sc,
            &resolved_vi_mode,
            &events,
            ctx(no_mods(), ms(0)),
        );
        assert_eq!(
            effects,
            vec![KeyEffect::Type {
                logical: Key::Character("a".into()),
                key_code: KeyCode::KeyA,
                mods: no_mods(),
            }]
        );
    }

    #[test]
    fn repeat_window_refires_on_os_repeat() {
        let sc = test_shortcuts_with_repeat_prefix(KeyCode::KeyH, Shortcut::EnterViMode, ms(500));
        let resolved_vi_mode = ResolvedViModeKeys::default();
        let mut phase = LeaderPhase::Repeat { deadline: ms(500) };
        let events = [press_repeat(KeyCode::KeyH, Key::Character("h".into()))];
        let effects = run(
            &mut phase,
            &sc,
            &resolved_vi_mode,
            &events,
            ctx(no_mods(), ms(100)),
        );
        assert_eq!(
            effects,
            vec![KeyEffect::Shortcut {
                action: Shortcut::EnterViMode,
                via_leader: true,
            }]
        );
        assert_eq!(
            phase,
            LeaderPhase::Repeat { deadline: ms(600) },
            "firing must re-arm the window"
        );
    }

    #[test]
    fn repeat_outside_window_passthrough_no_step() {
        let sc = test_shortcuts_with_repeat_prefix(KeyCode::KeyH, Shortcut::EnterViMode, ms(500));
        let resolved_vi_mode = ResolvedViModeKeys::default();
        let mut phase = LeaderPhase::Idle;
        let events = [press_repeat(KeyCode::KeyH, Key::Character("h".into()))];
        let effects = run(
            &mut phase,
            &sc,
            &resolved_vi_mode,
            &events,
            ctx(no_mods(), ms(0)),
        );
        assert_eq!(
            effects,
            vec![KeyEffect::Type {
                logical: Key::Character("h".into()),
                key_code: KeyCode::KeyH,
                mods: no_mods(),
            }],
            "an auto-repeat outside the window must not step the leader machine"
        );
        assert_eq!(phase, LeaderPhase::Idle);
    }

    #[test]
    fn held_repeat_refires_after_time_window_expires() {
        // Hold the key: the first press fires + arms the 500ms window AND
        // hold-to-repeat. The OS's first auto-repeat lands well past the window
        // (~1020ms), yet it must STILL re-fire the action — not leak to Type.
        let sc = test_shortcuts_with_repeat_prefix(KeyCode::KeyH, Shortcut::EnterViMode, ms(500));
        let rc = ResolvedViModeKeys::default();
        let mut phase = LeaderPhase::Pending;
        let mut held = None;
        let first = [press(KeyCode::KeyH, Key::Character("h".into()))];
        let out1 = classify_key_batch(
            &mut phase,
            &mut held,
            &sc,
            &rc,
            first.iter(),
            ctx(no_mods(), ms(0)),
        );
        assert_eq!(
            out1.effects,
            vec![KeyEffect::Shortcut {
                action: Shortcut::EnterViMode,
                via_leader: true,
            }],
            "the leader press fires the repeat binding"
        );
        assert_eq!(
            held,
            Some(KeyCode::KeyH),
            "the held key arms hold-to-repeat"
        );

        let repeat = [press_repeat(KeyCode::KeyH, Key::Character("h".into()))];
        let out2 = classify_key_batch(
            &mut phase,
            &mut held,
            &sc,
            &rc,
            repeat.iter(),
            ctx(no_mods(), ms(1020)),
        );
        assert_eq!(
            out2.effects,
            vec![KeyEffect::Shortcut {
                action: Shortcut::EnterViMode,
                via_leader: true,
            }],
            "an OS auto-repeat of the held key re-fires even after the 500ms window closed"
        );
        assert!(
            !out2
                .effects
                .iter()
                .any(|e| matches!(e, KeyEffect::Type { .. })),
            "the held key must not leak into the terminal"
        );
        assert_eq!(
            held,
            Some(KeyCode::KeyH),
            "the held key stays armed across auto-repeats"
        );
    }

    #[test]
    fn fresh_press_without_leader_clears_stale_held_repeat() {
        // A key armed by an earlier hold must NOT keep re-firing after it is
        // released and re-pressed WITHOUT the leader: the fresh (repeat:false)
        // press with no leader engaged disarms the stale hold, so it types.
        let sc = test_shortcuts_with_repeat_prefix(KeyCode::KeyH, Shortcut::EnterViMode, ms(500));
        let rc = ResolvedViModeKeys::default();
        let mut phase = LeaderPhase::Idle;
        let mut held = Some(KeyCode::KeyH);
        let fresh = [press(KeyCode::KeyH, Key::Character("h".into()))];
        let out = classify_key_batch(
            &mut phase,
            &mut held,
            &sc,
            &rc,
            fresh.iter(),
            ctx(no_mods(), ms(5000)),
        );
        assert_eq!(
            held, None,
            "a fresh press with no leader engaged disarms the stale hold"
        );
        assert_eq!(
            out.effects,
            vec![KeyEffect::Type {
                logical: Key::Character("h".into()),
                key_code: KeyCode::KeyH,
                mods: no_mods(),
            }],
            "and the key types normally instead of re-firing the shortcut"
        );
    }

    #[test]
    fn vi_mode_disarms_held_repeat() {
        // Vi mode must not let a held resize key re-fire into the hidden live
        // terminal: the pre-loop guard disarms hold-to-repeat.
        let sc = test_shortcuts_with_repeat_prefix(KeyCode::KeyH, Shortcut::EnterViMode, ms(500));
        let rc = ResolvedViModeKeys::default();
        let mut phase = LeaderPhase::Repeat {
            deadline: ms(60_000),
        };
        let mut held = Some(KeyCode::KeyH);
        let repeat = [press_repeat(KeyCode::KeyH, Key::Character("h".into()))];
        let mut c = ctx(no_mods(), ms(0));
        c.in_vi_mode = true;
        let out = classify_key_batch(&mut phase, &mut held, &sc, &rc, repeat.iter(), c);
        assert_eq!(held, None, "vi mode disarms hold-to-repeat");
        assert!(
            !out.effects
                .iter()
                .any(|e| matches!(e, KeyEffect::Shortcut { .. })),
            "a held repeat key must not re-fire its action in vi mode"
        );
    }

    #[test]
    fn pending_skips_bare_modifier_then_second_key() {
        let sc =
            test_shortcuts_with_repeat_prefix(KeyCode::KeyD, Shortcut::KillPane, Duration::ZERO);
        let resolved_vi_mode = ResolvedViModeKeys::default();
        let mut phase = LeaderPhase::Pending;
        let events = [
            press(KeyCode::ControlLeft, Key::Control),
            press(KeyCode::KeyD, Key::Character("d".into())),
        ];
        let effects = run(
            &mut phase,
            &sc,
            &resolved_vi_mode,
            &events,
            ctx(no_mods(), ms(0)),
        );
        assert_eq!(
            effects,
            vec![KeyEffect::Shortcut {
                action: Shortcut::KillPane,
                via_leader: true,
            }],
            "the leading bare modifier must not consume the pending slot; the real \
             second key must resolve"
        );
        assert_eq!(phase, LeaderPhase::Idle);
    }

    #[test]
    fn pending_suppresses_type_for_second_key() {
        let sc = test_shortcuts_with_repeat_prefix(KeyCode::KeyZ, Shortcut::KillPane, ms(500));
        let resolved_vi_mode = ResolvedViModeKeys::default();
        let mut phase = LeaderPhase::Pending;
        let events = [press(KeyCode::KeyA, Key::Character("a".into()))];
        let effects = run(
            &mut phase,
            &sc,
            &resolved_vi_mode,
            &events,
            ctx(no_mods(), ms(0)),
        );
        assert_eq!(
            effects,
            vec![],
            "an unbound second key while pending must be swallowed, not typed"
        );
        assert_eq!(phase, LeaderPhase::Idle);
    }

    #[test]
    fn pending_types_trailing_same_frame_key() {
        let sc = test_shortcuts_with_repeat_prefix(KeyCode::KeyZ, Shortcut::KillPane, ms(500));
        let resolved_vi_mode = ResolvedViModeKeys::default();
        let mut phase = LeaderPhase::Pending;
        let events = [
            press(KeyCode::KeyA, Key::Character("a".into())),
            press(KeyCode::KeyB, Key::Character("b".into())),
        ];
        let effects = run(
            &mut phase,
            &sc,
            &resolved_vi_mode,
            &events,
            ctx(no_mods(), ms(0)),
        );
        assert_eq!(
            effects,
            vec![KeyEffect::Type {
                logical: Key::Character("b".into()),
                key_code: KeyCode::KeyB,
                mods: no_mods(),
            }],
            "a trailing same-frame key after the suppressed second key must be typed"
        );
    }

    #[test]
    fn repeat_window_withholds_matching_key() {
        let sc =
            test_shortcuts_with_repeat_prefix(KeyCode::KeyH, Shortcut::EnterViMode, ms(60_000));
        let resolved_vi_mode = ResolvedViModeKeys::default();
        let mut phase = LeaderPhase::Repeat {
            deadline: ms(60_000),
        };
        let events = [press_repeat(KeyCode::KeyH, Key::Character("h".into()))];
        let effects = run(
            &mut phase,
            &sc,
            &resolved_vi_mode,
            &events,
            ctx(no_mods(), ms(0)),
        );
        assert!(
            !effects.iter().any(|e| matches!(e, KeyEffect::Type { .. })),
            "a repeat-marked key inside the window must never also emit Type"
        );
        assert_eq!(
            effects,
            vec![KeyEffect::Shortcut {
                action: Shortcut::EnterViMode,
                via_leader: true,
            }],
            "the action must fire — this is not an empty Vec"
        );
    }

    #[test]
    fn repeat_window_types_non_matching_key() {
        let sc =
            test_shortcuts_with_repeat_prefix(KeyCode::KeyH, Shortcut::EnterViMode, ms(60_000));
        let resolved_vi_mode = ResolvedViModeKeys::default();
        let mut phase = LeaderPhase::Repeat {
            deadline: ms(60_000),
        };
        let events = [press(KeyCode::KeyB, Key::Character("b".into()))];
        let effects = run(
            &mut phase,
            &sc,
            &resolved_vi_mode,
            &events,
            ctx(no_mods(), ms(0)),
        );
        assert_eq!(
            effects,
            vec![KeyEffect::Type {
                logical: Key::Character("b".into()),
                key_code: KeyCode::KeyB,
                mods: no_mods(),
            }],
            "a non-matching key during the repeat window must reach the terminal"
        );
        assert_eq!(
            phase,
            LeaderPhase::Idle,
            "the non-matching key closes the window"
        );
    }

    #[test]
    fn window_closing_key_stops_withholding_same_frame() {
        let sc =
            test_shortcuts_with_repeat_prefix(KeyCode::KeyH, Shortcut::EnterViMode, ms(60_000));
        let resolved_vi_mode = ResolvedViModeKeys::default();
        let mut phase = LeaderPhase::Repeat {
            deadline: ms(60_000),
        };
        let events = [
            press(KeyCode::KeyB, Key::Character("b".into())),
            press(KeyCode::KeyH, Key::Character("h".into())),
        ];
        let effects = run(
            &mut phase,
            &sc,
            &resolved_vi_mode,
            &events,
            ctx(no_mods(), ms(0)),
        );
        assert_eq!(
            effects,
            vec![
                KeyEffect::Type {
                    logical: Key::Character("b".into()),
                    key_code: KeyCode::KeyB,
                    mods: no_mods(),
                },
                KeyEffect::Type {
                    logical: Key::Character("h".into()),
                    key_code: KeyCode::KeyH,
                    mods: no_mods(),
                },
            ],
            "the non-matching key closes the window for the rest of the frame; the \
             repeat key after it must be typed, not withheld"
        );
    }

    #[test]
    fn unbound_ctrl_shift_escape_emits_type() {
        let sc = Shortcuts::default();
        let resolved_vi_mode = ResolvedViModeKeys::default();
        let mut phase = LeaderPhase::Idle;
        let events = [press(KeyCode::Escape, Key::Escape)];
        let effects = run(
            &mut phase,
            &sc,
            &resolved_vi_mode,
            &events,
            ctx(mods(true, true, false, false), ms(0)),
        );
        assert_eq!(
            effects,
            vec![KeyEffect::Type {
                logical: Key::Escape,
                key_code: KeyCode::Escape,
                mods: mods(true, true, false, false),
            }],
            "a chord bound to no action falls through to Type; the decider never \
             swallows on its own — the applier decides"
        );
    }

    #[test]
    fn no_type_while_in_vi_mode() {
        let sc = Shortcuts::default();
        let resolved_vi_mode = ResolvedViModeKeys::default();
        let mut phase = LeaderPhase::Idle;
        let events = [press(KeyCode::KeyX, Key::Character("x".into()))];
        let mut c = ctx(no_mods(), ms(0));
        c.in_vi_mode = true;
        let effects = run(&mut phase, &sc, &resolved_vi_mode, &events, c);
        assert!(
            !effects.iter().any(|e| matches!(e, KeyEffect::Type { .. })),
            "an unmatched key in vi mode must never fall through to Type"
        );
    }

    #[test]
    fn vi_key_shadowed_by_gui() {
        let sc = test_shortcuts_with_direct_chord(KeyCode::KeyV, no_mods(), Shortcut::EnterViMode);
        let resolved_vi_mode = ResolvedViModeKeys::default();
        let mut phase = LeaderPhase::Idle;
        let events = [press(KeyCode::KeyV, Key::Character("v".into()))];
        let mut c = ctx(no_mods(), ms(0));
        c.in_vi_mode = true;
        let effects = run(&mut phase, &sc, &resolved_vi_mode, &events, c);
        assert_eq!(
            effects,
            vec![KeyEffect::Shortcut {
                action: Shortcut::EnterViMode,
                via_leader: false,
            }],
            "a bound GUI chord must shadow a vi-mode key, not resolve as ViMode"
        );
    }

    #[test]
    fn meta_unmatched_dropped() {
        let sc = Shortcuts::default();
        let resolved_vi_mode = ResolvedViModeKeys::default();
        let mut phase = LeaderPhase::Idle;
        let events = [press(KeyCode::KeyJ, Key::Character("j".into()))];
        let effects = run(
            &mut phase,
            &sc,
            &resolved_vi_mode,
            &events,
            ctx(mods(false, false, false, true), ms(0)),
        );
        assert_eq!(effects, vec![], "Cmd+J must not reach the terminal");
    }

    /// Asserts that a direct paste chord is left unclaimed in vi mode rather
    /// than swallowed, so the key reaches the `[vi-mode]` table instead of
    /// dying on an action that is inert there.
    ///
    /// Case: a Windows user opens vi mode and presses `Ctrl+V`, which is both
    /// the stock paste chord and vi mode's rectangular-selection toggle.
    #[test]
    fn direct_paste_unclaimed_in_vi_mode() {
        let sc = test_shortcuts_with_direct_chord(
            KeyCode::KeyV,
            mods(true, false, false, false),
            Shortcut::Paste,
        );
        let resolved_vi_mode = ResolvedViModeKeys::test_with_ctrl_keys([(
            KeyCode::KeyV,
            ViModeAction::Selection(ViModeSelection::Rect),
        )]);
        let mut phase = LeaderPhase::Idle;
        let events = [press(KeyCode::KeyV, Key::Character("v".into()))];
        let mut c = ctx(mods(true, false, false, false), ms(0));
        c.in_vi_mode = true;
        let effects = run(&mut phase, &sc, &resolved_vi_mode, &events, c);
        assert_eq!(
            effects,
            vec![KeyEffect::ViMode(ViModeAction::Selection(
                ViModeSelection::Rect
            ))],
            "a direct paste chord must fall through to vi-mode resolution"
        );
    }

    /// Asserts that a Ctrl-only copy chord claims the key while a selection
    /// exists.
    ///
    /// Case: a Windows user drags out a selection and presses `Ctrl+C`.
    #[test]
    fn ctrl_copy_claims_key_with_selection() {
        let sc = test_shortcuts_with_direct_chord(
            KeyCode::KeyC,
            mods(true, false, false, false),
            Shortcut::Copy,
        );
        let resolved_vi_mode = ResolvedViModeKeys::default();
        let mut phase = LeaderPhase::Idle;
        let events = [press(KeyCode::KeyC, Key::Character("c".into()))];
        let mut c = ctx(mods(true, false, false, false), ms(0));
        c.has_selection = true;
        let effects = run(&mut phase, &sc, &resolved_vi_mode, &events, c);
        assert_eq!(
            effects,
            vec![KeyEffect::Shortcut {
                action: Shortcut::Copy,
                via_leader: false,
            }]
        );
    }

    /// Asserts that a Ctrl-only copy chord types instead of copying when no
    /// selection exists, so the PTY still receives the interrupt byte.
    ///
    /// Case: a Windows user presses `Ctrl+C` to interrupt a running command
    /// with nothing selected.
    #[test]
    fn ctrl_copy_falls_through_to_pty_without_selection() {
        let sc = test_shortcuts_with_direct_chord(
            KeyCode::KeyC,
            mods(true, false, false, false),
            Shortcut::Copy,
        );
        let resolved_vi_mode = ResolvedViModeKeys::default();
        let mut phase = LeaderPhase::Idle;
        let events = [press(KeyCode::KeyC, Key::Character("c".into()))];
        let effects = run(
            &mut phase,
            &sc,
            &resolved_vi_mode,
            &events,
            ctx(mods(true, false, false, false), ms(0)),
        );
        assert_eq!(
            effects,
            vec![KeyEffect::Type {
                logical: Key::Character("c".into()),
                key_code: KeyCode::KeyC,
                mods: mods(true, false, false, false),
            }],
            "Ctrl+C with no selection must reach the PTY"
        );
    }

    /// Asserts that a copy chord carrying `meta` claims the key even with no
    /// selection, so the macOS `Cmd+C` default never types into the PTY.
    ///
    /// Case: a macOS user presses `Cmd+C` with nothing selected.
    #[test]
    fn meta_copy_claims_key_without_selection() {
        let sc = test_shortcuts_with_direct_chord(
            KeyCode::KeyC,
            mods(false, false, false, true),
            Shortcut::Copy,
        );
        let resolved_vi_mode = ResolvedViModeKeys::default();
        let mut phase = LeaderPhase::Idle;
        let events = [press(KeyCode::KeyC, Key::Character("c".into()))];
        let effects = run(
            &mut phase,
            &sc,
            &resolved_vi_mode,
            &events,
            ctx(mods(false, false, false, true), ms(0)),
        );
        assert_eq!(
            effects,
            vec![KeyEffect::Shortcut {
                action: Shortcut::Copy,
                via_leader: false,
            }]
        );
    }

    /// Asserts that a leader-scoped copy binding claims the key with no
    /// selection, since no control byte is at stake behind the leader.
    ///
    /// Case: a user taps the leader then the copy key with nothing selected.
    #[test]
    fn leader_copy_claims_key_without_selection() {
        let sc = test_shortcuts_with_repeat_prefix(KeyCode::KeyC, Shortcut::Copy, Duration::ZERO);
        let resolved_vi_mode = ResolvedViModeKeys::default();
        let mut phase = LeaderPhase::Pending;
        let events = [press(KeyCode::KeyC, Key::Character("c".into()))];
        let effects = run(
            &mut phase,
            &sc,
            &resolved_vi_mode,
            &events,
            ctx(no_mods(), ms(0)),
        );
        assert_eq!(
            effects,
            vec![KeyEffect::Shortcut {
                action: Shortcut::Copy,
                via_leader: true,
            }]
        );
    }

    #[test]
    fn leader_paste_fires_in_vi_mode() {
        let sc = test_shortcuts_with_repeat_prefix(KeyCode::KeyP, Shortcut::Paste, Duration::ZERO);
        let resolved_vi_mode = ResolvedViModeKeys::default();
        let mut phase = LeaderPhase::Pending;
        let events = [press(KeyCode::KeyP, Key::Character("p".into()))];
        let mut c = ctx(no_mods(), ms(0));
        c.in_vi_mode = true;
        let effects = run(&mut phase, &sc, &resolved_vi_mode, &events, c);
        assert_eq!(
            effects,
            vec![KeyEffect::Shortcut {
                action: Shortcut::Paste,
                via_leader: true,
            }]
        );
    }

    #[test]
    fn in_vi_mode_closes_stale_repeat_window_before_dispatch() {
        // Discriminates the pre-loop guard specifically: an AUTO-REPEAT of the
        // repeat-bound key (KeyH) inside an open window. Without the guard,
        // `step_with_repeat` sees `ev.repeat && LeaderPhase::Repeat` and calls
        // `step_leader`, which re-fires `Shortcut{EnterViMode}` — `step_leader`'s
        // own Repeat arm does NOT close the window here because the key MATCHES.
        // The pre-loop guard is the only thing that forces the phase to Idle so
        // the same key resolves to vi-mode (unbound → nothing) instead.
        let sc =
            test_shortcuts_with_repeat_prefix(KeyCode::KeyH, Shortcut::EnterViMode, ms(60_000));
        let resolved_vi_mode = ResolvedViModeKeys::default();
        let mut phase = LeaderPhase::Repeat {
            deadline: ms(60_000),
        };
        let events = [press_repeat(KeyCode::KeyH, Key::Character("h".into()))];
        let mut c = ctx(no_mods(), ms(0));
        c.in_vi_mode = true;
        let effects = run(&mut phase, &sc, &resolved_vi_mode, &events, c);
        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, KeyEffect::Shortcut { .. })),
            "the stale repeat window must be closed before dispatch, so a \
             repeat-marked key in vi mode must NOT re-fire its bound action"
        );
        assert_eq!(
            phase,
            LeaderPhase::Idle,
            "the pre-loop guard must close the window before the batch"
        );
    }

    #[test]
    fn webview_pending_leader_fires_action_and_suppresses() {
        let sc =
            test_shortcuts_with_repeat_prefix(KeyCode::KeyS, Shortcut::EnterViMode, Duration::ZERO);
        let resolved_vi_mode = ResolvedViModeKeys::default();
        let mut phase = LeaderPhase::Pending;
        let events = [press(KeyCode::KeyS, Key::Character("s".into()))];
        let mut c = ctx(no_mods(), ms(0));
        c.webview_focused = true;
        let out = run_full(&mut phase, &sc, &resolved_vi_mode, &events, c);
        assert_eq!(
            out.effects,
            vec![KeyEffect::Shortcut {
                action: Shortcut::EnterViMode,
                via_leader: true,
            }],
            "a bound second key fires its leader action even while a webview is focused"
        );
        assert_eq!(
            out.webview_suppressed,
            vec![KeyCode::KeyS],
            "the fired key is withheld from CEF"
        );
        assert_eq!(phase, LeaderPhase::Idle);
    }

    #[test]
    fn webview_idle_leader_chord_engages_and_suppresses() {
        let sc =
            test_shortcuts_with_repeat_prefix(KeyCode::KeyS, Shortcut::EnterViMode, Duration::ZERO);
        let resolved_vi_mode = ResolvedViModeKeys::default();
        let mut phase = LeaderPhase::Idle;
        let events = [press(KeyCode::KeyA, Key::Character("a".into()))];
        let mut c = ctx(mods(true, false, false, false), ms(0));
        c.webview_focused = true;
        let out = run_full(&mut phase, &sc, &resolved_vi_mode, &events, c);
        assert_eq!(
            out.effects,
            vec![],
            "the leader chord itself emits no effect"
        );
        assert_eq!(
            out.webview_suppressed,
            vec![KeyCode::KeyA],
            "the leader chord is withheld from CEF"
        );
        assert_eq!(
            phase,
            LeaderPhase::Pending,
            "pressing the leader chord under webview focus engages Pending"
        );
    }

    #[test]
    fn webview_pending_unbound_key_swallowed_and_suppressed() {
        let sc =
            test_shortcuts_with_repeat_prefix(KeyCode::KeyS, Shortcut::EnterViMode, Duration::ZERO);
        let resolved_vi_mode = ResolvedViModeKeys::default();
        let mut phase = LeaderPhase::Pending;
        let events = [press(KeyCode::KeyZ, Key::Character("z".into()))];
        let mut c = ctx(no_mods(), ms(0));
        c.webview_focused = true;
        let out = run_full(&mut phase, &sc, &resolved_vi_mode, &events, c);
        assert_eq!(out.effects, vec![], "an unbound second key emits nothing");
        assert_eq!(
            out.webview_suppressed,
            vec![KeyCode::KeyZ],
            "the swallowed second key is still withheld from CEF"
        );
        assert_eq!(phase, LeaderPhase::Idle);
    }

    #[test]
    fn webview_idle_plain_key_not_suppressed() {
        let sc = Shortcuts::default();
        let resolved_vi_mode = ResolvedViModeKeys::default();
        let mut phase = LeaderPhase::Idle;
        let events = [press(KeyCode::KeyB, Key::Character("b".into()))];
        let mut c = ctx(no_mods(), ms(0));
        c.webview_focused = true;
        let out = run_full(&mut phase, &sc, &resolved_vi_mode, &events, c);
        assert_eq!(
            out.effects,
            vec![],
            "a plain key under webview focus emits nothing"
        );
        assert!(
            out.webview_suppressed.is_empty(),
            "a non-leader key is NOT withheld — the webview must still receive it"
        );
        assert_eq!(phase, LeaderPhase::Idle);
    }

    fn char_chord(c: char) -> NormalizedChord {
        NormalizedChord {
            key: ChordKey::Char(c),
            alt: false,
            ctrl: false,
            shift: false,
            logo: false,
        }
    }

    fn forward_ctx(chords: &[NormalizedChord], mods: Modifiers) -> BatchContext<'_> {
        let mut c = ctx(mods, ms(0));
        c.webview_focused = true;
        c.forward_chords = chords;
        c
    }

    /// Asserts that a punctuation chord matches the character a key produced
    /// with Shift held, whichever physical key produced it.
    ///
    /// Case: on a JIS keyboard the user presses Shift+/ to open the help of a
    /// TUI browser that forwards `?`, and the same view runs under a layout
    /// that puts `?` on another key.
    #[test]
    fn a_punctuation_chord_matches_the_produced_character_with_shift_held() {
        let sc = Shortcuts::default();
        let resolved_vi_mode = ResolvedViModeKeys::default();
        let chords = [char_chord('?')];
        for key_code in [KeyCode::Slash, KeyCode::Minus] {
            let mut phase = LeaderPhase::Idle;
            let events = [press(key_code, Key::Character("?".into()))];
            let out = run_full(
                &mut phase,
                &sc,
                &resolved_vi_mode,
                &events,
                forward_ctx(&chords, mods(false, true, false, false)),
            );
            assert_eq!(
                out.effects,
                vec![KeyEffect::Type {
                    logical: Key::Character("?".into()),
                    key_code,
                    mods: mods(false, true, false, false),
                }],
                "`?` produced by {key_code:?} must match the `?` chord"
            );
        }
    }

    /// Asserts that a punctuation chord does not match while Ctrl is held
    /// unless the chord names Ctrl.
    ///
    /// Case: the user presses Ctrl+/ (a page shortcut) in a view that
    /// forwards a plain `/`.
    #[test]
    fn a_punctuation_chord_requires_its_ctrl_state() {
        let sc = Shortcuts::default();
        let resolved_vi_mode = ResolvedViModeKeys::default();
        let chords = [char_chord('/')];
        let mut phase = LeaderPhase::Idle;
        let events = [press(KeyCode::Slash, Key::Character("/".into()))];
        let out = run_full(
            &mut phase,
            &sc,
            &resolved_vi_mode,
            &events,
            forward_ctx(&chords, mods(true, false, false, false)),
        );
        assert_eq!(out.effects, vec![], "Ctrl+/ must stay with the page");
    }

    /// Asserts that a physical-key chord still compares Shift exactly.
    ///
    /// Case: a view forwards Shift+g (go to bottom) but not a plain g, and
    /// the user presses both.
    #[test]
    fn a_physical_key_chord_compares_shift_exactly() {
        let sc = Shortcuts::default();
        let resolved_vi_mode = ResolvedViModeKeys::default();
        let chords = [NormalizedChord {
            key: ChordKey::Code(KeyCode::KeyG),
            alt: false,
            ctrl: false,
            shift: true,
            logo: false,
        }];
        let mut phase = LeaderPhase::Idle;
        let plain = [press(KeyCode::KeyG, Key::Character("g".into()))];
        let out = run_full(
            &mut phase,
            &sc,
            &resolved_vi_mode,
            &plain,
            forward_ctx(&chords, no_mods()),
        );
        assert_eq!(out.effects, vec![], "a plain g must stay with the page");
        let shifted = [press(KeyCode::KeyG, Key::Character("G".into()))];
        let out = run_full(
            &mut phase,
            &sc,
            &resolved_vi_mode,
            &shifted,
            forward_ctx(&chords, mods(false, true, false, false)),
        );
        assert_eq!(
            out.effects,
            vec![KeyEffect::Type {
                logical: Key::Character("G".into()),
                key_code: KeyCode::KeyG,
                mods: mods(false, true, false, false),
            }]
        );
    }

    /// Asserts that a declared forward chord is forwarded and withheld from
    /// the page.
    ///
    /// Case: a markdown viewer forwards `k`, the user has clicked its page,
    /// and presses `k` to scroll up.
    #[test]
    fn webview_idle_forward_chord_forwards_and_is_withheld_from_the_page() {
        let sc = Shortcuts::default();
        let resolved_vi_mode = ResolvedViModeKeys::default();
        let mut phase = LeaderPhase::Idle;
        let chords = [NormalizedChord {
            key: ChordKey::Code(KeyCode::KeyK),
            alt: false,
            ctrl: false,
            shift: false,
            logo: false,
        }];
        let events = [press(KeyCode::KeyK, Key::Character("k".into()))];
        let out = run_full(
            &mut phase,
            &sc,
            &resolved_vi_mode,
            &events,
            forward_ctx(&chords, no_mods()),
        );
        assert_eq!(
            out.effects,
            vec![KeyEffect::Type {
                logical: Key::Character("k".into()),
                key_code: KeyCode::KeyK,
                mods: no_mods(),
            }]
        );
        assert_eq!(out.webview_suppressed, vec![KeyCode::KeyK]);
        assert_eq!(phase, LeaderPhase::Idle);
    }

    /// Asserts that each auto-repeat of a held forward chord is forwarded
    /// and withheld from the page again.
    ///
    /// Case: the user holds `j` to keep scrolling a markdown page whose
    /// viewer forwards `j`.
    #[test]
    fn a_held_forward_chord_forwards_and_is_withheld_on_every_repeat() {
        let sc = Shortcuts::default();
        let resolved_vi_mode = ResolvedViModeKeys::default();
        let mut phase = LeaderPhase::Idle;
        let chords = [NormalizedChord {
            key: ChordKey::Code(KeyCode::KeyJ),
            alt: false,
            ctrl: false,
            shift: false,
            logo: false,
        }];
        let events = [
            press(KeyCode::KeyJ, Key::Character("j".into())),
            press_repeat(KeyCode::KeyJ, Key::Character("j".into())),
            press_repeat(KeyCode::KeyJ, Key::Character("j".into())),
        ];
        let out = run_full(
            &mut phase,
            &sc,
            &resolved_vi_mode,
            &events,
            forward_ctx(&chords, no_mods()),
        );
        assert_eq!(out.effects.len(), 3, "every repeat is forwarded");
        assert_eq!(out.webview_suppressed, vec![KeyCode::KeyJ; 3]);
    }

    #[test]
    fn webview_release_chord_emits_action() {
        let sc = test_shortcuts_with_direct_chord(
            KeyCode::Escape,
            mods(true, true, false, false),
            Shortcut::ReleaseWebviewFocus,
        );
        let resolved_vi_mode = ResolvedViModeKeys::default();
        let mut phase = LeaderPhase::Idle;
        let events = [press(KeyCode::Escape, Key::Escape)];
        let mut c = ctx(mods(true, true, false, false), ms(0));
        c.webview_focused = true;
        let effects = run(&mut phase, &sc, &resolved_vi_mode, &events, c);
        assert_eq!(
            effects,
            vec![KeyEffect::Shortcut {
                action: Shortcut::ReleaseWebviewFocus,
                via_leader: false,
            }],
            "with a webview focused the release chord resolves as a normal action"
        );
    }

    fn meta() -> Modifiers {
        mods(false, false, false, true)
    }

    fn zoom_in_shortcuts() -> Shortcuts {
        test_shortcuts_with_direct_chord(
            KeyCode::Equal,
            meta(),
            Shortcut::FontSize(FontSizeStep::Increase),
        )
    }

    fn zoom_in_effect() -> KeyEffect {
        KeyEffect::Shortcut {
            action: Shortcut::FontSize(FontSizeStep::Increase),
            via_leader: false,
        }
    }

    fn press_equal() -> KeyboardInput {
        press(KeyCode::Equal, Key::Character("=".into()))
    }

    fn meta_equal_chord() -> NormalizedChord {
        NormalizedChord {
            key: ChordKey::Code(KeyCode::Equal),
            alt: false,
            ctrl: false,
            shift: false,
            logo: true,
        }
    }

    /// Asserts that a direct chord fires while a webview holds keyboard
    /// focus, and that its key is withheld from the page.
    ///
    /// Case: the user has clicked into a markdown page and presses `Cmd+=`
    /// to enlarge the terminal font.
    #[test]
    fn a_direct_chord_fires_over_a_focused_webview() {
        let sc = zoom_in_shortcuts();
        let resolved_vi_mode = ResolvedViModeKeys::default();
        let mut phase = LeaderPhase::Idle;
        let events = [press_equal()];
        let out = run_full(
            &mut phase,
            &sc,
            &resolved_vi_mode,
            &events,
            forward_ctx(&[], meta()),
        );
        assert_eq!(out.effects, vec![zoom_in_effect()]);
        assert_eq!(out.webview_suppressed, vec![KeyCode::Equal]);
    }

    /// Asserts that with `direct-chords-over-webview` off, a direct chord
    /// under webview focus emits nothing and still reaches the page.
    ///
    /// Case: a user who turned the priority off presses `Cmd+=` in a page
    /// that zooms its own content.
    #[test]
    fn a_direct_chord_reaches_the_page_when_the_priority_is_off() {
        let sc = zoom_in_shortcuts().with_direct_chords_over_webview(false);
        let resolved_vi_mode = ResolvedViModeKeys::default();
        let mut phase = LeaderPhase::Idle;
        let events = [press_equal()];
        let out = run_full(
            &mut phase,
            &sc,
            &resolved_vi_mode,
            &events,
            forward_ctx(&[], meta()),
        );
        assert_eq!(out.effects, vec![]);
        assert!(out.webview_suppressed.is_empty());
    }

    /// Asserts that direct copy and paste chords do not fire while a webview
    /// holds keyboard focus, and that their keys still reach the page.
    ///
    /// Case: the user selects text in a focused page, presses `Cmd+C`, and
    /// then presses `Cmd+V` in one of the page's inputs.
    #[test]
    fn copy_and_paste_chords_stay_with_a_focused_webview() {
        let resolved_vi_mode = ResolvedViModeKeys::default();
        for (key_code, text, action) in [
            (KeyCode::KeyC, "c", Shortcut::Copy),
            (KeyCode::KeyV, "v", Shortcut::Paste),
        ] {
            let sc = test_shortcuts_with_direct_chord(key_code, meta(), action);
            let mut phase = LeaderPhase::Idle;
            let events = [press(key_code, Key::Character(text.into()))];
            let out = run_full(
                &mut phase,
                &sc,
                &resolved_vi_mode,
                &events,
                forward_ctx(&[], meta()),
            );
            assert_eq!(out.effects, vec![], "{action:?} must not fire");
            assert!(
                out.webview_suppressed.is_empty(),
                "{action:?} must reach the page"
            );
        }
    }

    /// Asserts that a direct chord the focused page also forwards fires as
    /// the shortcut rather than reaching the PTY.
    ///
    /// Case: a TUI viewer forwards `Cmd+=`, and the user presses it while the
    /// viewer's page has focus.
    #[test]
    fn a_direct_chord_wins_over_a_forward_chord() {
        let sc = zoom_in_shortcuts();
        let resolved_vi_mode = ResolvedViModeKeys::default();
        let chords = [meta_equal_chord()];
        let mut phase = LeaderPhase::Idle;
        let events = [press_equal()];
        let out = run_full(
            &mut phase,
            &sc,
            &resolved_vi_mode,
            &events,
            forward_ctx(&chords, meta()),
        );
        assert_eq!(out.effects, vec![zoom_in_effect()]);
        assert_eq!(out.webview_suppressed, vec![KeyCode::Equal]);
    }

    /// Asserts that a copy chord the focused page forwards reaches the PTY.
    ///
    /// Case: on Linux, where copy is bound to `Ctrl+C`, a ratatui app
    /// forwards `Ctrl+C` to quit, and the user presses it while the app's
    /// page has focus.
    #[test]
    fn a_copy_chord_the_page_forwards_reaches_the_pty() {
        let ctrl = mods(true, false, false, false);
        let sc = test_shortcuts_with_direct_chord(KeyCode::KeyC, ctrl, Shortcut::Copy);
        let resolved_vi_mode = ResolvedViModeKeys::default();
        let chords = [NormalizedChord {
            key: ChordKey::Code(KeyCode::KeyC),
            alt: false,
            ctrl: true,
            shift: false,
            logo: false,
        }];
        let mut phase = LeaderPhase::Idle;
        let events = [press(KeyCode::KeyC, Key::Character("c".into()))];
        let out = run_full(
            &mut phase,
            &sc,
            &resolved_vi_mode,
            &events,
            forward_ctx(&chords, ctrl),
        );
        assert_eq!(
            out.effects,
            vec![KeyEffect::Type {
                logical: Key::Character("c".into()),
                key_code: KeyCode::KeyC,
                mods: ctrl,
            }]
        );
        assert_eq!(out.webview_suppressed, vec![KeyCode::KeyC]);
    }

    /// Asserts that with `direct-chords-over-webview` off, a direct chord
    /// the focused page forwards reaches the PTY.
    ///
    /// Case: a user who turned the priority off presses `Cmd+=` in a TUI
    /// viewer that forwards it.
    #[test]
    fn a_forwarded_direct_chord_reaches_the_pty_when_the_priority_is_off() {
        let sc = zoom_in_shortcuts().with_direct_chords_over_webview(false);
        let resolved_vi_mode = ResolvedViModeKeys::default();
        let chords = [meta_equal_chord()];
        let mut phase = LeaderPhase::Idle;
        let events = [press_equal()];
        let out = run_full(
            &mut phase,
            &sc,
            &resolved_vi_mode,
            &events,
            forward_ctx(&chords, meta()),
        );
        assert_eq!(
            out.effects,
            vec![KeyEffect::Type {
                logical: Key::Character("=".into()),
                key_code: KeyCode::Equal,
                mods: meta(),
            }]
        );
        assert_eq!(out.webview_suppressed, vec![KeyCode::Equal]);
    }

    /// Asserts that a direct release chord fires and is withheld from the
    /// page even with `direct-chords-over-webview` off.
    ///
    /// Case: a user who turned the priority off presses their direct
    /// release chord to leave a focused page.
    #[test]
    fn the_release_chord_fires_when_the_priority_is_off() {
        let ctrl_shift = mods(true, true, false, false);
        let sc = test_shortcuts_with_direct_chord(
            KeyCode::Escape,
            ctrl_shift,
            Shortcut::ReleaseWebviewFocus,
        )
        .with_direct_chords_over_webview(false);
        let resolved_vi_mode = ResolvedViModeKeys::default();
        let mut phase = LeaderPhase::Idle;
        let events = [press(KeyCode::Escape, Key::Escape)];
        let out = run_full(
            &mut phase,
            &sc,
            &resolved_vi_mode,
            &events,
            forward_ctx(&[], ctrl_shift),
        );
        assert_eq!(
            out.effects,
            vec![KeyEffect::Shortcut {
                action: Shortcut::ReleaseWebviewFocus,
                via_leader: false,
            }]
        );
        assert_eq!(out.webview_suppressed, vec![KeyCode::Escape]);
    }

    #[test]
    fn release_chord_without_webview_emits_action_not_type() {
        let sc = test_shortcuts_with_direct_chord(
            KeyCode::Escape,
            mods(true, true, false, false),
            Shortcut::ReleaseWebviewFocus,
        );
        let resolved_vi_mode = ResolvedViModeKeys::default();
        let mut phase = LeaderPhase::Idle;
        let events = [press(KeyCode::Escape, Key::Escape)];
        let c = ctx(mods(true, true, false, false), ms(0));
        let effects = run(&mut phase, &sc, &resolved_vi_mode, &events, c);
        assert_eq!(
            effects,
            vec![KeyEffect::Shortcut {
                action: Shortcut::ReleaseWebviewFocus,
                via_leader: false,
            }],
            "with no webview focused the release chord still resolves as an action, never a Type"
        );
    }

    /// Asserts that a key typed with a composing Option key is typed as its
    /// character with no Alt, and does not fire the matching Alt chord.
    ///
    /// Case: a macOS user with `option_as_alt = "right"` types `˙` with left
    /// Option+h while `Alt+h` selects the left pane.
    #[test]
    fn a_composing_option_key_types_its_character_without_alt() {
        let sc = test_shortcuts_with_direct_chord(
            KeyCode::KeyH,
            alt(),
            Shortcut::SelectPane(PaneDirection::Left),
        );
        let held = HeldModifiers {
            alt_left: true,
            ..Default::default()
        };
        let mut phase = LeaderPhase::Idle;
        let events = [press(KeyCode::KeyH, Key::Character("˙".into()))];
        let effects = run(
            &mut phase,
            &sc,
            &ResolvedViModeKeys::default(),
            &events,
            policy_ctx(held, AltPolicy::for_option_as_alt(OptionAsAlt::Right)),
        );
        assert_eq!(
            effects,
            vec![KeyEffect::Type {
                logical: Key::Character("˙".into()),
                key_code: KeyCode::KeyH,
                mods: no_mods(),
            }]
        );
    }

    /// Asserts that the Option key `option_as_alt` names fires the Alt chord.
    ///
    /// Case: a macOS user with `option_as_alt = "right"` presses right
    /// Option+h to select the left pane.
    #[test]
    fn the_alt_option_key_fires_the_alt_chord() {
        let sc = test_shortcuts_with_direct_chord(
            KeyCode::KeyH,
            alt(),
            Shortcut::SelectPane(PaneDirection::Left),
        );
        let held = HeldModifiers {
            alt_right: true,
            ..Default::default()
        };
        let mut phase = LeaderPhase::Idle;
        let events = [press(KeyCode::KeyH, Key::Character("h".into()))];
        let effects = run(
            &mut phase,
            &sc,
            &ResolvedViModeKeys::default(),
            &events,
            policy_ctx(held, AltPolicy::for_option_as_alt(OptionAsAlt::Right)),
        );
        assert_eq!(
            effects,
            vec![KeyEffect::Shortcut {
                action: Shortcut::SelectPane(PaneDirection::Left),
                via_leader: false,
            }]
        );
    }

    /// Asserts that an AltGr-typed character is typed with no Alt and does not
    /// fire the matching Alt chord.
    ///
    /// Case: a user on a German Windows layout types `{` with AltGr+7 while
    /// `Alt+7` is bound.
    #[test]
    fn altgr_types_its_character_without_alt() {
        let sc = test_shortcuts_with_direct_chord(KeyCode::Digit7, alt(), Shortcut::KillPane);
        let held = HeldModifiers {
            alt_right: true,
            alt_graph: true,
            ..Default::default()
        };
        let mut phase = LeaderPhase::Idle;
        let events = [press(KeyCode::Digit7, Key::Character("{".into()))];
        let effects = run(
            &mut phase,
            &sc,
            &ResolvedViModeKeys::default(),
            &events,
            policy_ctx(held, AltPolicy::default()),
        );
        assert_eq!(
            effects,
            vec![KeyEffect::Type {
                logical: Key::Character("{".into()),
                key_code: KeyCode::Digit7,
                mods: no_mods(),
            }]
        );
    }

    /// Asserts that a composing Option key held with Ctrl fires a Ctrl+Alt
    /// chord.
    ///
    /// Case: a macOS user with `option_as_alt = "right"` presses left
    /// Option+Ctrl+Q for a `Ctrl+Alt+Q` binding.
    #[test]
    fn a_composing_option_key_with_ctrl_fires_the_ctrl_alt_chord() {
        let sc = test_shortcuts_with_direct_chord(
            KeyCode::KeyQ,
            mods(true, false, true, false),
            Shortcut::KillPane,
        );
        let held = HeldModifiers {
            ctrl: true,
            alt_left: true,
            ..Default::default()
        };
        let mut phase = LeaderPhase::Idle;
        let events = [press(KeyCode::KeyQ, Key::Character("q".into()))];
        let effects = run(
            &mut phase,
            &sc,
            &ResolvedViModeKeys::default(),
            &events,
            policy_ctx(held, AltPolicy::for_option_as_alt(OptionAsAlt::Right)),
        );
        assert_eq!(
            effects,
            vec![KeyEffect::Shortcut {
                action: Shortcut::KillPane,
                via_leader: false,
            }]
        );
    }

    /// Asserts that a composing Option key fires an Alt chord on a named key.
    ///
    /// Case: a macOS user with `option_as_alt = "right"` presses left
    /// Option+ArrowLeft for an `Alt+ArrowLeft` binding.
    #[test]
    fn a_composing_option_key_fires_the_alt_chord_on_a_named_key() {
        let sc = test_shortcuts_with_direct_chord(
            KeyCode::ArrowLeft,
            alt(),
            Shortcut::SelectPane(PaneDirection::Left),
        );
        let held = HeldModifiers {
            alt_left: true,
            ..Default::default()
        };
        let mut phase = LeaderPhase::Idle;
        let events = [press(KeyCode::ArrowLeft, Key::ArrowLeft)];
        let effects = run(
            &mut phase,
            &sc,
            &ResolvedViModeKeys::default(),
            &events,
            policy_ctx(held, AltPolicy::for_option_as_alt(OptionAsAlt::Right)),
        );
        assert_eq!(
            effects,
            vec![KeyEffect::Shortcut {
                action: Shortcut::SelectPane(PaneDirection::Left),
                via_leader: false,
            }]
        );
    }

    /// Asserts that an AltGr-typed punctuation character matches an
    /// unmodified forward chord for that character on a focused webview.
    ///
    /// Case: a user on a German layout types `[` with AltGr+8 while a page
    /// holds the keyboard and its program forwards `[`.
    #[test]
    fn altgr_punctuation_matches_a_plain_forward_chord() {
        let sc = test_shortcuts_with_direct_chord(KeyCode::KeyQ, alt(), Shortcut::KillPane);
        let chords = [NormalizedChord {
            key: ChordKey::Char('['),
            alt: false,
            ctrl: false,
            shift: false,
            logo: false,
        }];
        let held = HeldModifiers {
            alt_right: true,
            alt_graph: true,
            ..Default::default()
        };
        let mut phase = LeaderPhase::Idle;
        let events = [press(KeyCode::Digit8, Key::Character("[".into()))];
        let classified = run_full(
            &mut phase,
            &sc,
            &ResolvedViModeKeys::default(),
            &events,
            BatchContext {
                webview_focused: true,
                forward_chords: &chords,
                ..policy_ctx(held, AltPolicy::default())
            },
        );
        assert_eq!(
            classified.effects,
            vec![KeyEffect::Type {
                logical: Key::Character("[".into()),
                key_code: KeyCode::Digit8,
                mods: no_mods(),
            }]
        );
        assert_eq!(classified.webview_suppressed, vec![KeyCode::Digit8]);
    }

    fn alt_shift() -> Modifiers {
        mods(false, true, true, false)
    }

    fn ctrl() -> Modifiers {
        mods(true, false, false, false)
    }

    /// Asserts that an unmarked direct chord fires once while held and its OS
    /// key repeats are neither fired nor typed.
    ///
    /// Case: a user holds `Alt+p` a moment too long while closing one pane.
    #[test]
    fn an_unmarked_direct_chord_fires_once_while_held() {
        let sc =
            Shortcuts::default().with_direct_chord(KeyCode::KeyP, alt(), Shortcut::KillPane, false);
        let mut phase = LeaderPhase::Idle;
        let p = || Key::Character("p".into());
        let events = [
            press(KeyCode::KeyP, p()),
            press_repeat(KeyCode::KeyP, p()),
            press_repeat(KeyCode::KeyP, p()),
        ];
        let effects = run(
            &mut phase,
            &sc,
            &ResolvedViModeKeys::default(),
            &events,
            ctx(alt(), ms(0)),
        );
        assert_eq!(
            effects,
            vec![KeyEffect::Shortcut {
                action: Shortcut::KillPane,
                via_leader: false,
            }]
        );
    }

    /// Asserts that an `r:` direct chord fires on every OS key repeat.
    ///
    /// Case: a user holds `Alt+Shift+H` to keep moving a divider left.
    #[test]
    fn a_repeat_marked_direct_chord_fires_on_every_repeat() {
        let sc = Shortcuts::default().with_direct_chord(
            KeyCode::KeyH,
            alt_shift(),
            Shortcut::ResizePane(PaneDirection::Left),
            true,
        );
        let mut phase = LeaderPhase::Idle;
        let h = || Key::Character("H".into());
        let events = [
            press(KeyCode::KeyH, h()),
            press_repeat(KeyCode::KeyH, h()),
            press_repeat(KeyCode::KeyH, h()),
        ];
        let effects = run(
            &mut phase,
            &sc,
            &ResolvedViModeKeys::default(),
            &events,
            ctx(alt_shift(), ms(0)),
        );
        assert_eq!(effects.len(), 3);
        assert!(effects.iter().all(|effect| *effect
            == KeyEffect::Shortcut {
                action: Shortcut::ResizePane(PaneDirection::Left),
                via_leader: false,
            }));
    }

    /// Asserts that an unmarked direct chord's repeats are withheld from a
    /// focused webview without firing again.
    ///
    /// Case: a page holds the keyboard and the user holds `Alt+p`.
    #[test]
    fn an_unmarked_direct_chord_repeat_is_withheld_from_the_page() {
        let sc = test_shortcuts_with_direct_chord(KeyCode::KeyP, alt(), Shortcut::KillPane);
        let mut phase = LeaderPhase::Idle;
        let p = || Key::Character("p".into());
        let events = [press(KeyCode::KeyP, p()), press_repeat(KeyCode::KeyP, p())];
        let classified = run_full(
            &mut phase,
            &sc,
            &ResolvedViModeKeys::default(),
            &events,
            BatchContext {
                webview_focused: true,
                ..ctx(alt(), ms(0))
            },
        );
        assert_eq!(classified.effects.len(), 1);
        assert_eq!(
            classified.webview_suppressed,
            vec![KeyCode::KeyP, KeyCode::KeyP]
        );
    }

    /// Asserts that a copy chord refused for lack of a selection keeps typing
    /// on every OS key repeat.
    ///
    /// Case: a user holds `Ctrl+C` with nothing selected to interrupt a
    /// program.
    #[test]
    fn a_refused_copy_chord_types_on_every_repeat() {
        let sc =
            Shortcuts::default().with_direct_chord(KeyCode::KeyC, ctrl(), Shortcut::Copy, false);
        let mut phase = LeaderPhase::Idle;
        let c = || Key::Character("c".into());
        let events = [press(KeyCode::KeyC, c()), press_repeat(KeyCode::KeyC, c())];
        let effects = run(
            &mut phase,
            &sc,
            &ResolvedViModeKeys::default(),
            &events,
            ctx(ctrl(), ms(0)),
        );
        let typed = KeyEffect::Type {
            logical: c(),
            key_code: KeyCode::KeyC,
            mods: ctrl(),
        };
        assert_eq!(effects, vec![typed.clone(), typed]);
    }

    /// Asserts that a copy chord refused for lack of a selection is typed
    /// after a pending leader.
    ///
    /// Case: a user taps the leader by accident and then presses `Ctrl+C`
    /// with nothing selected.
    #[test]
    fn a_refused_copy_after_an_abandoned_leader_is_typed() {
        let sc = test_shortcuts_with_repeat_prefix(KeyCode::KeyS, Shortcut::EnterViMode, ms(500))
            .with_direct_chord(KeyCode::KeyC, ctrl(), Shortcut::Copy, false);
        let mut phase = LeaderPhase::Pending;
        let events = [press(KeyCode::KeyC, Key::Character("c".into()))];
        let effects = run(
            &mut phase,
            &sc,
            &ResolvedViModeKeys::default(),
            &events,
            ctx(ctrl(), ms(0)),
        );
        assert_eq!(
            effects,
            vec![KeyEffect::Type {
                logical: Key::Character("c".into()),
                key_code: KeyCode::KeyC,
                mods: ctrl(),
            }]
        );
    }

    /// Asserts that a forward chord pressed while the leader is pending is
    /// forwarded on a focused webview.
    ///
    /// Case: a page holds the keyboard, the user taps the leader by accident,
    /// and then presses the program's `Alt+h` forward key.
    #[test]
    fn a_forward_chord_after_an_abandoned_leader_is_forwarded() {
        let sc = test_shortcuts_with_repeat_prefix(KeyCode::KeyS, Shortcut::EnterViMode, ms(500));
        let chords = [NormalizedChord {
            key: ChordKey::Code(KeyCode::KeyH),
            alt: true,
            ctrl: false,
            shift: false,
            logo: false,
        }];
        let mut phase = LeaderPhase::Pending;
        let events = [press(KeyCode::KeyH, Key::Character("h".into()))];
        let classified = run_full(
            &mut phase,
            &sc,
            &ResolvedViModeKeys::default(),
            &events,
            BatchContext {
                webview_focused: true,
                forward_chords: &chords,
                ..ctx(alt(), ms(0))
            },
        );
        assert_eq!(
            classified.effects,
            vec![KeyEffect::Type {
                logical: Key::Character("h".into()),
                key_code: KeyCode::KeyH,
                mods: alt(),
            }]
        );
        assert_eq!(classified.webview_suppressed, vec![KeyCode::KeyH]);
    }

    /// Asserts that holding an unmarked direct chord right after a stray
    /// leader tap fires it once, types nothing, and clears the pending leader.
    ///
    /// Case: on Windows a user taps Alt alone by accident and then holds
    /// `Alt+p`.
    #[test]
    fn a_held_direct_chord_after_a_stray_tap_fires_once() {
        let sc = test_shortcuts_with_repeat_prefix(KeyCode::KeyS, Shortcut::EnterViMode, ms(500))
            .with_direct_chord(KeyCode::KeyP, alt(), Shortcut::KillPane, false);
        let mut phase = LeaderPhase::Pending;
        let p = || Key::Character("p".into());
        let events = [
            press(KeyCode::KeyP, p()),
            press_repeat(KeyCode::KeyP, p()),
            press_repeat(KeyCode::KeyP, p()),
        ];
        let effects = run(
            &mut phase,
            &sc,
            &ResolvedViModeKeys::default(),
            &events,
            ctx(alt(), ms(0)),
        );
        assert_eq!(
            effects,
            vec![KeyEffect::Shortcut {
                action: Shortcut::KillPane,
                via_leader: false,
            }]
        );
        assert_eq!(phase, LeaderPhase::Idle);
    }

    /// Asserts that a held `r:` direct chord keeps firing in vi mode instead
    /// of resolving as a vi-mode key.
    ///
    /// Case: a user in vi mode holds `Alt+Shift+H` to widen the pane they are
    /// reading.
    #[test]
    fn a_repeat_marked_direct_chord_keeps_firing_in_vi_mode() {
        let sc = Shortcuts::default().with_direct_chord(
            KeyCode::KeyH,
            alt_shift(),
            Shortcut::ResizePane(PaneDirection::Left),
            true,
        );
        let mut phase = LeaderPhase::Idle;
        let h = || Key::Character("H".into());
        let events = [press(KeyCode::KeyH, h()), press_repeat(KeyCode::KeyH, h())];
        let effects = run(
            &mut phase,
            &sc,
            &ResolvedViModeKeys::default(),
            &events,
            BatchContext {
                in_vi_mode: true,
                ..ctx(alt_shift(), ms(0))
            },
        );
        assert_eq!(
            effects,
            vec![
                KeyEffect::Shortcut {
                    action: Shortcut::ResizePane(PaneDirection::Left),
                    via_leader: false,
                };
                2
            ]
        );
    }
}
