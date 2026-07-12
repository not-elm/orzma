//! Resolves configured shortcut chords (logical keys) into physical
//! `KeyCode`-based entries the runtime input dispatcher matches against.
//! The translation lives here (not in `orzma_configs`) so the config crate
//! stays free of any `bevy` dependency.

use crate::configs::OrzmaConfigsResource;
use crate::input::InputPhase;
use crate::input::bindings::{FineModifier, OrzmaMouseConfig};
use crate::input::shortcuts::apply::ShortcutsApplyPlugin;
use bevy::ecs::system::SystemParam;
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::mouse::MouseButton;
use bevy::prelude::*;
use bevy::time::Real;
use bevy::window::PrimaryWindow;
use bevy_cef::prelude::FocusedWebview;
use orzma_configs::mouse::{FineModifier as CfgFineModifier, MouseConfig};
use orzma_configs::shortcuts::{
    Key as ConfigKey, KeyChord, Leader, Modifiers, Shortcut, TapModifier,
};
use orzma_configs::vi_mode::ViModeAction;
use orzma_tty_engine::{ButtonConfig, WheelConfig};
use std::time::Duration;

mod apply;

pub(super) struct ShortcutsPlugin;

impl Plugin for ShortcutsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ShortcutsApplyPlugin)
            .configure_sets(
                Update,
                (ShortcutSet::Resolve, ShortcutSet::Apply)
                    .chain()
                    .in_set(InputPhase::FocusedKey),
            )
            .add_message::<ShortcutMessage>()
            .add_message::<ViModeMessage>()
            .add_message::<TypeMessage>()
            .init_resource::<Shortcuts>()
            .init_resource::<LeaderPhase>()
            .init_resource::<HeldRepeatKey>()
            .init_resource::<ModifierTapState>()
            .configure_sets(Update, (LeaderGate::Detect, LeaderGate::Advance).chain())
            .add_systems(Startup, (build_shortcuts, populate_mouse_config).chain())
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
            );
    }
}

/// One resolved keyboard shortcut action, fanned out from `resolve_key_effects`
/// to the appliers (`crate::input::shortcuts::apply`).
/// Excludes `Quit` / `ReleaseWebviewFocus` (handled
/// inline in `resolve_key_effects`). `focused` is the `KeyboardFocused` surface;
/// `in_vi_mode` gates the vi-mode re-entry and paste-suppression rules.
#[derive(Message)]
pub(in crate::input) struct ShortcutMessage {
    /// The action to run.
    pub action: Shortcut,
    /// Whether the action was reached through the leader rather than a direct chord.
    pub via_leader: bool,
    /// The `KeyboardFocused` surface, or `None` when none is focused.
    pub focused: Option<Entity>,
    /// Whether the focused surface is in vi mode.
    pub in_vi_mode: bool,
}

/// One matched `[vi-mode]` key, fanned out to the appliers
/// (`crate::input::shortcuts::apply`).
#[derive(Message)]
pub(in crate::input) struct ViModeMessage {
    /// The vi-mode action to run.
    pub action: ViModeAction,
    /// The `KeyboardFocused` surface, or `None` when none is focused.
    pub focused: Option<Entity>,
}

/// One raw key to type into / forward to the focused terminal.
#[derive(Message)]
pub(in crate::input) struct TypeMessage {
    /// The logical key, for text/printable-key mapping.
    pub logical: Key,
    /// The `KeyboardFocused` surface, or `None` when none is focused.
    pub focused: Option<Entity>,
    /// The frame's modifier snapshot.
    pub mods: Modifiers,
}

/// The three shortcut-effect message writers `resolve_key_effects` fans out to,
/// bundled to stay within Bevy's system-parameter limit.
#[derive(SystemParam)]
pub(in crate::input) struct ShortcutMessages<'w> {
    pub shortcut: MessageWriter<'w, ShortcutMessage>,
    pub vi_mode: MessageWriter<'w, ViModeMessage>,
    pub type_keys: MessageWriter<'w, TypeMessage>,
}

/// Orders the two halves of shortcut dispatch inside `InputPhase::FocusedKey`:
/// `resolve_key_effects` (`Resolve`) fans out the per-responsibility messages
/// before the appliers (`crate::input::shortcuts::apply`,
/// `Apply`) read them, so every message is consumed the same frame it is
/// written.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(in crate::input) enum ShortcutSet {
    /// `resolve_key_effects`: classifies keys and fans out the typed messages.
    Resolve,
    /// The appliers (`crate::input::shortcuts::apply`):
    /// read the typed messages and apply their effects.
    Apply,
}

