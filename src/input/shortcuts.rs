//! Resolves configured shortcut chords (logical keys) into physical
//! `KeyCode`-based entries the runtime input dispatcher matches against.
//! The translation lives here (not in `ozmux_configs`) so the config crate
//! stays free of any `bevy` dependency.

use crate::configs::OzmuxConfigsResource;
use crate::input::bindings::{FineModifier, OzmaMouseConfig, ReservedChord, TerminalInputBindings};
use crate::mode::AppMode;
use bevy::input::ButtonState;
use bevy::input::keyboard::KeyboardInput;
use bevy::input::mouse::MouseButton;
use bevy::prelude::*;
use bevy::time::Real;
use bevy::window::PrimaryWindow;
use bevy_cef::prelude::FocusedWebview;
use ozma_tty_engine::{ButtonConfig, WheelConfig};
use ozmux_configs::mouse::{FineModifier as CfgFineModifier, MouseConfig};
use ozmux_configs::shortcuts::{
    Key as ConfigKey, KeyChord, Leader, Modifiers, ShortcutAction, TapModifier,
};
use std::time::Duration;

pub(super) struct ShortcutsPlugin;

impl Plugin for ShortcutsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Shortcuts>()
            .init_resource::<LeaderPhase>()
            .init_resource::<ModifierTapState>()
            .configure_sets(
                Update,
                (LeaderGate::Detect, LeaderGate::Read, LeaderGate::Advance).chain(),
            )
            .add_systems(
                Startup,
                (
                    build_shortcuts,
                    populate_input_bindings,
                    populate_mouse_config,
                )
                    .chain(),
            )
            .add_systems(
                Update,
                (
                    // NOTE: intentionally not gated on `on_message::<KeyboardInput>` — must run
                    // on keyboard-less frames so a mouse press (e.g. Cmd+click mid-tap) can
                    // disarm `state.armed`; gating it on `on_message` would silently break it.
                    detect_modifier_tap
                        .in_set(LeaderGate::Detect)
                        .run_if(tap_leader_enabled),
                    // NOTE: webview focus moves on mouse clicks (no `KeyboardInput`), so the
                    // keyboard dispatchers never see the round-trip; without this reset a
                    // leader engaged before a mouse-only webview focus/blur would consume the
                    // next terminal keystroke as its second key.
                    reset_leader_phase
                        .run_if(resource_exists_and_changed::<FocusedWebview>)
                        .before(LeaderGate::Detect),
                ),
            )
            .add_systems(OnExit(AppMode::Tmux), reset_leader_phase)
            .add_systems(OnExit(AppMode::Default), reset_leader_phase);
    }
}

/// Shared leader phase: where the leader state machine is between keys.
/// Owned by `ShortcutsPlugin`; advanced by the tmux and Default keyboard
/// dispatchers and read by `dispatch_input` to withhold leader-consumed keys
/// from PTY typing.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LeaderPhase {
    /// No leader sequence in progress.
    #[default]
    Idle,
    /// A leader fired; the next key resolves against the prefix table.
    Pending,
    /// A `<Leader:r>` binding fired; repeat-marked keys re-fire until the
    /// deadline (extended on each fire) or a non-matching key closes it.
    Repeat {
        /// Absolute `Time<Real>::elapsed()` instant the window closes at.
        deadline: Duration,
    },
}

/// In-progress modifier-tap state: the target's down-time while armed.
#[derive(Resource, Default)]
struct ModifierTapState {
    armed: Option<Duration>,
}

/// Orders the three `FocusedKey` systems that touch `LeaderPhase` so
/// `detect_modifier_tap` (`Detect`) sets it before `dispatch_input`
/// (`Read`) observes it, and `app_shortcut_handler` (`Advance`) steps the
/// leader machine and clears it last.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum LeaderGate {
    /// `detect_modifier_tap`: sets `LeaderPhase` to `Pending` on a modifier tap.
    Detect,
    /// `dispatch_input`: reads `LeaderPhase` to gate PTY typing.
    Read,
    /// `app_shortcut_handler` / `forward_keys_to_tmux`: advances the leader.
    Advance,
}

/// The leader resolved to runtime form: a physical chord, or a modifier tap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResolvedLeader {
    /// Chord leader: exact `(KeyCode, Modifiers)` to match.
    Chord(KeyCode, Modifiers),
    /// Modifier-tap leader: the bare modifier to detect a tap on.
    Tap(TapModifier),
}

/// One configured shortcut resolved to a physical key: the `KeyCode` to match,
/// the exact modifier set required, and the action to run.
#[derive(Debug, Clone, PartialEq, Eq)]
struct OzmuxShortcut {
    keycode: KeyCode,
    modifiers: Modifiers,
    action: ShortcutAction,
    repeat: bool,
}

/// The startup-resolved ozmux shortcut tables. Built once from
/// `OzmuxConfigsResource`; consumed by the tmux and Default keyboard
/// dispatchers.
#[derive(Resource, Default, Debug, Clone)]
pub(crate) struct Shortcuts {
    direct: Vec<OzmuxShortcut>,
    prefix: Vec<OzmuxShortcut>,
    leader: Option<ResolvedLeader>,
    tap_timeout: Duration,
    repeat_time: Duration,
}

