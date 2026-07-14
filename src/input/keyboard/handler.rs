//! Resolves the frame's pressed keys through the pure
//! `crate::input::resolve::classify_key_batch` decider, handles the two
//! mode-independent effects inline (Quit → `AppExit`, release-webview-focus →
//! clear `FocusedWebview`), and fans out the remaining effects as the three
//! per-responsibility shortcut messages (`ShortcutMessage`, `ViModeMessage`,
//! `TypeMessage`). The appliers (`crate::input::shortcuts::apply`) consume
//! those messages and apply the events. This is the sole system that steps
//! `LeaderPhase`.

use crate::action::vi::ResolvedViModeKeys;
use crate::action::vi::mode::ViModeState;
use crate::input::current_modifiers;
use crate::input::focus::KeyboardFocused;
use crate::input::ime::{ImeState, resolve_focused_surface};
use crate::input::keyboard::key_effect::{
    BatchContext, ClassifiedKeys, KeyEffect, classify_key_batch,
};
use crate::input::shortcuts::{
    HeldRepeatKey, LeaderGate, LeaderPhase, ShortcutMessage, ShortcutMessages, ShortcutSet,
    Shortcuts, TypeMessage, ViModeMessage, clear_leader_phase,
};
use crate::ui::multiplexer::rename_prompt::RenameState;
use bevy::ecs::system::SystemParam;
use bevy::input::keyboard::{KeyCode, KeyboardInput};
use bevy::prelude::*;
use bevy::time::Real;
use bevy::window::PrimaryWindow;
use bevy_cef::prelude::{CefKeyboardFilter, FocusedWebview, KeyboardDeliverSet, ModifiersState};
use orzma_configs::shortcuts::Shortcut;
use orzma_webview::ForwardKeys;

/// Registers `resolve_key_effects` and the `ShortcutSet` ordering.
pub(super) struct KeyboardHandlerPlugin;

impl Plugin for KeyboardHandlerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            resolve_key_effects
                .in_set(ShortcutSet::Resolve)
                .in_set(LeaderGate::Advance)
                .before(KeyboardDeliverSet)
                .run_if(on_message::<KeyboardInput>),
        );
    }
}

/// The classifier inputs `resolve_key_effects` feeds to `classify_key_batch`: the
/// shortcut table, resolved vi-mode keys, held modifier keys, and the
/// real-time clock the leader timeout is measured against.
#[derive(SystemParam)]
struct ClassifyInputs<'w> {
    shortcuts: Res<'w, Shortcuts>,
    resolved_vi_mode: Res<'w, ResolvedViModeKeys>,
    bevy_keys: Res<'w, ButtonInput<KeyCode>>,
    time: Res<'w, Time<Real>>,
}