/// Shared leader phase: where the leader state machine is between keys.
/// Owned by `ShortcutsPlugin`; advanced by `crate::input::keyboard::handler::resolve_key_effects`
/// (the sole `LeaderGate::Advance` member) as it classifies each frame's keys.
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

/// The physical key currently held that fired a repeat-marked `<Leader:r>`
/// binding. OS auto-repeats of this key re-fire the binding regardless of the
/// `LeaderPhase::Repeat` time window: that window (`repeat_time_ms`) only
/// bridges discrete re-presses and is far shorter than the OS initial
/// key-repeat delay, so without this a held key would drop out of the repeat
/// and leak into the terminal. Armed on a fresh press that opens or renews the
/// repeat window; cleared on any fresh press that does not.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HeldRepeatKey(pub(crate) Option<KeyCode>);

/// In-progress modifier-tap state: the target's down-time while armed.
#[derive(Resource, Default)]
struct ModifierTapState {
    armed: Option<Duration>,
}

/// Orders the `FocusedKey` systems that touch `LeaderPhase` so
/// `detect_modifier_tap` (`Detect`) sets it before `resolve_key_effects`
/// (`Advance`) steps the leader machine and clears it.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum LeaderGate {
    /// `detect_modifier_tap`: sets `LeaderPhase` to `Pending` on a modifier tap.
    Detect,
    /// `crate::input::keyboard::handler::resolve_key_effects`: advances the leader.
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
struct OrzmaShortcut {
    keycode: KeyCode,
    modifiers: Modifiers,
    action: Shortcut,
    repeat: bool,
}

/// The startup-resolved orzma shortcut tables. Built once from
/// `OrzmaConfigsResource`; consumed by the keyboard dispatcher.
#[derive(Resource, Default, Debug, Clone)]
pub(crate) struct Shortcuts {
    direct: Vec<OrzmaShortcut>,
    prefix: Vec<OrzmaShortcut>,
    leader: Option<ResolvedLeader>,
    tap_timeout: Duration,
    repeat_time: Duration,
}

impl Shortcuts {
    /// Returns the GUI action bound to `(keycode, mods)` in the direct table, if
    /// any.
    pub(crate) fn match_gui_action(&self, keycode: KeyCode, mods: Modifiers) -> Option<Shortcut> {
        Self::find_entry(&self.direct, keycode, mods).map(|s| s.action)
    }

