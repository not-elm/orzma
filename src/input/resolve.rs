//! Pure decision layer for keyboard-shortcut dispatch: the `KeyEffect`
//! intermediate representation plus the single decider `classify_key_batch`
//! that the Default and tmux keyboard dispatchers wire into. No ECS handles —
//! fully unit-testable without a Bevy `App`.

use crate::action::vi::ResolvedCopyModeKeys;
use crate::input::shortcuts::{LeaderPhase, LeaderStep, Shortcuts, is_modifier_key, step_leader};
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyCode, KeyboardInput};
use ozma_webview::NormalizedChord;
use ozmux_configs::copy_mode::CopyModeAction;
use ozmux_configs::shortcuts::{Modifiers, ShortcutAction};
use std::time::Duration;

/// One decided effect of a single pressed key, produced by `classify_key_batch`.
/// Mode-specific appliers (the Default and tmux dispatchers) interpret each
/// variant; this type carries no ECS handles so the decider stays pure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum KeyEffect {
    /// Run a bound `ShortcutAction`. `via_leader` distinguishes a leader-scoped
    /// firing from a direct GUI chord — appliers suppress a different subset
    /// of each (e.g. a direct `Paste` fires in copy mode, a leader `Paste`
    /// does not).
    Action {
        /// The action to run.
        action: ShortcutAction,
        /// Whether the action was reached through the leader (prefix table)
        /// rather than a direct chord.
        via_leader: bool,
    },
    /// Run a matched `[copy-mode]` key.
    CopyMode(CopyModeAction),
    /// Type the key into the focused terminal (Default: the PTY directly;
    /// tmux: forwarded as a `send-keys`).
    Type {
        /// The logical key, for text/printable-key mapping.
        logical: Key,
        /// The physical key, for named-key mapping.
        key_code: KeyCode,
    },
    /// Forward the key to the focused webview's declared forward-key chord.
    WebviewForward {
        /// The logical key, for text/printable-key mapping.
        logical: Key,
        /// The physical key, for named-key mapping.
        key_code: KeyCode,
    },
}

/// Per-batch context `classify_key_batch` needs beyond the leader/shortcut
/// state threaded through `leader_phase`.
pub(crate) struct BatchContext<'a> {
    /// The frame's modifier snapshot, shared by every event in the batch.
    pub(crate) mods: Modifiers,
    /// The caller's `Time<Real>::elapsed()`, for the repeat-window deadline.
    pub(crate) now: Duration,
    /// Whether the focused terminal is currently in copy mode.
    pub(crate) in_copy_mode: bool,
    /// Whether a webview currently owns the keyboard.
    pub(crate) webview_focused: bool,
    /// The focused webview's declared forward-key chords (empty when none).
    pub(crate) forward_chords: &'a [NormalizedChord],
}

/// The decided output of `classify_key_batch`: the per-key `KeyEffect`s, plus the
/// physical keys the leader claimed while a webview owned the keyboard. The caller
/// applies the frame's modifier snapshot when withholding `webview_suppressed`
/// from CEF via `CefKeyboardFilter`; it is empty on the non-webview path.
pub(crate) struct ClassifiedKeys {
    pub(crate) effects: Vec<KeyEffect>,
    pub(crate) webview_suppressed: Vec<KeyCode>,
}