impl Shortcuts {
    /// Returns the GUI action bound to `(keycode, mods)` in the direct table, if
    /// any. Excludes `ReleaseWebviewFocus` (matched via `is_release_webview_focus`).
    pub(crate) fn match_gui_action(
        &self,
        keycode: KeyCode,
        mods: Modifiers,
    ) -> Option<ShortcutAction> {
        Self::find_entry(&self.direct, keycode, mods).map(|s| s.action)
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
        if let Some(ResolvedLeader::Chord(keycode, modifiers)) = self.leader {
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

    /// Returns the leader-scoped action bound to `(keycode, mods)` when the
    /// binding is repeat-marked (`<Leader:r>`). The single predicate shared by
    /// `step_leader` (transition) and `dispatch_input` (PTY withholding) so the
    /// two can never disagree about which keys the repeat window consumes.
    pub(crate) fn match_repeat_prefix(
        &self,
        keycode: KeyCode,
        mods: Modifiers,
    ) -> Option<ShortcutAction> {
        self.match_prefix_entry(keycode, mods)
            .filter(|s| s.repeat)
            .map(|s| s.action)
    }

    /// True when a leader second key `(keycode, mods)` opens the repeat window:
    /// it matches a repeat-marked prefix binding and `repeat_time` is non-zero.
    /// Mirrors the `step_leader` Pending arm so `dispatch_input` can start
    /// withholding in the same frame the window opens.
    pub(crate) fn opens_repeat_window(&self, keycode: KeyCode, mods: Modifiers) -> bool {
        !self.repeat_time.is_zero() && self.match_repeat_prefix(keycode, mods).is_some()
    }

    /// True when `(keycode, mods)` is the configured leader chord.
    fn is_leader(&self, keycode: KeyCode, mods: Modifiers) -> bool {
        matches!(self.leader, Some(ResolvedLeader::Chord(kc, m)) if kc == keycode && m == mods)
    }

    fn tap_modifier(&self) -> Option<TapModifier> {
        match self.leader {
            Some(ResolvedLeader::Tap(m)) => Some(m),
            _ => None,
        }
    }

    /// Returns the prefix-table entry bound to `(keycode, mods)`, excluding
    /// `ReleaseWebviewFocus` (mirrors `match_gui_action`): leader dispatch only
    /// runs when no webview is focused, so a leader-scoped
    /// release-webview-focus could never fire — resolving it to `Swallow`
    /// avoids a dead `RunAction`.
    fn match_prefix_entry(&self, keycode: KeyCode, mods: Modifiers) -> Option<&OzmuxShortcut> {
        Self::find_entry(&self.prefix, keycode, mods)
    }

    /// Returns the first entry in `table` bound to `(keycode, mods)`, excluding
    /// `ReleaseWebviewFocus` (matched separately via `is_release_webview_focus`).
    /// The single exclusion predicate behind both the direct and prefix lookups.
    fn find_entry(
        table: &[OzmuxShortcut],
        keycode: KeyCode,
        mods: Modifiers,
    ) -> Option<&OzmuxShortcut> {
        table.iter().find(|s| {
            s.action != ShortcutAction::ReleaseWebviewFocus
                && s.keycode == keycode
                && s.modifiers == mods
        })
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
/// `phase` across frames. `now` is the caller's `Time<Real>::elapsed()`.
/// Swallows the leader itself and any unmatched second key; drives the
/// `<Leader:r>` repeat window (fire-and-extend inside the window, close and
/// re-evaluate on any other key); returns `Passthrough` for unrelated keys.
pub(crate) fn step_leader(
    phase: &mut LeaderPhase,
    shortcuts: &Shortcuts,
    keycode: KeyCode,
    mods: Modifiers,
    now: Duration,
) -> LeaderStep {
    // NOTE: a bare modifier press must NOT touch the phase. The second chord's
    // modifier (e.g. Ctrl) emits its own `Pressed` event ahead of the main key;
    // stepping on it would consume the pending leader by parity and abort the
    // sequence. Closing the repeat window on it would likewise break
    // modifier-carrying repeat chords (`<Leader:r>Ctrl+H`).
    if is_modifier_key(keycode) {
        return LeaderStep::Passthrough;
    }
    if let LeaderPhase::Repeat { deadline } = *phase {
        if now <= deadline
            && let Some(action) = shortcuts.match_repeat_prefix(keycode, mods)
        {
            *phase = LeaderPhase::Repeat {
                deadline: now + shortcuts.repeat_time,
            };
            return LeaderStep::RunAction(action);
        }
        // NOTE: an expired window or a non-repeat key must close the window and
        // fall through to normal evaluation below — swallowing the key here
        // would eat ordinary typing (tmux re-dispatches the same way).
        *phase = LeaderPhase::Idle;
    }
    if *phase == LeaderPhase::Pending {
        *phase = LeaderPhase::Idle;
        return match shortcuts.match_prefix_entry(keycode, mods) {
            Some(entry) => {
                if entry.repeat && !shortcuts.repeat_time.is_zero() {
                    *phase = LeaderPhase::Repeat {
                        deadline: now + shortcuts.repeat_time,
                    };
                }
                LeaderStep::RunAction(entry.action)
            }
            None => LeaderStep::Swallow,
        };
    }
    if shortcuts.is_leader(keycode, mods) {
        *phase = LeaderPhase::Pending;
        return LeaderStep::Swallow;
    }
    LeaderStep::Passthrough
}

/// Resets the shared leader phase to `Idle`, writing through the `ResMut` only
/// on a real change so Bevy change detection fires exactly when the phase was
/// engaged. The single reset idiom for every dispatcher drain/abort site.
pub(crate) fn clear_leader_phase(leader_phase: &mut ResMut<LeaderPhase>) {
    if **leader_phase != LeaderPhase::Idle {
        **leader_phase = LeaderPhase::Idle;
    }
}

/// Test-only constructor: a `Shortcuts` with one repeat-marked, modifier-less
/// prefix binding and a `Ctrl+A` chord leader. Used by the keyboard and
/// dispatcher tests, which cannot name this module's private fields.
#[cfg(test)]
pub(crate) fn test_shortcuts_with_repeat_prefix(
    keycode: KeyCode,
    action: ShortcutAction,
    repeat_time: Duration,
) -> Shortcuts {
    Shortcuts {
        direct: Vec::new(),
        prefix: vec![OzmuxShortcut {
            keycode,
            modifiers: Modifiers::default(),
            action,
            repeat: true,
        }],
        leader: Some(ResolvedLeader::Chord(
            KeyCode::KeyA,
            Modifiers {
                ctrl: true,
                shift: false,
                alt: false,
                meta: false,
            },
        )),
        tap_timeout: Duration::from_millis(300),
        repeat_time,
    }
}

/// Clears the leader phase (pending or repeat window) and any in-progress
/// modifier tap on an `AppMode` transition or a webview focus change, so a
/// leader engaged/armed in one context never fires in another.
fn reset_leader_phase(mut leader_phase: ResMut<LeaderPhase>, mut tap: ResMut<ModifierTapState>) {
    clear_leader_phase(&mut leader_phase);
    if tap.armed.is_some() {
        tap.armed = None;
    }
}

/// Run condition: only run `detect_modifier_tap` when the leader is a tap.
fn tap_leader_enabled(shortcuts: Res<Shortcuts>) -> bool {
    shortcuts.tap_modifier().is_some()
}

/// Detects a bare modifier tap (press+release within the timeout, no
/// intervening key or mouse press) and engages `LeaderPhase::Pending`.
/// Mode-agnostic; runs in `LeaderGate::Detect` before the dispatchers
/// read/advance the leader.
///
/// Gated with `run_if(tap_leader_enabled)`. Reads press AND release
/// `KeyboardInput` (the dispatchers only read `Pressed`), so it owns the tap
/// gesture; the second key is handled by the existing `step_leader` pending arm.
fn detect_modifier_tap(
    mut state: ResMut<ModifierTapState>,
    mut leader_phase: ResMut<LeaderPhase>,
    mut keys: MessageReader<KeyboardInput>,
    mouse: Res<ButtonInput<MouseButton>>,
    time: Res<Time<Real>>,
    shortcuts: Res<Shortcuts>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    // `run_if(tap_leader_enabled)` guarantees this is `Some`.
    let Some(modifier) = shortcuts.tap_modifier() else {
        return;
    };
    let focused = windows.single().map(|w| w.focused).unwrap_or(false);
    let mut armed = state.armed;
    if !focused || mouse.get_just_pressed().next().is_some() {
        // NOTE: a mouse press anywhere this frame (or lost focus) invalidates the
        // tap gesture — disarm and drain this frame's key events without arming or
        // firing. Draining here (own reader cursor) prevents a same-frame
        // `Cmd-down` + click from arming and firing on the next `Cmd-up`.
        armed = None;
        keys.clear();
    } else {
        let now = time.elapsed();
        let timeout = shortcuts.tap_timeout;
        for ev in keys.read() {
            if ev.repeat {
                continue;
            }
            let event = if is_tap_modifier_key(ev.key_code, modifier) {
                match ev.state {
                    ButtonState::Pressed => TapEvent::TargetDown,
                    ButtonState::Released => TapEvent::TargetUp,
                }
            } else if ev.state == ButtonState::Pressed {
                TapEvent::OtherDown
            } else {
                continue;
            };
            if step_tap(&mut armed, event, now, timeout) == TapOutcome::Fired
                && *leader_phase != LeaderPhase::Pending
            {
                *leader_phase = LeaderPhase::Pending;
            }
        }
    }
    if armed != state.armed {
        state.armed = armed;
    }
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
    resolved.direct = resolve_from_chords(
        sc.direct_chords()
            .map(|(label, chord, action)| (label, chord, action, false)),
    );
    resolved.prefix = resolve_from_chords(sc.leader_chords());
    resolved.tap_timeout = Duration::from_millis(sc.leader_tap_timeout_ms);
    resolved.repeat_time = Duration::from_millis(sc.repeat_time_ms);
    // The leader (default Cmd tap) is only meaningful when there are
    // `<Leader>`-scoped bindings to reach; with none it stays inert, so a
    // default tap never swallows a key for users who bind no leader action.
    resolved.leader = if resolved.prefix.is_empty() {
        None
    } else {
        match sc.leader.as_ref() {
            None => None,
            Some(Leader::ModifierTap(m)) => Some(ResolvedLeader::Tap(*m)),
            Some(Leader::Chord(chord)) => match key_to_keycode(&chord.key) {
                Some(keycode) => Some(ResolvedLeader::Chord(keycode, chord.modifiers)),
                None => {
                    tracing::warn!(chord = %chord, "shortcut leader key has no physical KeyCode mapping; <Leader> bindings unreachable");
                    None
                }
            },
        }
    };
    if resolved.leader.is_none() && !resolved.prefix.is_empty() {
        tracing::warn!(
            "shortcuts.<Leader> bindings are set but the leader is disabled or unmappable; they are unreachable"
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

/// Resolves each bound chord to an `OzmuxShortcut`, skipping (with a warning)
/// any chord whose logical key has no physical `KeyCode`.
fn resolve_from_chords<'a>(
    chords: impl Iterator<Item = (&'static str, &'a KeyChord, ShortcutAction, bool)>,
) -> Vec<OzmuxShortcut> {
    let mut out = Vec::new();
    for (label, chord, action, repeat) in chords {
        match key_to_keycode(&chord.key) {
            Some(keycode) => out.push(OzmuxShortcut {
                keycode,
                modifiers: chord.modifiers,
                action,
                repeat,
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

/// One input the tap machine observes. Key auto-repeats are filtered by the
/// caller before this is produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TapEvent {
    /// The target modifier was pressed.
    TargetDown,
    /// A non-target key was pressed (a chord is forming — disarm).
    OtherDown,
    /// The target modifier was released.
    TargetUp,
}

/// Result of one `step_tap`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TapOutcome {
    /// Nothing to do.
    None,
    /// A valid tap completed — engage the leader.
    Fired,
}

/// Pure transition for the modifier-tap machine. `armed` holds the target's
/// down-time (or `None` when not armed). A tap fires on `TargetUp` iff the
/// target is still armed within `timeout`; any other-key press disarms.
/// Mouse presses are handled at the frame level by the caller before `step_tap`
/// is invoked.
fn step_tap(
    armed: &mut Option<Duration>,
    event: TapEvent,
    now: Duration,
    timeout: Duration,
) -> TapOutcome {
    match event {
        TapEvent::TargetDown => {
            *armed = Some(now);
            TapOutcome::None
        }
        TapEvent::OtherDown => {
            *armed = None;
            TapOutcome::None
        }
        TapEvent::TargetUp => {
            let fired = matches!(*armed, Some(down_at) if now.saturating_sub(down_at) <= timeout);
            *armed = None;
            if fired {
                TapOutcome::Fired
            } else {
                TapOutcome::None
            }
        }
    }
}

/// True when `keycode` is a left/right variant of `modifier`.
fn is_tap_modifier_key(keycode: KeyCode, modifier: TapModifier) -> bool {
    match modifier {
        TapModifier::Meta => matches!(keycode, KeyCode::SuperLeft | KeyCode::SuperRight),
        TapModifier::Ctrl => matches!(keycode, KeyCode::ControlLeft | KeyCode::ControlRight),
        TapModifier::Alt => matches!(keycode, KeyCode::AltLeft | KeyCode::AltRight),
    }
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
    use bevy::input::keyboard::Key;
    use ozmux_configs::OzmuxConfigs;
    use ozmux_configs::shortcuts::{Binding, Shortcuts as ConfigShortcuts};

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn repeat_fixture() -> Shortcuts {
        Shortcuts {
            direct: Vec::new(),
            prefix: vec![
                OzmuxShortcut {
                    keycode: KeyCode::KeyS,
                    modifiers: mods(false, false, false, false),
                    action: ShortcutAction::EnterCopyMode,
                    repeat: true,
                },
                OzmuxShortcut {
                    keycode: KeyCode::KeyD,
                    modifiers: mods(false, false, false, false),
                    action: ShortcutAction::DetachSession,
                    repeat: true,
                },
                OzmuxShortcut {
                    keycode: KeyCode::KeyP,
                    modifiers: mods(false, false, false, false),
                    action: ShortcutAction::Paste,
                    repeat: false,
                },
            ],
            leader: Some(ResolvedLeader::Chord(
                KeyCode::KeyA,
                mods(true, false, false, false),
            )),
            tap_timeout: ms(300),
            repeat_time: ms(500),
        }
    }

    fn no_mods() -> Modifiers {
        mods(false, false, false, false)
    }

    #[test]
    fn repeat_binding_opens_window_and_refires_without_leader() {
        let sc = repeat_fixture();
        let mut phase = LeaderPhase::Pending;
        assert!(matches!(
            step_leader(&mut phase, &sc, KeyCode::KeyS, no_mods(), ms(0)),
            LeaderStep::RunAction(ShortcutAction::EnterCopyMode)
        ));
        assert_eq!(phase, LeaderPhase::Repeat { deadline: ms(500) });
        assert!(matches!(
            step_leader(&mut phase, &sc, KeyCode::KeyS, no_mods(), ms(100)),
            LeaderStep::RunAction(ShortcutAction::EnterCopyMode)
        ));
        assert_eq!(
            phase,
            LeaderPhase::Repeat { deadline: ms(600) },
            "each fire re-arms the window"
        );
    }

    #[test]
    fn other_repeat_binding_fires_inside_window() {
        let sc = repeat_fixture();
        let mut phase = LeaderPhase::Repeat { deadline: ms(500) };
        assert!(matches!(
            step_leader(&mut phase, &sc, KeyCode::KeyD, no_mods(), ms(100)),
            LeaderStep::RunAction(ShortcutAction::DetachSession)
        ));
        assert_eq!(phase, LeaderPhase::Repeat { deadline: ms(600) });
    }

    #[test]
    fn non_repeat_prefix_binding_does_not_fire_inside_window() {
        let sc = repeat_fixture();
        let mut phase = LeaderPhase::Repeat { deadline: ms(500) };
        assert!(matches!(
            step_leader(&mut phase, &sc, KeyCode::KeyP, no_mods(), ms(100)),
            LeaderStep::Passthrough
        ));
        assert_eq!(
            phase,
            LeaderPhase::Idle,
            "a non-repeat key closes the window and re-dispatches normally (tmux parity)"
        );
    }

    #[test]
    fn unmatched_key_inside_window_passes_through_and_closes() {
        let sc = repeat_fixture();
        let mut phase = LeaderPhase::Repeat { deadline: ms(500) };
        assert!(matches!(
            step_leader(&mut phase, &sc, KeyCode::KeyZ, no_mods(), ms(100)),
            LeaderStep::Passthrough
        ));
        assert_eq!(phase, LeaderPhase::Idle);
    }

    #[test]
    fn leader_inside_window_starts_new_pending() {
        let sc = repeat_fixture();
        let mut phase = LeaderPhase::Repeat { deadline: ms(500) };
        assert!(matches!(
            step_leader(
                &mut phase,
                &sc,
                KeyCode::KeyA,
                mods(true, false, false, false),
                ms(100)
            ),
            LeaderStep::Swallow
        ));
        assert_eq!(phase, LeaderPhase::Pending);
    }

    #[test]
    fn expired_window_reevaluates_normally() {
        let sc = repeat_fixture();
        let mut phase = LeaderPhase::Repeat { deadline: ms(500) };
        assert!(matches!(
            step_leader(&mut phase, &sc, KeyCode::KeyS, no_mods(), ms(600)),
            LeaderStep::Passthrough
        ));
        assert_eq!(phase, LeaderPhase::Idle);
    }

    #[test]
    fn zero_repeat_time_never_opens_window() {
        let sc = Shortcuts {
            repeat_time: Duration::ZERO,
            ..repeat_fixture()
        };
        let mut phase = LeaderPhase::Pending;
        assert!(matches!(
            step_leader(&mut phase, &sc, KeyCode::KeyS, no_mods(), ms(0)),
            LeaderStep::RunAction(ShortcutAction::EnterCopyMode)
        ));
        assert_eq!(phase, LeaderPhase::Idle);
    }

    #[test]
    fn modifier_key_does_not_close_window() {
        let sc = repeat_fixture();
        let mut phase = LeaderPhase::Repeat { deadline: ms(500) };
        assert!(matches!(
            step_leader(
                &mut phase,
                &sc,
                KeyCode::ControlLeft,
                mods(true, false, false, false),
                ms(100)
            ),
            LeaderStep::Passthrough
        ));
        assert_eq!(phase, LeaderPhase::Repeat { deadline: ms(500) });
    }

    #[test]
    fn non_repeat_binding_from_pending_stays_idle() {
        let sc = repeat_fixture();
        let mut phase = LeaderPhase::Pending;
        assert!(matches!(
            step_leader(&mut phase, &sc, KeyCode::KeyP, no_mods(), ms(0)),
            LeaderStep::RunAction(ShortcutAction::Paste)
        ));
        assert_eq!(phase, LeaderPhase::Idle);
    }

    #[test]
    fn test_constructor_builds_repeat_prefix() {
        let sc = test_shortcuts_with_repeat_prefix(
            KeyCode::KeyH,
            ShortcutAction::EnterCopyMode,
            ms(500),
        );
        assert_eq!(
            sc.match_repeat_prefix(KeyCode::KeyH, mods(false, false, false, false)),
            Some(ShortcutAction::EnterCopyMode)
        );
        assert!(sc.is_leader(KeyCode::KeyA, mods(true, false, false, false)));
    }

    #[test]
    fn step_tap_fires_on_quick_press_release() {
        let mut armed = None;
        assert_eq!(
            step_tap(&mut armed, TapEvent::TargetDown, ms(0), ms(300)),
            TapOutcome::None
        );
        assert_eq!(
            step_tap(&mut armed, TapEvent::TargetUp, ms(100), ms(300)),
            TapOutcome::Fired
        );
        assert_eq!(armed, None);
    }

    #[test]
    fn step_tap_no_fire_after_timeout() {
        let mut armed = None;
        step_tap(&mut armed, TapEvent::TargetDown, ms(0), ms(300));
        assert_eq!(
            step_tap(&mut armed, TapEvent::TargetUp, ms(400), ms(300)),
            TapOutcome::None
        );
    }

    #[test]
    fn step_tap_other_key_disarms() {
        let mut armed = None;
        step_tap(&mut armed, TapEvent::TargetDown, ms(0), ms(300));
        step_tap(&mut armed, TapEvent::OtherDown, ms(50), ms(300));
        assert_eq!(
            step_tap(&mut armed, TapEvent::TargetUp, ms(100), ms(300)),
            TapOutcome::None
        );
    }

    #[test]
    fn step_tap_release_without_arm_does_nothing() {
        let mut armed = None;
        assert_eq!(
            step_tap(&mut armed, TapEvent::TargetUp, ms(10), ms(300)),
            TapOutcome::None
        );
    }

    #[test]
    fn is_tap_modifier_key_matches_left_and_right() {
        assert!(is_tap_modifier_key(KeyCode::SuperLeft, TapModifier::Meta));
        assert!(is_tap_modifier_key(KeyCode::SuperRight, TapModifier::Meta));
        assert!(!is_tap_modifier_key(
            KeyCode::ControlLeft,
            TapModifier::Meta
        ));
        assert!(is_tap_modifier_key(KeyCode::AltRight, TapModifier::Alt));
    }

    #[test]
    fn tap_modifier_returns_modifier_for_tap_leader() {
        let s = Shortcuts {
            direct: Vec::new(),
            prefix: Vec::new(),
            leader: Some(ResolvedLeader::Tap(TapModifier::Meta)),
            tap_timeout: Duration::from_millis(300),
            repeat_time: Duration::from_millis(500),
        };
        assert_eq!(s.tap_modifier(), Some(TapModifier::Meta));
    }

    #[test]
    fn tap_leader_does_not_match_is_leader() {
        let s = Shortcuts {
            direct: Vec::new(),
            prefix: Vec::new(),
            leader: Some(ResolvedLeader::Tap(TapModifier::Meta)),
            tap_timeout: Duration::from_millis(300),
            repeat_time: Duration::from_millis(500),
        };
        assert!(!s.is_leader(KeyCode::SuperLeft, mods(false, false, false, true)));
        assert!(!s.is_leader(KeyCode::SuperLeft, mods(false, false, false, false)));
    }

    #[test]
    fn input_bindings_tap_leader_reserves_nothing_extra() {
        let s = Shortcuts {
            direct: Vec::new(),
            prefix: Vec::new(),
            leader: Some(ResolvedLeader::Tap(TapModifier::Meta)),
            tap_timeout: Duration::from_millis(300),
            repeat_time: Duration::from_millis(500),
        };
        let b = s.input_bindings();
        assert!(
            b.reserved.is_empty(),
            "a tap leader has no chord to reserve in TerminalInputBindings",
        );
    }

    fn mods(ctrl: bool, shift: bool, alt: bool, meta: bool) -> Modifiers {
        Modifiers {
            ctrl,
            shift,
            alt,
            meta,
        }
    }

    fn direct_only(config: &ConfigShortcuts) -> Shortcuts {
        Shortcuts {
            direct: resolve_from_chords(config.direct_chords().map(|(l, c, a)| (l, c, a, false))),
            prefix: Vec::new(),
            leader: None,
            tap_timeout: Duration::from_millis(300),
            repeat_time: Duration::from_millis(500),
        }
    }

    #[test]
    fn leader_resolves_from_config_chord() {
        let s = Shortcuts {
            direct: Vec::new(),
            prefix: Vec::new(),
            leader: Some(ResolvedLeader::Chord(
                KeyCode::KeyA,
                mods(true, false, false, false),
            )),
            tap_timeout: Duration::from_millis(300),
            repeat_time: Duration::from_millis(500),
        };
        assert!(s.is_leader(KeyCode::KeyA, mods(true, false, false, false)));
        assert!(!s.is_leader(KeyCode::KeyA, mods(false, false, false, false)));
        assert!(!s.is_leader(KeyCode::KeyB, mods(true, false, false, false)));
    }

    #[test]
    fn match_prefix_entry_excludes_release_webview_focus() {
        let s = Shortcuts {
            direct: Vec::new(),
            prefix: vec![OzmuxShortcut {
                keycode: KeyCode::KeyR,
                modifiers: mods(false, false, false, false),
                action: ShortcutAction::ReleaseWebviewFocus,
                repeat: false,
            }],
            leader: Some(ResolvedLeader::Chord(
                KeyCode::KeyA,
                mods(true, false, false, false),
            )),
            tap_timeout: Duration::from_millis(300),
            repeat_time: Duration::from_millis(500),
        };
        assert_eq!(
            s.match_prefix_entry(KeyCode::KeyR, mods(false, false, false, false))
                .map(|s| s.action),
            None,
            "a leader-scoped release-webview-focus resolves to Swallow, not a dead RunAction",
        );
    }

    #[test]
    fn input_bindings_reserves_the_leader_chord() {
        let mut s = direct_only(&ConfigShortcuts::default());
        s.leader = Some(ResolvedLeader::Chord(
            KeyCode::KeyA,
            mods(true, false, false, false),
        ));
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
        // event before KeyD; it must not consume the pending phase. Leader
        // Ctrl+B, prefix detach-session = Ctrl+D.
        let sc = Shortcuts {
            direct: Vec::new(),
            prefix: vec![OzmuxShortcut {
                keycode: KeyCode::KeyD,
                modifiers: mods(true, false, false, false),
                action: ShortcutAction::DetachSession,
                repeat: false,
            }],
            leader: Some(ResolvedLeader::Chord(
                KeyCode::KeyB,
                mods(true, false, false, false),
            )),
            tap_timeout: Duration::from_millis(300),
            repeat_time: Duration::from_millis(500),
        };
        let mut phase = LeaderPhase::Idle;
        assert!(matches!(
            step_leader(
                &mut phase,
                &sc,
                KeyCode::ControlLeft,
                mods(true, false, false, false),
                ms(0)
            ),
            LeaderStep::Passthrough
        ));
        assert_eq!(
            phase,
            LeaderPhase::Idle,
            "a bare modifier must not engage the leader"
        );
        assert!(matches!(
            step_leader(
                &mut phase,
                &sc,
                KeyCode::KeyB,
                mods(true, false, false, false),
                ms(0)
            ),
            LeaderStep::Swallow
        ));
        assert_eq!(
            phase,
            LeaderPhase::Pending,
            "the leader chord engages pending"
        );
        assert!(matches!(
            step_leader(
                &mut phase,
                &sc,
                KeyCode::ControlLeft,
                mods(true, false, false, false),
                ms(0)
            ),
            LeaderStep::Passthrough
        ));
        assert_eq!(
            phase,
            LeaderPhase::Pending,
            "a bare modifier must NOT clear pending mid-sequence"
        );
        assert!(matches!(
            step_leader(
                &mut phase,
                &sc,
                KeyCode::KeyD,
                mods(true, false, false, false),
                ms(0)
            ),
            LeaderStep::RunAction(ShortcutAction::DetachSession)
        ));
        assert_eq!(phase, LeaderPhase::Idle);
    }

    #[test]
    fn match_prefix_entry_resolves_and_requires_exact_mods() {
        let s = Shortcuts {
            direct: Vec::new(),
            prefix: vec![OzmuxShortcut {
                keycode: KeyCode::KeyS,
                modifiers: mods(false, false, false, false),
                action: ShortcutAction::EnterCopyMode,
                repeat: false,
            }],
            leader: Some(ResolvedLeader::Chord(
                KeyCode::KeyA,
                mods(true, false, false, false),
            )),
            tap_timeout: Duration::from_millis(300),
            repeat_time: Duration::from_millis(500),
        };
        assert_eq!(
            s.match_prefix_entry(KeyCode::KeyS, mods(false, false, false, false))
                .map(|s| s.action),
            Some(ShortcutAction::EnterCopyMode)
        );
        assert_eq!(
            s.match_prefix_entry(KeyCode::KeyS, mods(false, true, false, false))
                .map(|s| s.action),
            None
        );
        assert_eq!(
            s.match_prefix_entry(KeyCode::KeyD, mods(false, false, false, false))
                .map(|s| s.action),
            None
        );
    }

    #[test]
    fn resolve_from_chords_accepts_leader_chords() {
        let config = ConfigShortcuts {
            detach_session: Some(Binding::Leader {
                chord: ozmux_configs::shortcuts::parse_key_chord("d").unwrap(),
                repeat: true,
            }),
            ..Default::default()
        };
        let resolved = resolve_from_chords(config.leader_chords());
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].keycode, KeyCode::KeyD);
        assert_eq!(resolved[0].action, ShortcutAction::DetachSession);
        assert!(
            resolved[0].repeat,
            "the <Leader:r> flag must reach the resolved table"
        );
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
        let r = direct_only(&ConfigShortcuts::default());
        assert_eq!(r.direct.len(), 5);
    }

    #[test]
    fn match_gui_action_resolves_defaults() {
        let r = direct_only(&ConfigShortcuts::default());
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
        let r = direct_only(&ConfigShortcuts::default());
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
        let r = direct_only(&ConfigShortcuts::default());
        assert_eq!(
            r.match_gui_action(KeyCode::Escape, mods(true, true, false, false)),
            None
        );
    }

    #[test]
    fn unmatched_chord_is_none() {
        let r = direct_only(&ConfigShortcuts::default());
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
        let r = direct_only(&ConfigShortcuts::default());
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
        let r = direct_only(&ConfigShortcuts::default());
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
                repeat: false,
            }],
            leader: Some(ResolvedLeader::Chord(
                KeyCode::KeyA,
                mods(true, false, false, false),
            )),
            tap_timeout: Duration::from_millis(300),
            repeat_time: Duration::from_millis(500),
        }
    }

    #[test]
    fn leader_press_sets_pending_and_swallows() {
        let sc = leader_fixture();
        let mut phase = LeaderPhase::Idle;
        let step = step_leader(
            &mut phase,
            &sc,
            KeyCode::KeyA,
            mods(true, false, false, false),
            ms(0),
        );
        assert!(matches!(step, LeaderStep::Swallow));
        assert_eq!(phase, LeaderPhase::Pending);
    }

    #[test]
    fn pending_plus_bound_key_runs_action_and_clears_pending() {
        let sc = leader_fixture();
        let mut phase = LeaderPhase::Pending;
        let step = step_leader(
            &mut phase,
            &sc,
            KeyCode::KeyS,
            mods(false, false, false, false),
            ms(0),
        );
        assert!(matches!(
            step,
            LeaderStep::RunAction(ShortcutAction::EnterCopyMode)
        ));
        assert_eq!(phase, LeaderPhase::Idle);
    }

    #[test]
    fn pending_plus_unbound_key_swallows_and_clears_pending() {
        let sc = leader_fixture();
        let mut phase = LeaderPhase::Pending;
        let step = step_leader(
            &mut phase,
            &sc,
            KeyCode::KeyZ,
            mods(false, false, false, false),
            ms(0),
        );
        assert!(matches!(step, LeaderStep::Swallow));
        assert_eq!(phase, LeaderPhase::Idle);
    }

    #[test]
    fn unrelated_key_passes_through() {
        let sc = leader_fixture();
        let mut phase = LeaderPhase::Idle;
        let step = step_leader(
            &mut phase,
            &sc,
            KeyCode::KeyB,
            mods(false, false, false, false),
            ms(0),
        );
        assert!(matches!(step, LeaderStep::Passthrough));
        assert_eq!(phase, LeaderPhase::Idle);
    }

    #[test]
    fn sequential_leader_then_bound_key_threads_pending() {
        // NOTE: the dispatch loop calls step_leader per event with one shared
        // `phase` local; this verifies the leader press then the bound key
        // thread that state correctly across two calls.
        let sc = leader_fixture();
        let mut phase = LeaderPhase::Idle;
        let first = step_leader(
            &mut phase,
            &sc,
            KeyCode::KeyA,
            mods(true, false, false, false),
            ms(0),
        );
        assert!(matches!(first, LeaderStep::Swallow));
        let second = step_leader(
            &mut phase,
            &sc,
            KeyCode::KeyS,
            mods(false, false, false, false),
            ms(0),
        );
        assert!(matches!(
            second,
            LeaderStep::RunAction(ShortcutAction::EnterCopyMode)
        ));
        assert_eq!(phase, LeaderPhase::Idle);
    }

    #[test]
    fn step_tap_fires_at_exact_timeout_boundary() {
        let mut armed = None;
        step_tap(&mut armed, TapEvent::TargetDown, ms(0), ms(300));
        assert_eq!(
            step_tap(&mut armed, TapEvent::TargetUp, ms(300), ms(300)),
            TapOutcome::Fired
        );
    }

    fn tap_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<KeyboardInput>()
            .init_resource::<ButtonInput<MouseButton>>()
            .insert_resource(ModifierTapState::default())
            .init_resource::<LeaderPhase>()
            .insert_resource(Shortcuts {
                direct: Vec::new(),
                prefix: Vec::new(),
                leader: Some(ResolvedLeader::Tap(TapModifier::Meta)),
                tap_timeout: Duration::from_millis(300),
                repeat_time: Duration::from_millis(500),
            })
            .add_systems(Update, detect_modifier_tap);
        app.world_mut().spawn((
            Window {
                focused: true,
                ..default()
            },
            PrimaryWindow,
        ));
        app
    }

    fn tap_key(app: &mut App, key_code: KeyCode, state: ButtonState) {
        app.world_mut().write_message(KeyboardInput {
            key_code,
            logical_key: Key::Super,
            state,
            text: None,
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
    }

    #[test]
    fn detect_modifier_tap_fires_on_quick_tap() {
        let mut app = tap_app();
        tap_key(&mut app, KeyCode::SuperLeft, ButtonState::Pressed);
        app.update();
        tap_key(&mut app, KeyCode::SuperLeft, ButtonState::Released);
        app.update();
        assert_eq!(
            *app.world().resource::<LeaderPhase>(),
            LeaderPhase::Pending,
            "a quick modifier tap must engage LeaderPhase::Pending"
        );
    }

    #[test]
    fn detect_modifier_tap_mouse_press_on_keyboardless_frame_disarms() {
        let mut app = tap_app();
        tap_key(&mut app, KeyCode::SuperLeft, ButtonState::Pressed);
        app.update();
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        app.update();
        tap_key(&mut app, KeyCode::SuperLeft, ButtonState::Released);
        app.update();
        assert_eq!(
            *app.world().resource::<LeaderPhase>(),
            LeaderPhase::Idle,
            "mouse press in a keyboard-less frame must disarm the tap and suppress the leader"
        );
    }

    #[test]
    fn detect_modifier_tap_same_frame_mouse_press_suppresses_tap() {
        let mut app = tap_app();
        // Same frame: target-modifier down AND a mouse press.
        tap_key(&mut app, KeyCode::SuperLeft, ButtonState::Pressed);
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        app.update();
        // Next frame: the mouse is no longer just-pressed; only the release arrives.
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .clear();
        tap_key(&mut app, KeyCode::SuperLeft, ButtonState::Released);
        app.update();
        assert_eq!(
            *app.world().resource::<LeaderPhase>(),
            LeaderPhase::Idle,
            "a mouse press in the same frame as the modifier press must suppress the tap"
        );
    }

    fn resolved_shortcuts(config: OzmuxConfigs) -> Shortcuts {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<Shortcuts>()
            .insert_resource(OzmuxConfigsResource(config))
            .add_systems(Startup, build_shortcuts);
        app.update();
        app.world().resource::<Shortcuts>().clone()
    }

    #[test]
    fn build_shortcuts_leaves_default_cmd_leader_inert_without_leader_bindings() {
        let resolved = resolved_shortcuts(OzmuxConfigs::default());
        assert!(resolved.tap_modifier().is_none());
        assert!(resolved.leader.is_none());
    }

    #[test]
    fn build_shortcuts_activates_default_cmd_leader_with_a_leader_binding() {
        let config = OzmuxConfigs {
            shortcuts: ConfigShortcuts {
                detach_session: Some(Binding::Leader {
                    chord: ozmux_configs::shortcuts::parse_key_chord("d").unwrap(),
                    repeat: false,
                }),
                ..Default::default()
            },
            ..Default::default()
        };
        let resolved = resolved_shortcuts(config);
        assert_eq!(resolved.tap_modifier(), Some(TapModifier::Meta));
    }

    #[test]
    fn build_shortcuts_resolves_repeat_time() {
        let config = OzmuxConfigs {
            shortcuts: ConfigShortcuts {
                repeat_time_ms: 250,
                ..Default::default()
            },
            ..Default::default()
        };
        let resolved = resolved_shortcuts(config);
        assert_eq!(resolved.repeat_time, Duration::from_millis(250));
    }
}