    /// Returns the leader-scoped action bound to `(keycode, mods)` when the
    /// binding is repeat-marked (`<Leader:r>`). Drives the repeat-window
    /// re-fire in `step_leader` and the held-key re-fire in `refire_held_repeat`.
    fn match_repeat_prefix(&self, keycode: KeyCode, mods: Modifiers) -> Option<Shortcut> {
        self.match_prefix_entry(keycode, mods)
            .filter(|s| s.repeat)
            .map(|s| s.action)
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

    /// Returns the prefix-table entry bound to `(keycode, mods)`, if any.
    fn match_prefix_entry(&self, keycode: KeyCode, mods: Modifiers) -> Option<&OrzmaShortcut> {
        Self::find_entry(&self.prefix, keycode, mods)
    }

    /// Returns the first entry in `table` bound to `(keycode, mods)`. The single
    /// lookup predicate behind both the direct and prefix tables.
    fn find_entry(
        table: &[OrzmaShortcut],
        keycode: KeyCode,
        mods: Modifiers,
    ) -> Option<&OrzmaShortcut> {
        table
            .iter()
            .find(|s| s.keycode == keycode && s.modifiers == mods)
    }
}

/// Outcome of one `step_leader` call for a single pressed key. `Passthrough`
/// means the key is not leader-related and the caller proceeds with its normal
/// dispatch.
pub(crate) enum LeaderStep {
    /// A leader-scoped binding matched; run this action.
    RunAction(Shortcut),
    /// Consume the key with no effect (the leader itself, or an unmatched second
    /// key that abandons the sequence).
    Swallow,
    /// Not leader-related; fall through to the caller's normal dispatch.
    Passthrough,
}

/// Advances the orzma leader state machine for one pressed key, threading
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

/// Re-fires a held repeat-marked binding on an OS auto-repeat and re-arms the
/// repeat window. Returns the bound action when `(keycode, mods)` still
/// resolves to a repeat-marked binding, so a physically-held key keeps firing
/// past the `repeat_time` window (which is too short to bridge the OS
/// initial-repeat delay); returns `None` without touching `phase` so the caller
/// falls back to its normal `<Leader:r>`/typing handling.
pub(crate) fn refire_held_repeat(
    phase: &mut LeaderPhase,
    shortcuts: &Shortcuts,
    keycode: KeyCode,
    mods: Modifiers,
    now: Duration,
) -> Option<Shortcut> {
    let action = shortcuts.match_repeat_prefix(keycode, mods)?;
    *phase = LeaderPhase::Repeat {
        deadline: now + shortcuts.repeat_time,
    };
    Some(action)
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
    action: Shortcut,
    repeat_time: Duration,
) -> Shortcuts {
    Shortcuts {
        direct: Vec::new(),
        prefix: vec![OrzmaShortcut {
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

/// Test-only constructor: a `Shortcuts` with one direct (non-leader) chord and
/// no leader/prefix bindings. Used by `resolve.rs`'s decider tests, which
/// cannot name this module's private fields.
#[cfg(test)]
pub(crate) fn test_shortcuts_with_direct_chord(
    keycode: KeyCode,
    modifiers: Modifiers,
    action: Shortcut,
) -> Shortcuts {
    Shortcuts {
        direct: vec![OrzmaShortcut {
            keycode,
            modifiers,
            action,
            repeat: false,
        }],
        prefix: Vec::new(),
        leader: None,
        tap_timeout: Duration::from_millis(300),
        repeat_time: Duration::from_millis(500),
    }
}

/// Clears the leader phase (pending or repeat window), any armed hold-to-repeat,
/// and any in-progress modifier tap on a webview focus change, so a leader
/// engaged/armed before the focus change never fires after it.
fn reset_leader_phase(
    mut leader_phase: ResMut<LeaderPhase>,
    mut held_repeat: ResMut<HeldRepeatKey>,
    mut tap: ResMut<ModifierTapState>,
) {
    clear_leader_phase(&mut leader_phase);
    // NOTE: hold-to-repeat re-fires independently of `LeaderPhase`, so clearing
    // only the phase would leave a physically-held resize key re-firing across
    // the very transition this reset exists to fence off.
    if held_repeat.0.is_some() {
        held_repeat.0 = None;
    }
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
fn build_shortcuts(mut resolved: ResMut<Shortcuts>, configs: Res<OrzmaConfigsResource>) {
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

/// `Startup` system: inserts `OrzmaMouseConfig` from the resolved `[mouse]` block.
fn populate_mouse_config(mut commands: Commands, configs: Res<OrzmaConfigsResource>) {
    commands.insert_resource(orzma_mouse_config(&configs.mouse));
}

/// Maps the resolved `[mouse]` config block to the terminal crate's
/// `OrzmaMouseConfig`.
fn orzma_mouse_config(mc: &MouseConfig) -> OrzmaMouseConfig {
    OrzmaMouseConfig {
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

/// Resolves each bound chord to an `OrzmaShortcut`, skipping (with a warning)
/// any chord whose logical key has no physical `KeyCode`.
fn resolve_from_chords<'a>(
    chords: impl Iterator<Item = (&'static str, &'a KeyChord, Shortcut, bool)>,
) -> Vec<OrzmaShortcut> {
    let mut out = Vec::new();
    for (label, chord, action, repeat) in chords {
        match key_to_keycode(&chord.key) {
            Some(keycode) => out.push(OrzmaShortcut {
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

/// Maps a config logical `Key` to the physical `KeyCode` orzma matches on.
/// Returns `None` for keys with no stable physical mapping (`Plus`, `Other`,
/// non-alphanumeric chars).
fn key_to_keycode(key: &ConfigKey) -> Option<KeyCode> {
    // NOTE: keep this accepted domain in lockstep with
    // `orzma_configs::shortcuts::Key::maps_to_physical_key`; a divergence lets
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
            '[' => KeyCode::BracketLeft,
            ']' => KeyCode::BracketRight,
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
    use orzma_configs::OrzmaConfigs;
    use orzma_configs::shortcuts::{Binding, Shortcuts as ConfigShortcuts};

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn reset_leader_phase_clears_held_repeat() {
        let mut app = App::new();
        app.init_resource::<LeaderPhase>()
            .init_resource::<HeldRepeatKey>()
            .init_resource::<ModifierTapState>()
            .add_systems(Update, reset_leader_phase);
        *app.world_mut().resource_mut::<LeaderPhase>() = LeaderPhase::Pending;
        app.world_mut().resource_mut::<HeldRepeatKey>().0 = Some(KeyCode::KeyH);
        app.update();
        assert_eq!(
            *app.world().resource::<LeaderPhase>(),
            LeaderPhase::Idle,
            "the context reset clears the leader phase"
        );
        assert_eq!(
            app.world().resource::<HeldRepeatKey>().0,
            None,
            "a context-transition reset must also disarm hold-to-repeat, not just the leader phase"
        );
    }

    fn repeat_fixture() -> Shortcuts {
        Shortcuts {
            direct: Vec::new(),
            prefix: vec![
                OrzmaShortcut {
                    keycode: KeyCode::KeyS,
                    modifiers: mods(false, false, false, false),
                    action: Shortcut::EnterViMode,
                    repeat: true,
                },
                OrzmaShortcut {
                    keycode: KeyCode::KeyD,
                    modifiers: mods(false, false, false, false),
                    action: Shortcut::KillPane,
                    repeat: true,
                },
                OrzmaShortcut {
                    keycode: KeyCode::KeyP,
                    modifiers: mods(false, false, false, false),
                    action: Shortcut::Paste,
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
            LeaderStep::RunAction(Shortcut::EnterViMode)
        ));
        assert_eq!(phase, LeaderPhase::Repeat { deadline: ms(500) });
        assert!(matches!(
            step_leader(&mut phase, &sc, KeyCode::KeyS, no_mods(), ms(100)),
            LeaderStep::RunAction(Shortcut::EnterViMode)
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
            LeaderStep::RunAction(Shortcut::KillPane)
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
            LeaderStep::RunAction(Shortcut::EnterViMode)
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
            LeaderStep::RunAction(Shortcut::Paste)
        ));
        assert_eq!(phase, LeaderPhase::Idle);
    }

    #[test]
    fn test_constructor_builds_repeat_prefix() {
        let sc = test_shortcuts_with_repeat_prefix(KeyCode::KeyH, Shortcut::EnterViMode, ms(500));
        assert_eq!(
            sc.match_repeat_prefix(KeyCode::KeyH, mods(false, false, false, false)),
            Some(Shortcut::EnterViMode)
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
    fn match_prefix_entry_resolves_release_webview_focus() {
        let s = Shortcuts {
            direct: Vec::new(),
            prefix: vec![OrzmaShortcut {
                keycode: KeyCode::KeyR,
                modifiers: mods(false, false, false, false),
                action: Shortcut::ReleaseWebviewFocus,
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
            Some(Shortcut::ReleaseWebviewFocus),
            "release-webview-focus is a normal action; a leader-scoped binding resolves to it \
             (leader dispatch runs under webview focus since #240)",
        );
    }

    #[test]
    fn step_leader_ignores_bare_modifier_and_survives_to_second_chord() {
        // Reproduces [0]: the second chord's Ctrl modifier emits its own Pressed
        // event before KeyD; it must not consume the pending phase. Leader
        // Ctrl+B, prefix kill-pane = Ctrl+D.
        let sc = Shortcuts {
            direct: Vec::new(),
            prefix: vec![OrzmaShortcut {
                keycode: KeyCode::KeyD,
                modifiers: mods(true, false, false, false),
                action: Shortcut::KillPane,
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
            LeaderStep::RunAction(Shortcut::KillPane)
        ));
        assert_eq!(phase, LeaderPhase::Idle);
    }

    #[test]
    fn match_prefix_entry_resolves_and_requires_exact_mods() {
        let s = Shortcuts {
            direct: Vec::new(),
            prefix: vec![OrzmaShortcut {
                keycode: KeyCode::KeyS,
                modifiers: mods(false, false, false, false),
                action: Shortcut::EnterViMode,
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
            Some(Shortcut::EnterViMode)
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
            kill_pane: Some(Binding::Leader {
                chord: orzma_configs::shortcuts::parse_key_chord("d").unwrap(),
                repeat: true,
            }),
            ..Default::default()
        };
        let resolved = resolve_from_chords(config.leader_chords());
        let kill_pane = resolved
            .iter()
            .find(|s| s.action == Shortcut::KillPane)
            .expect("kill-pane must resolve as a leader chord");
        assert_eq!(kill_pane.keycode, KeyCode::KeyD);
        assert!(
            kill_pane.repeat,
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
    fn bracket_maps_to_keycode() {
        assert_eq!(
            key_to_keycode(&ConfigKey::Char('[')),
            Some(KeyCode::BracketLeft)
        );
        assert_eq!(
            key_to_keycode(&ConfigKey::Char(']')),
            Some(KeyCode::BracketRight)
        );
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
    fn default_bindings_resolve_to_three_direct() {
        let r = direct_only(&ConfigShortcuts::default());
        assert_eq!(r.direct.len(), 3);
    }

    #[test]
    fn match_gui_action_resolves_defaults() {
        let r = direct_only(&ConfigShortcuts::default());
        assert_eq!(
            r.match_gui_action(KeyCode::KeyQ, mods(false, false, false, true)),
            Some(Shortcut::Quit)
        );
        assert_eq!(
            r.match_gui_action(KeyCode::KeyV, mods(false, false, false, true)),
            Some(Shortcut::Paste),
            "paste is a direct Cmd+V chord by default"
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
    fn release_webview_focus_matches_default_leader_chord() {
        let resolved = resolve_from_chords(ConfigShortcuts::default().leader_chords());
        let entry = resolved
            .iter()
            .find(|s| s.action == Shortcut::ReleaseWebviewFocus)
            .expect("release-webview-focus resolves as a leader chord by default");
        assert_eq!(
            entry.keycode,
            KeyCode::KeyU,
            "the default release-webview-focus binding is <Leader>u",
        );
        assert_eq!(entry.modifiers, mods(false, false, false, false));
    }

    #[test]
    fn mouse_config_maps_from_orzma_config() {
        use orzma_configs::mouse::{FineModifier as CfgFine, MouseConfig};
        let mc = MouseConfig {
            fine_modifier: CfgFine::Ctrl,
            max_protocol_events_per_frame: 5,
            cells_per_notch: 1.0,
            axis_lock_ratio: 0.5,
            ..MouseConfig::default()
        };
        let out = orzma_mouse_config(&mc);
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

    fn leader_fixture() -> Shortcuts {
        Shortcuts {
            direct: Vec::new(),
            prefix: vec![OrzmaShortcut {
                keycode: KeyCode::KeyS,
                modifiers: mods(false, false, false, false),
                action: Shortcut::EnterViMode,
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
        assert!(matches!(step, LeaderStep::RunAction(Shortcut::EnterViMode)));
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
            LeaderStep::RunAction(Shortcut::EnterViMode)
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

    fn resolved_shortcuts(config: OrzmaConfigs) -> Shortcuts {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<Shortcuts>()
            .insert_resource(OrzmaConfigsResource(config))
            .add_systems(Startup, build_shortcuts);
        app.update();
        app.world().resource::<Shortcuts>().clone()
    }

    #[test]
    fn build_shortcuts_leaves_default_cmd_leader_inert_without_leader_bindings() {
        // NOTE: `ConfigShortcuts::default()` ships 29 leader-scoped actions
        // (plus the direct `paste`/`quit` chords), so `OrzmaConfigs::default()`
        // alone no longer exercises the inert-leader path; every leader binding
        // is explicitly unbound here to reproduce a config with no leader
        // bindings at all.
        let config = OrzmaConfigs {
            shortcuts: ConfigShortcuts {
                paste: None,
                release_webview_focus: None,
                enter_vi_mode: None,
                select_left_pane: None,
                select_down_pane: None,
                select_up_pane: None,
                select_right_pane: None,
                split_vertical_pane: None,
                split_horizontal_pane: None,
                kill_pane: None,
                zoom_pane: None,
                resize_left_pane: None,
                resize_down_pane: None,
                resize_up_pane: None,
                resize_right_pane: None,
                new_window: None,
                kill_window: None,
                next_window: None,
                previous_window: None,
                select_window_0: None,
                select_window_1: None,
                select_window_2: None,
                select_window_3: None,
                select_window_4: None,
                select_window_5: None,
                select_window_6: None,
                select_window_7: None,
                select_window_8: None,
                select_window_9: None,
                rename_window: None,
                ..Default::default()
            },
            ..Default::default()
        };
        let resolved = resolved_shortcuts(config);
        assert!(resolved.tap_modifier().is_none());
        assert!(resolved.leader.is_none());
    }

    #[test]
    fn build_shortcuts_activates_default_cmd_leader_with_a_leader_binding() {
        let config = OrzmaConfigs {
            shortcuts: ConfigShortcuts {
                kill_pane: Some(Binding::Leader {
                    chord: orzma_configs::shortcuts::parse_key_chord("d").unwrap(),
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
        let config = OrzmaConfigs {
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