/// Resolves the frame's pressed keys and fans out the per-responsibility
/// shortcut messages. Runs unconditionally (gated only on
/// `on_message::<KeyboardInput>`), in `InputPhase::FocusedKey` /
/// `ShortcutSet::Resolve` / `LeaderGate::Advance`. The sole `LeaderPhase`-stepping
/// system: on a coarse guard (IME composition, an unfocused window, or
/// the rename prompt owning the keyboard) it clears the leader, drains the
/// frame's keys, and writes no messages; otherwise it classifies the keys,
/// applies `Quit` (`AppExit`) and `ReleaseWebviewFocus` (clear
/// `FocusedWebview`) inline, and writes every other effect to its typed
/// message (`ShortcutMessage`, `ViModeMessage`, `TypeMessage`).
fn resolve_key_effects(
    mut exit: MessageWriter<AppExit>,
    mut events: MessageReader<KeyboardInput>,
    mut focused_webview: ResMut<FocusedWebview>,
    mut cef_filter: ResMut<CefKeyboardFilter>,
    mut leader_phase: ResMut<LeaderPhase>,
    mut held_repeat: ResMut<HeldRepeatKey>,
    mut messages: ShortcutMessages,
    ime: Res<ImeState>,
    inputs: ClassifyInputs,
    windows: Query<&Window, With<PrimaryWindow>>,
    focused_surface: Query<Entity, With<KeyboardFocused>>,
    vi_modes: Query<(), With<ViModeState>>,
    forward_keys: Query<&ForwardKeys>,
    rename: Option<Res<RenameState>>,
) {
    let focused_window = windows.single().map(|w| w.focused).unwrap_or(false);
    // NOTE: DRAIN (clear) the frame's keys on this guard rather than gating
    // the whole system off with run_if — a run_if would leave the frame's
    // keys buffered on this reader's cursor and re-inject them (e.g. a
    // Cmd+V paste, or a split/zoom chord) into resolution on the next
    // ungated frame (the reader cursor only advances when the body runs).
    if ime.is_composing() || !focused_window || rename.is_some() {
        clear_leader_phase(&mut leader_phase);
        if held_repeat.0.is_some() {
            held_repeat.0 = None;
        }
        clear_cef_filter(&mut cef_filter);
        events.clear();
        return;
    }

    let focused = resolve_focused_surface(&focused_surface);
    let in_vi_mode = focused.is_some_and(|entity| vi_modes.get(entity).is_ok());
    let forward_chords = focused_webview
        .0
        .and_then(|entity| forward_keys.get(entity).ok())
        .map(|chords| chords.0.as_slice())
        .unwrap_or(&[]);
    let mods = current_modifiers(&inputs.bevy_keys);
    let ctx = BatchContext {
        mods,
        now: inputs.time.elapsed(),
        in_vi_mode,
        webview_focused: focused_webview.0.is_some(),
        forward_chords,
    };
    // NOTE: snapshot the focused webview BEFORE the effects loop, which may set
    // `focused_webview.0 = None` on a `ReleaseWebviewFocus` chord. Keying the
    // filter to the value read here (not after the loop) keeps this frame's
    // suppression tied to the webview the keys were classified against, rather
    // than leaning on bevy_cef's None-target delivery guard to cover the gap.
    let suppress_target = focused_webview.0;
    let mut held = held_repeat.0;
    let ClassifiedKeys {
        effects: all,
        webview_suppressed,
    } = classify_key_batch(
        &mut leader_phase,
        &mut held,
        &inputs.shortcuts,
        &inputs.resolved_vi_mode,
        events.read(),
        ctx,
    );
    if held_repeat.0 != held {
        held_repeat.0 = held;
    }

    for effect in all {
        match effect {
            KeyEffect::Shortcut {
                action: Shortcut::Quit,
                ..
            } => {
                exit.write(AppExit::Success);
            }
            KeyEffect::Shortcut {
                action: Shortcut::ReleaseWebviewFocus,
                ..
            } => focused_webview.0 = None,
            KeyEffect::Shortcut { action, via_leader } => {
                messages.shortcut.write(ShortcutMessage {
                    action,
                    via_leader,
                    focused,
                    in_vi_mode,
                });
            }
            KeyEffect::ViMode(action) => {
                messages.vi_mode.write(ViModeMessage { action, focused });
            }
            KeyEffect::Type { logical, .. } => {
                messages.type_keys.write(TypeMessage {
                    logical,
                    focused,
                    mods,
                });
            }
            // NOTE: WebviewForward is classified (not suppressed, not typed) so
            // the chord reaches the focused webview through CEF's native
            // keyboard path; there is no message to deliver host-side.
            KeyEffect::WebviewForward { .. } => {}
        }
    }
    let ms = ModifiersState {
        alt: mods.alt,
        ctrl: mods.ctrl,
        shift: mods.shift,
        logo: mods.meta,
    };
    match suppress_target {
        Some(webview) => cef_filter.set(
            webview_suppressed
                .into_iter()
                .map(|code| (webview, code, ms)),
        ),
        None => clear_cef_filter(&mut cef_filter),
    }
}