/// Classifies one frame's pressed `KeyboardInput` events into `KeyEffect`s,
/// threading the shared leader state machine across the batch. Pure: no ECS
/// handles, so callers can drive it in a unit test with no `App`.
///
/// A stale repeat window is closed before the batch is processed whenever
/// `ctx.in_copy_mode` is set, so a repeat-marked key that doubles as a
/// copy-mode key resolves against `resolved_copy` instead of re-firing its
/// leader-scoped action.
pub(crate) fn classify_key_batch<'a>(
    leader_phase: &mut LeaderPhase,
    shortcuts: &Shortcuts,
    resolved_copy: &ResolvedCopyModeKeys,
    events: impl Iterator<Item = &'a KeyboardInput>,
    ctx: BatchContext<'a>,
) -> ClassifiedKeys {
    // NOTE: an open repeat window must not intercept copy-mode keys — a
    // repeat-marked key doubling as a copy-mode key would re-fire its bound
    // action into the hidden live terminal instead of being resolved as a
    // copy-mode key below. Close the window before the batch is processed
    // (tmux/Default parity).
    if ctx.in_copy_mode && matches!(*leader_phase, LeaderPhase::Repeat { .. }) {
        *leader_phase = LeaderPhase::Idle;
    }
    let mut effects = Vec::new();
    let mut webview_suppressed = Vec::new();
    for ev in events.filter(|ev| ev.state == ButtonState::Pressed) {
        if ctx.webview_focused {
            // NOTE: the leader runs even while a webview owns the keyboard, so
            // `<Leader>` shortcuts work regardless of focus. Keys the leader
            // claims (the leader chord itself, an abandoned second key, or a
            // fired binding) are recorded in `webview_suppressed` so the caller
            // withholds them from CEF; a key the leader does not claim
            // (`Passthrough`) still resolves to release / forward as before.
            match step_with_repeat(leader_phase, shortcuts, ev, ctx.mods, ctx.now) {
                LeaderStep::Swallow => {
                    webview_suppressed.push(ev.key_code);
                }
                LeaderStep::RunAction(action) => {
                    webview_suppressed.push(ev.key_code);
                    effects.push(KeyEffect::Action {
                        action,
                        via_leader: true,
                    });
                }
                LeaderStep::Passthrough => {
                    if let Some(action @ ShortcutAction::ReleaseWebviewFocus) =
                        shortcuts.match_gui_action(ev.key_code, ctx.mods)
                    {
                        effects.push(KeyEffect::Action {
                            action,
                            via_leader: false,
                        });
                    } else if ctx
                        .forward_chords
                        .iter()
                        .any(|chord| chord_matches(chord, ev.key_code, ctx.mods))
                    {
                        effects.push(KeyEffect::WebviewForward {
                            logical: ev.logical_key.clone(),
                            key_code: ev.key_code,
                        });
                    }
                }
            }
            continue;
        }
        let action = match step_with_repeat(leader_phase, shortcuts, ev, ctx.mods, ctx.now) {
            LeaderStep::Swallow => continue,
            LeaderStep::RunAction(action) => Some((action, true)),
            LeaderStep::Passthrough => shortcuts
                .match_gui_action(ev.key_code, ctx.mods)
                .map(|action| (action, false)),
        };
        if let Some((action, via_leader)) = action {
            effects.push(KeyEffect::Action { action, via_leader });
            continue;
        }
        // NOTE: copy-mode keys resolve only after leader and GUI-shortcut
        // dispatch declined the key, and a copy-mode key never falls through
        // to Type — an unmatched key in copy mode is swallowed, not typed
        // (tmux/Default parity).
        if ctx.in_copy_mode {
            if let Some(copy_action) = resolved_copy.resolve(&ev.logical_key, ev.key_code, ctx.mods)
            {
                effects.push(KeyEffect::CopyMode(copy_action));
            }
            continue;
        }
        if is_modifier_key(ev.key_code) || ctx.mods.meta {
            continue;
        }
        effects.push(KeyEffect::Type {
            logical: ev.logical_key.clone(),
            key_code: ev.key_code,
        });
    }
    ClassifiedKeys {
        effects,
        webview_suppressed,
    }
}

/// Advances the leader machine for one pressed event, honoring OS auto-repeat:
/// outside the repeat window a repeat event does NOT step the machine.
fn step_with_repeat(
    leader_phase: &mut LeaderPhase,
    shortcuts: &Shortcuts,
    ev: &KeyboardInput,
    mods: Modifiers,
    now: Duration,
) -> LeaderStep {
    if ev.repeat {
        match *leader_phase {
            LeaderPhase::Pending => LeaderStep::Swallow,
            LeaderPhase::Repeat { .. } => {
                step_leader(leader_phase, shortcuts, ev.key_code, mods, now)
            }
            LeaderPhase::Idle => LeaderStep::Passthrough,
        }
    } else {
        step_leader(leader_phase, shortcuts, ev.key_code, mods, now)
    }
}