/// Empties `CefKeyboardFilter`. Used on the coarse-guard early return and when no
/// webview owns the keyboard, so a stale leader claim never withholds a later key.
fn clear_cef_filter(cef_filter: &mut CefKeyboardFilter) {
    cef_filter.set(Vec::<(Entity, KeyCode, ModifiersState)>::new());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::shortcuts::{
        test_shortcuts_with_direct_chord, test_shortcuts_with_repeat_prefix,
    };
    use crate::surface::OrzmaTerminal;
    use bevy::ecs::schedule::{LogLevel, ScheduleBuildSettings};
    use bevy::input::ButtonState;
    use bevy::input::keyboard::Key;
    use orzma_configs::shortcuts::Modifiers;
    use std::time::Duration;

    #[derive(Resource, Default)]
    struct Captured {
        app_exit: usize,
        shortcuts: Vec<(Shortcut, bool)>,
        vi_mode: usize,
        typed: usize,
        focused: Option<Entity>,
        in_vi_mode: Option<bool>,
        mods: Option<Modifiers>,
        last_typed: Option<Key>,
    }

    impl Captured {
        fn message_count(&self) -> usize {
            self.shortcuts.len() + self.vi_mode + self.typed
        }
    }

    fn capture_messages(
        mut cap: ResMut<Captured>,
        mut shortcuts: MessageReader<ShortcutMessage>,
        mut vi_mode: MessageReader<ViModeMessage>,
        mut typed: MessageReader<TypeMessage>,
    ) {
        for m in shortcuts.read() {
            cap.shortcuts.push((m.action, m.via_leader));
            cap.focused = m.focused;
            cap.in_vi_mode = Some(m.in_vi_mode);
        }
        for m in vi_mode.read() {
            cap.vi_mode += 1;
            cap.focused = m.focused;
        }
        for m in typed.read() {
            cap.typed += 1;
            cap.focused = m.focused;
            cap.mods = Some(m.mods);
            cap.last_typed = Some(m.logical.clone());
        }
    }

    fn capture_exit(mut reader: MessageReader<AppExit>, mut cap: ResMut<Captured>) {
        cap.app_exit += reader.read().count();
    }

    fn resolve_app(shortcuts: Shortcuts) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(KeyboardHandlerPlugin)
            .add_message::<KeyboardInput>()
            .add_message::<ShortcutMessage>()
            .add_message::<ViModeMessage>()
            .add_message::<TypeMessage>()
            .add_message::<AppExit>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ImeState>()
            .init_resource::<FocusedWebview>()
            .init_resource::<CefKeyboardFilter>()
            .init_resource::<LeaderPhase>()
            .init_resource::<HeldRepeatKey>()
            .init_resource::<ResolvedViModeKeys>()
            .init_resource::<Captured>()
            .insert_resource(shortcuts)
            .configure_sets(Update, (ShortcutSet::Resolve, ShortcutSet::Apply).chain())
            .add_systems(
                Update,
                (capture_messages, capture_exit)
                    .chain()
                    .in_set(ShortcutSet::Apply),
            );
        app.world_mut().spawn((
            Window {
                focused: true,
                ..default()
            },
            PrimaryWindow,
        ));
        app
    }

    fn press_key(app: &mut App, key_code: KeyCode, logical: Key) {
        app.world_mut().write_message(KeyboardInput {
            key_code,
            logical_key: logical,
            state: ButtonState::Pressed,
            text: None,
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
    }

    #[test]
    fn normal_key_resolves_to_one_type_message() {
        let mut app = resolve_app(Shortcuts::default());
        let term = app.world_mut().spawn((OrzmaTerminal, KeyboardFocused)).id();
        press_key(&mut app, KeyCode::KeyA, Key::Character("a".into()));
        app.update();
        let cap = app.world().resource::<Captured>();
        assert_eq!(
            cap.message_count(),
            1,
            "exactly one shortcut message resolves per keyboard frame"
        );
        assert_eq!(
            cap.last_typed,
            Some(Key::Character("a".into())),
            "a plain key resolves to one TypeMessage"
        );
        assert_eq!(cap.focused, Some(term));
        assert_eq!(
            cap.mods,
            Some(Modifiers::default()),
            "no modifier keys are held, so the message carries the default modifiers"
        );
    }

    #[test]
    fn guarded_frame_emits_no_messages() {
        let mut app = resolve_app(Shortcuts::default());
        app.world_mut().spawn((OrzmaTerminal, KeyboardFocused));
        // Window unfocused: a coarse guard drains the frame with no messages.
        {
            let mut windows = app
                .world_mut()
                .query_filtered::<&mut Window, With<PrimaryWindow>>();
            windows.single_mut(app.world_mut()).unwrap().focused = false;
        }
        *app.world_mut().resource_mut::<LeaderPhase>() = LeaderPhase::Pending;
        press_key(&mut app, KeyCode::KeyA, Key::Character("a".into()));
        app.update();
        assert_eq!(
            app.world().resource::<Captured>().message_count(),
            0,
            "a guarded frame writes no shortcut messages"
        );
        assert_eq!(
            *app.world().resource::<LeaderPhase>(),
            LeaderPhase::Idle,
            "the guard clears the leader phase"
        );
    }

    fn meta_mods() -> Modifiers {
        Modifiers {
            ctrl: false,
            shift: false,
            alt: false,
            meta: true,
        }
    }

    /// Reproduces the modality leak this change fixes: with a `RenameState`
    /// prompt open, a chord bound to a pane action (split/zoom/etc.) must not
    /// resolve to a `ShortcutMessage` — the prompt owns the keyboard, so the
    /// focused pane must see no effect at all.
    #[test]
    fn shortcut_suppressed_while_modal_open() {
        let mut app = resolve_app(test_shortcuts_with_direct_chord(
            KeyCode::KeyW,
            Modifiers::default(),
            Shortcut::ZoomPane,
        ));
        let term = app.world_mut().spawn((OrzmaTerminal, KeyboardFocused)).id();
        app.world_mut().insert_resource(RenameState::renaming(term));
        press_key(&mut app, KeyCode::KeyW, Key::Character("w".into()));
        app.update();
        assert_eq!(
            app.world().resource::<Captured>().message_count(),
            0,
            "no ShortcutMessage resolves while the rename modal is open"
        );
    }

    /// Reproduces the paste leak: a Cmd+V-equivalent chord bound to `Paste`
    /// must not resolve to a `ShortcutMessage` while a `RenameState` prompt
    /// is open, so a paste never reaches the focused pane's PTY behind the
    /// prompt.
    #[test]
    fn paste_suppressed_while_modal_open() {
        let mut app = resolve_app(test_shortcuts_with_direct_chord(
            KeyCode::KeyV,
            meta_mods(),
            Shortcut::Paste,
        ));
        let term = app.world_mut().spawn((OrzmaTerminal, KeyboardFocused)).id();
        app.world_mut().insert_resource(RenameState::renaming(term));
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::SuperLeft);
        press_key(&mut app, KeyCode::KeyV, Key::Character("v".into()));
        app.update();
        assert_eq!(
            app.world().resource::<Captured>().message_count(),
            0,
            "no ShortcutMessage (including Paste) resolves while the rename modal is open"
        );
    }

    fn quit_test_app(spawn_focused: bool) -> App {
        let mut app = resolve_app(test_shortcuts_with_direct_chord(
            KeyCode::KeyQ,
            meta_mods(),
            Shortcut::Quit,
        ));
        if spawn_focused {
            app.world_mut().spawn((OrzmaTerminal, KeyboardFocused));
        }
        app
    }

    #[test]
    fn quit_writes_appexit_and_no_message() {
        let mut app = quit_test_app(true);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::SuperLeft);
        press_key(&mut app, KeyCode::KeyQ, Key::Character("q".into()));
        app.update();
        let cap = app.world().resource::<Captured>();
        assert_eq!(cap.app_exit, 1, "Cmd+Q writes AppExit");
        assert_eq!(
            cap.message_count(),
            0,
            "Quit is handled inline and never reaches a ShortcutMessage"
        );
    }

    #[test]
    fn release_clears_webview_and_no_message() {
        let mut app = resolve_app(test_shortcuts_with_direct_chord(
            KeyCode::Escape,
            Modifiers {
                ctrl: true,
                shift: true,
                alt: false,
                meta: false,
            },
            Shortcut::ReleaseWebviewFocus,
        ));
        let webview = app.world_mut().spawn_empty().id();
        app.world_mut().resource_mut::<FocusedWebview>().0 = Some(webview);
        {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.press(KeyCode::ControlLeft);
            keys.press(KeyCode::ShiftLeft);
        }
        press_key(&mut app, KeyCode::Escape, Key::Escape);
        app.update();
        let cap = app.world().resource::<Captured>();
        assert_eq!(
            app.world().resource::<FocusedWebview>().0,
            None,
            "the release chord clears the focused webview inline"
        );
        assert_eq!(
            cap.message_count(),
            0,
            "ReleaseWebviewFocus is handled inline and never reaches a ShortcutMessage"
        );
    }

    #[test]
    fn in_vi_mode_flag_set() {
        let mut app = resolve_app(test_shortcuts_with_direct_chord(
            KeyCode::KeyA,
            Modifiers::default(),
            Shortcut::EnterViMode,
        ));
        app.world_mut()
            .spawn((OrzmaTerminal, KeyboardFocused, ViModeState));
        press_key(&mut app, KeyCode::KeyA, Key::Character("a".into()));
        app.update();
        assert_eq!(
            app.world().resource::<Captured>().in_vi_mode,
            Some(true),
            "a focused surface in vi mode sets ShortcutMessage.in_vi_mode"
        );
    }

    #[test]
    fn in_vi_mode_flag_clear_outside_vi_mode() {
        let mut app = resolve_app(test_shortcuts_with_direct_chord(
            KeyCode::KeyA,
            Modifiers::default(),
            Shortcut::EnterViMode,
        ));
        app.world_mut().spawn((OrzmaTerminal, KeyboardFocused));
        press_key(&mut app, KeyCode::KeyA, Key::Character("a".into()));
        app.update();
        assert_eq!(
            app.world().resource::<Captured>().in_vi_mode,
            Some(false),
            "a focused surface NOT in vi mode sets ShortcutMessage.in_vi_mode to false"
        );
    }

    #[test]
    fn messages_consumed_same_update() {
        let mut app = resolve_app(Shortcuts::default());
        app.world_mut().spawn((OrzmaTerminal, KeyboardFocused));
        press_key(&mut app, KeyCode::KeyA, Key::Character("a".into()));
        app.update();
        assert_eq!(
            app.world().resource::<Captured>().typed,
            1,
            "the ShortcutSet Resolve->Apply chain lets the applier consume the message \
             the same frame it is written, not the next"
        );
    }

    #[test]
    fn quit_writes_appexit_with_no_focused_terminal() {
        // No KeyboardFocused terminal spawned; the window is focused.
        let mut app = quit_test_app(false);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::SuperLeft);
        press_key(&mut app, KeyCode::KeyQ, Key::Character("q".into()));
        app.update();
        let cap = app.world().resource::<Captured>();
        assert_eq!(
            cap.app_exit, 1,
            "Quit is handled inline in resolve_key_effects regardless of focus, so Cmd+Q with no \
             focused terminal still writes AppExit"
        );
        assert_eq!(
            cap.message_count(),
            0,
            "no ShortcutMessage is written even when nothing is focused"
        );
    }

    #[test]
    fn filter_holds_leader_claim_under_webview_focus() {
        let mut app = resolve_app(test_shortcuts_with_repeat_prefix(
            KeyCode::KeyS,
            Shortcut::EnterViMode,
            Duration::ZERO,
        ));
        app.world_mut().spawn((OrzmaTerminal, KeyboardFocused));
        let webview = app.world_mut().spawn_empty().id();
        app.world_mut().resource_mut::<FocusedWebview>().0 = Some(webview);
        *app.world_mut().resource_mut::<LeaderPhase>() = LeaderPhase::Pending;
        press_key(&mut app, KeyCode::KeyS, Key::Character("s".into()));
        app.update();
        assert!(
            app.world().resource::<CefKeyboardFilter>().contains(
                webview,
                KeyCode::KeyS,
                ModifiersState::default()
            ),
            "the leader-claimed second key is withheld from CEF for the focused webview"
        );
    }

    #[test]
    fn filter_cleared_on_guarded_frame() {
        let mut app = resolve_app(test_shortcuts_with_repeat_prefix(
            KeyCode::KeyS,
            Shortcut::EnterViMode,
            Duration::ZERO,
        ));
        app.world_mut().spawn((OrzmaTerminal, KeyboardFocused));
        let webview = app.world_mut().spawn_empty().id();
        app.world_mut().resource_mut::<FocusedWebview>().0 = Some(webview);
        app.world_mut().resource_mut::<CefKeyboardFilter>().set([(
            webview,
            KeyCode::KeyS,
            ModifiersState::default(),
        )]);
        {
            let mut windows = app
                .world_mut()
                .query_filtered::<&mut Window, With<PrimaryWindow>>();
            windows.single_mut(app.world_mut()).unwrap().focused = false;
        }
        *app.world_mut().resource_mut::<LeaderPhase>() = LeaderPhase::Pending;
        press_key(&mut app, KeyCode::KeyS, Key::Character("s".into()));
        app.update();
        assert!(
            !app.world().resource::<CefKeyboardFilter>().contains(
                webview,
                KeyCode::KeyS,
                ModifiersState::default()
            ),
            "a guarded frame clears the filter so a stale claim never lingers"
        );
    }

    #[test]
    fn filter_cleared_when_nothing_claimed() {
        let mut app = resolve_app(Shortcuts::default());
        let term = app.world_mut().spawn((OrzmaTerminal, KeyboardFocused)).id();
        let stale = app.world_mut().spawn_empty().id();
        app.world_mut().resource_mut::<CefKeyboardFilter>().set([(
            stale,
            KeyCode::KeyS,
            ModifiersState::default(),
        )]);
        press_key(&mut app, KeyCode::KeyA, Key::Character("a".into()));
        app.update();
        assert!(
            !app.world().resource::<CefKeyboardFilter>().contains(
                stale,
                KeyCode::KeyS,
                ModifiersState::default()
            ),
            "a frame that claims nothing clears the filter"
        );
        let _ = term;
    }

    #[derive(Resource, Default)]
    struct DeliverProbe {
        saw_claim: bool,
    }

    fn deliver_probe(
        mut probe: ResMut<DeliverProbe>,
        filter: Res<CefKeyboardFilter>,
        webview: Res<ProbeWebview>,
    ) {
        if let Some(webview) = webview.0 {
            probe.saw_claim = filter.contains(webview, KeyCode::KeyS, ModifiersState::default());
        }
    }

    #[derive(Resource, Default)]
    struct ProbeWebview(Option<Entity>);

    #[test]
    fn filter_is_populated_before_keyboard_deliver_set() {
        let mut app = resolve_app(test_shortcuts_with_repeat_prefix(
            KeyCode::KeyS,
            Shortcut::EnterViMode,
            Duration::ZERO,
        ));
        app.init_resource::<DeliverProbe>()
            .init_resource::<ProbeWebview>()
            .add_systems(Update, deliver_probe.in_set(KeyboardDeliverSet));
        app.world_mut().spawn((OrzmaTerminal, KeyboardFocused));
        let webview = app.world_mut().spawn_empty().id();
        app.world_mut().resource_mut::<FocusedWebview>().0 = Some(webview);
        app.world_mut().resource_mut::<ProbeWebview>().0 = Some(webview);
        *app.world_mut().resource_mut::<LeaderPhase>() = LeaderPhase::Pending;
        press_key(&mut app, KeyCode::KeyS, Key::Character("s".into()));
        app.edit_schedule(Update, |schedule| {
            schedule.set_build_settings(ScheduleBuildSettings {
                ambiguity_detection: LogLevel::Error,
                ..Default::default()
            });
        });
        app.update();
        assert!(
            app.world().resource::<DeliverProbe>().saw_claim,
            "resolve_key_effects must populate CefKeyboardFilter before KeyboardDeliverSet runs"
        );
    }
}