/// True when `chord` (a focused webview's declared forward-key entry) matches
/// the physical key and exact modifier set of a pressed event.
fn chord_matches(chord: &NormalizedChord, key_code: KeyCode, mods: Modifiers) -> bool {
    chord.code == key_code
        && chord.ctrl == mods.ctrl
        && chord.shift == mods.shift
        && chord.alt == mods.alt
        && chord.logo == mods.meta
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::shortcuts::{
        test_shortcuts_with_direct_chord, test_shortcuts_with_repeat_prefix,
    };
    use bevy::prelude::Entity;

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
            mods,
            now,
            in_copy_mode: false,
            webview_focused: false,
            forward_chords: &[],
        }
    }

    fn run<'a>(
        leader_phase: &mut LeaderPhase,
        shortcuts: &Shortcuts,
        resolved_copy: &ResolvedCopyModeKeys,
        events: &'a [KeyboardInput],
        ctx: BatchContext<'a>,
    ) -> Vec<KeyEffect> {
        classify_key_batch(leader_phase, shortcuts, resolved_copy, events.iter(), ctx).effects
    }

    fn run_full<'a>(
        leader_phase: &mut LeaderPhase,
        shortcuts: &Shortcuts,
        resolved_copy: &ResolvedCopyModeKeys,
        events: &'a [KeyboardInput],
        ctx: BatchContext<'a>,
    ) -> ClassifiedKeys {
        classify_key_batch(leader_phase, shortcuts, resolved_copy, events.iter(), ctx)
    }

    #[test]
    fn leader_press_swallows_and_no_type() {
        let sc = test_shortcuts_with_repeat_prefix(
            KeyCode::KeyS,
            ShortcutAction::EnterCopyMode,
            ms(500),
        );
        let resolved_copy = ResolvedCopyModeKeys::default();
        let mut phase = LeaderPhase::Idle;
        let events = [press(KeyCode::KeyA, Key::Character("a".into()))];
        let effects = run(
            &mut phase,
            &sc,
            &resolved_copy,
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
        let sc = test_shortcuts_with_repeat_prefix(
            KeyCode::KeyS,
            ShortcutAction::EnterCopyMode,
            Duration::ZERO,
        );
        let resolved_copy = ResolvedCopyModeKeys::default();
        let mut phase = LeaderPhase::Pending;
        let events = [press(KeyCode::KeyS, Key::Character("s".into()))];
        let effects = run(
            &mut phase,
            &sc,
            &resolved_copy,
            &events,
            ctx(no_mods(), ms(0)),
        );
        assert_eq!(
            effects,
            vec![KeyEffect::Action {
                action: ShortcutAction::EnterCopyMode,
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
            ShortcutAction::Quit,
        );
        let resolved_copy = ResolvedCopyModeKeys::default();
        let mut phase = LeaderPhase::Idle;
        let events = [press(KeyCode::KeyQ, Key::Character("q".into()))];
        let effects = run(
            &mut phase,
            &sc,
            &resolved_copy,
            &events,
            ctx(mods(false, false, false, true), ms(0)),
        );
        assert_eq!(
            effects,
            vec![KeyEffect::Action {
                action: ShortcutAction::Quit,
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
        let resolved_copy = ResolvedCopyModeKeys::default();
        let mut phase = LeaderPhase::Idle;
        let events = [press(KeyCode::KeyA, Key::Character("a".into()))];
        let effects = run(
            &mut phase,
            &sc,
            &resolved_copy,
            &events,
            ctx(no_mods(), ms(0)),
        );
        assert_eq!(
            effects,
            vec![KeyEffect::Type {
                logical: Key::Character("a".into()),
                key_code: KeyCode::KeyA,
            }]
        );
    }

    #[test]
    fn repeat_window_refires_on_os_repeat() {
        let sc = test_shortcuts_with_repeat_prefix(
            KeyCode::KeyH,
            ShortcutAction::EnterCopyMode,
            ms(500),
        );
        let resolved_copy = ResolvedCopyModeKeys::default();
        let mut phase = LeaderPhase::Repeat { deadline: ms(500) };
        let events = [press_repeat(KeyCode::KeyH, Key::Character("h".into()))];
        let effects = run(
            &mut phase,
            &sc,
            &resolved_copy,
            &events,
            ctx(no_mods(), ms(100)),
        );
        assert_eq!(
            effects,
            vec![KeyEffect::Action {
                action: ShortcutAction::EnterCopyMode,
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
        let sc = test_shortcuts_with_repeat_prefix(
            KeyCode::KeyH,
            ShortcutAction::EnterCopyMode,
            ms(500),
        );
        let resolved_copy = ResolvedCopyModeKeys::default();
        let mut phase = LeaderPhase::Idle;
        let events = [press_repeat(KeyCode::KeyH, Key::Character("h".into()))];
        let effects = run(
            &mut phase,
            &sc,
            &resolved_copy,
            &events,
            ctx(no_mods(), ms(0)),
        );
        assert_eq!(
            effects,
            vec![KeyEffect::Type {
                logical: Key::Character("h".into()),
                key_code: KeyCode::KeyH,
            }],
            "an auto-repeat outside the window must not step the leader machine"
        );
        assert_eq!(phase, LeaderPhase::Idle);
    }

    #[test]
    fn pending_skips_bare_modifier_then_second_key() {
        let sc = test_shortcuts_with_repeat_prefix(
            KeyCode::KeyD,
            ShortcutAction::DetachSession,
            Duration::ZERO,
        );
        let resolved_copy = ResolvedCopyModeKeys::default();
        let mut phase = LeaderPhase::Pending;
        let events = [
            press(KeyCode::ControlLeft, Key::Control),
            press(KeyCode::KeyD, Key::Character("d".into())),
        ];
        let effects = run(
            &mut phase,
            &sc,
            &resolved_copy,
            &events,
            ctx(no_mods(), ms(0)),
        );
        assert_eq!(
            effects,
            vec![KeyEffect::Action {
                action: ShortcutAction::DetachSession,
                via_leader: true,
            }],
            "the leading bare modifier must not consume the pending slot; the real \
             second key must resolve"
        );
        assert_eq!(phase, LeaderPhase::Idle);
    }

    #[test]
    fn pending_suppresses_type_for_second_key() {
        let sc = test_shortcuts_with_repeat_prefix(
            KeyCode::KeyZ,
            ShortcutAction::DetachSession,
            ms(500),
        );
        let resolved_copy = ResolvedCopyModeKeys::default();
        let mut phase = LeaderPhase::Pending;
        let events = [press(KeyCode::KeyA, Key::Character("a".into()))];
        let effects = run(
            &mut phase,
            &sc,
            &resolved_copy,
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
        let sc = test_shortcuts_with_repeat_prefix(
            KeyCode::KeyZ,
            ShortcutAction::DetachSession,
            ms(500),
        );
        let resolved_copy = ResolvedCopyModeKeys::default();
        let mut phase = LeaderPhase::Pending;
        let events = [
            press(KeyCode::KeyA, Key::Character("a".into())),
            press(KeyCode::KeyB, Key::Character("b".into())),
        ];
        let effects = run(
            &mut phase,
            &sc,
            &resolved_copy,
            &events,
            ctx(no_mods(), ms(0)),
        );
        assert_eq!(
            effects,
            vec![KeyEffect::Type {
                logical: Key::Character("b".into()),
                key_code: KeyCode::KeyB,
            }],
            "a trailing same-frame key after the suppressed second key must be typed"
        );
    }

    #[test]
    fn repeat_window_withholds_matching_key() {
        let sc = test_shortcuts_with_repeat_prefix(
            KeyCode::KeyH,
            ShortcutAction::EnterCopyMode,
            ms(60_000),
        );
        let resolved_copy = ResolvedCopyModeKeys::default();
        let mut phase = LeaderPhase::Repeat {
            deadline: ms(60_000),
        };
        let events = [press_repeat(KeyCode::KeyH, Key::Character("h".into()))];
        let effects = run(
            &mut phase,
            &sc,
            &resolved_copy,
            &events,
            ctx(no_mods(), ms(0)),
        );
        assert!(
            !effects.iter().any(|e| matches!(e, KeyEffect::Type { .. })),
            "a repeat-marked key inside the window must never also emit Type"
        );
        assert_eq!(
            effects,
            vec![KeyEffect::Action {
                action: ShortcutAction::EnterCopyMode,
                via_leader: true,
            }],
            "the action must fire — this is not an empty Vec"
        );
    }

    #[test]
    fn repeat_window_types_non_matching_key() {
        let sc = test_shortcuts_with_repeat_prefix(
            KeyCode::KeyH,
            ShortcutAction::EnterCopyMode,
            ms(60_000),
        );
        let resolved_copy = ResolvedCopyModeKeys::default();
        let mut phase = LeaderPhase::Repeat {
            deadline: ms(60_000),
        };
        let events = [press(KeyCode::KeyB, Key::Character("b".into()))];
        let effects = run(
            &mut phase,
            &sc,
            &resolved_copy,
            &events,
            ctx(no_mods(), ms(0)),
        );
        assert_eq!(
            effects,
            vec![KeyEffect::Type {
                logical: Key::Character("b".into()),
                key_code: KeyCode::KeyB,
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
        let sc = test_shortcuts_with_repeat_prefix(
            KeyCode::KeyH,
            ShortcutAction::EnterCopyMode,
            ms(60_000),
        );
        let resolved_copy = ResolvedCopyModeKeys::default();
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
            &resolved_copy,
            &events,
            ctx(no_mods(), ms(0)),
        );
        assert_eq!(
            effects,
            vec![
                KeyEffect::Type {
                    logical: Key::Character("b".into()),
                    key_code: KeyCode::KeyB,
                },
                KeyEffect::Type {
                    logical: Key::Character("h".into()),
                    key_code: KeyCode::KeyH,
                },
            ],
            "the non-matching key closes the window for the rest of the frame; the \
             repeat key after it must be typed, not withheld"
        );
    }

    #[test]
    fn release_webview_chord_emits_type_no_webview() {
        let sc = Shortcuts::default();
        let resolved_copy = ResolvedCopyModeKeys::default();
        let mut phase = LeaderPhase::Idle;
        let events = [press(KeyCode::Escape, Key::Escape)];
        let effects = run(
            &mut phase,
            &sc,
            &resolved_copy,
            &events,
            ctx(mods(true, true, false, false), ms(0)),
        );
        assert_eq!(
            effects,
            vec![KeyEffect::Type {
                logical: Key::Escape,
                key_code: KeyCode::Escape,
            }],
            "the decider must not special-case the release-webview-focus chord when \
             no webview is focused; the Default applier drops it, tmux forwards it"
        );
    }

    #[test]
    fn no_type_while_in_copy_mode() {
        let sc = Shortcuts::default();
        let resolved_copy = ResolvedCopyModeKeys::default();
        let mut phase = LeaderPhase::Idle;
        let events = [press(KeyCode::KeyX, Key::Character("x".into()))];
        let mut c = ctx(no_mods(), ms(0));
        c.in_copy_mode = true;
        let effects = run(&mut phase, &sc, &resolved_copy, &events, c);
        assert!(
            !effects.iter().any(|e| matches!(e, KeyEffect::Type { .. })),
            "an unmatched key in copy mode must never fall through to Type"
        );
    }

    #[test]
    fn copy_key_shadowed_by_gui() {
        let sc = test_shortcuts_with_direct_chord(
            KeyCode::KeyV,
            no_mods(),
            ShortcutAction::EnterCopyMode,
        );
        let resolved_copy = ResolvedCopyModeKeys::default();
        let mut phase = LeaderPhase::Idle;
        let events = [press(KeyCode::KeyV, Key::Character("v".into()))];
        let mut c = ctx(no_mods(), ms(0));
        c.in_copy_mode = true;
        let effects = run(&mut phase, &sc, &resolved_copy, &events, c);
        assert_eq!(
            effects,
            vec![KeyEffect::Action {
                action: ShortcutAction::EnterCopyMode,
                via_leader: false,
            }],
            "a bound GUI chord must shadow a copy-mode key, not resolve as CopyMode"
        );
    }

    #[test]
    fn meta_unmatched_dropped() {
        let sc = Shortcuts::default();
        let resolved_copy = ResolvedCopyModeKeys::default();
        let mut phase = LeaderPhase::Idle;
        let events = [press(KeyCode::KeyJ, Key::Character("j".into()))];
        let effects = run(
            &mut phase,
            &sc,
            &resolved_copy,
            &events,
            ctx(mods(false, false, false, true), ms(0)),
        );
        assert_eq!(effects, vec![], "Cmd+J must not reach the terminal");
    }

    #[test]
    fn direct_paste_suppressed_in_copy_mode() {
        let sc = test_shortcuts_with_direct_chord(
            KeyCode::KeyV,
            mods(false, false, false, true),
            ShortcutAction::Paste,
        );
        let resolved_copy = ResolvedCopyModeKeys::default();
        let mut phase = LeaderPhase::Idle;
        let events = [press(KeyCode::KeyV, Key::Character("v".into()))];
        let mut c = ctx(mods(false, false, false, true), ms(0));
        c.in_copy_mode = true;
        let effects = run(&mut phase, &sc, &resolved_copy, &events, c);
        assert_eq!(
            effects,
            vec![KeyEffect::Action {
                action: ShortcutAction::Paste,
                via_leader: false,
            }],
            "the decider still emits the direct paste action; the Default \
             applier is what suppresses it in copy mode (via `via_leader || \
             !in_copy_mode`), so a direct paste never fires while copy mode is \
             active"
        );
    }

    #[test]
    fn leader_paste_fires_in_copy_mode() {
        let sc =
            test_shortcuts_with_repeat_prefix(KeyCode::KeyP, ShortcutAction::Paste, Duration::ZERO);
        let resolved_copy = ResolvedCopyModeKeys::default();
        let mut phase = LeaderPhase::Pending;
        let events = [press(KeyCode::KeyP, Key::Character("p".into()))];
        let mut c = ctx(no_mods(), ms(0));
        c.in_copy_mode = true;
        let effects = run(&mut phase, &sc, &resolved_copy, &events, c);
        assert_eq!(
            effects,
            vec![KeyEffect::Action {
                action: ShortcutAction::Paste,
                via_leader: true,
            }]
        );
    }

    #[test]
    fn in_copy_mode_closes_stale_repeat_window_before_dispatch() {
        // Discriminates the pre-loop guard specifically: an AUTO-REPEAT of the
        // repeat-bound key (KeyH) inside an open window. Without the guard,
        // `step_with_repeat` sees `ev.repeat && LeaderPhase::Repeat` and calls
        // `step_leader`, which re-fires `Action{EnterCopyMode}` — `step_leader`'s
        // own Repeat arm does NOT close the window here because the key MATCHES.
        // The pre-loop guard is the only thing that forces the phase to Idle so
        // the same key resolves to copy-mode (unbound → nothing) instead.
        let sc = test_shortcuts_with_repeat_prefix(
            KeyCode::KeyH,
            ShortcutAction::EnterCopyMode,
            ms(60_000),
        );
        let resolved_copy = ResolvedCopyModeKeys::default();
        let mut phase = LeaderPhase::Repeat {
            deadline: ms(60_000),
        };
        let events = [press_repeat(KeyCode::KeyH, Key::Character("h".into()))];
        let mut c = ctx(no_mods(), ms(0));
        c.in_copy_mode = true;
        let effects = run(&mut phase, &sc, &resolved_copy, &events, c);
        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, KeyEffect::Action { .. })),
            "the stale repeat window must be closed before dispatch, so a \
             repeat-marked key in copy mode must NOT re-fire its bound action"
        );
        assert_eq!(
            phase,
            LeaderPhase::Idle,
            "the pre-loop guard must close the window before the batch"
        );
    }

    #[test]
    fn webview_pending_leader_fires_action_and_suppresses() {
        let sc = test_shortcuts_with_repeat_prefix(
            KeyCode::KeyS,
            ShortcutAction::EnterCopyMode,
            Duration::ZERO,
        );
        let resolved_copy = ResolvedCopyModeKeys::default();
        let mut phase = LeaderPhase::Pending;
        let events = [press(KeyCode::KeyS, Key::Character("s".into()))];
        let mut c = ctx(no_mods(), ms(0));
        c.webview_focused = true;
        let out = run_full(&mut phase, &sc, &resolved_copy, &events, c);
        assert_eq!(
            out.effects,
            vec![KeyEffect::Action {
                action: ShortcutAction::EnterCopyMode,
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
        let sc = test_shortcuts_with_repeat_prefix(
            KeyCode::KeyS,
            ShortcutAction::EnterCopyMode,
            Duration::ZERO,
        );
        let resolved_copy = ResolvedCopyModeKeys::default();
        let mut phase = LeaderPhase::Idle;
        let events = [press(KeyCode::KeyA, Key::Character("a".into()))];
        let mut c = ctx(mods(true, false, false, false), ms(0));
        c.webview_focused = true;
        let out = run_full(&mut phase, &sc, &resolved_copy, &events, c);
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
        let sc = test_shortcuts_with_repeat_prefix(
            KeyCode::KeyS,
            ShortcutAction::EnterCopyMode,
            Duration::ZERO,
        );
        let resolved_copy = ResolvedCopyModeKeys::default();
        let mut phase = LeaderPhase::Pending;
        let events = [press(KeyCode::KeyZ, Key::Character("z".into()))];
        let mut c = ctx(no_mods(), ms(0));
        c.webview_focused = true;
        let out = run_full(&mut phase, &sc, &resolved_copy, &events, c);
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
        let resolved_copy = ResolvedCopyModeKeys::default();
        let mut phase = LeaderPhase::Idle;
        let events = [press(KeyCode::KeyB, Key::Character("b".into()))];
        let mut c = ctx(no_mods(), ms(0));
        c.webview_focused = true;
        let out = run_full(&mut phase, &sc, &resolved_copy, &events, c);
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

    #[test]
    fn webview_idle_forward_chord_forwards_and_not_suppressed() {
        let sc = Shortcuts::default();
        let resolved_copy = ResolvedCopyModeKeys::default();
        let mut phase = LeaderPhase::Idle;
        let chords = [NormalizedChord {
            code: KeyCode::KeyK,
            alt: false,
            ctrl: false,
            shift: false,
            logo: false,
        }];
        let events = [press(KeyCode::KeyK, Key::Character("k".into()))];
        let mut c = ctx(no_mods(), ms(0));
        c.webview_focused = true;
        c.forward_chords = &chords;
        let out = run_full(&mut phase, &sc, &resolved_copy, &events, c);
        assert_eq!(
            out.effects,
            vec![KeyEffect::WebviewForward {
                logical: Key::Character("k".into()),
                key_code: KeyCode::KeyK,
            }],
            "with no leader engaged a declared forward chord still forwards"
        );
        assert!(
            out.webview_suppressed.is_empty(),
            "a forward chord is not a leader claim — not withheld from CEF"
        );
        assert_eq!(phase, LeaderPhase::Idle);
    }

    #[test]
    fn webview_release_chord_emits_action() {
        let sc = test_shortcuts_with_direct_chord(
            KeyCode::Escape,
            mods(true, true, false, false),
            ShortcutAction::ReleaseWebviewFocus,
        );
        let resolved_copy = ResolvedCopyModeKeys::default();
        let mut phase = LeaderPhase::Idle;
        let events = [press(KeyCode::Escape, Key::Escape)];
        let mut c = ctx(mods(true, true, false, false), ms(0));
        c.webview_focused = true;
        let effects = run(&mut phase, &sc, &resolved_copy, &events, c);
        assert_eq!(
            effects,
            vec![KeyEffect::Action {
                action: ShortcutAction::ReleaseWebviewFocus,
                via_leader: false,
            }],
            "with a webview focused the release chord resolves as a normal action"
        );
    }

    #[test]
    fn release_chord_without_webview_emits_action_not_type() {
        let sc = test_shortcuts_with_direct_chord(
            KeyCode::Escape,
            mods(true, true, false, false),
            ShortcutAction::ReleaseWebviewFocus,
        );
        let resolved_copy = ResolvedCopyModeKeys::default();
        let mut phase = LeaderPhase::Idle;
        let events = [press(KeyCode::Escape, Key::Escape)];
        let c = ctx(mods(true, true, false, false), ms(0));
        let effects = run(&mut phase, &sc, &resolved_copy, &events, c);
        assert_eq!(
            effects,
            vec![KeyEffect::Action {
                action: ShortcutAction::ReleaseWebviewFocus,
                via_leader: false,
            }],
            "with no webview focused the release chord still resolves as an action, never a Type"
        );
    }
}
