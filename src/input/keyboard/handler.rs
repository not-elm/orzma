//! `AppMode`s, resolves the frame's pressed keys through the pure
//! `crate::input::resolve::classify_key_batch` decider, handles the two
//! mode-independent effects inline (Quit → `AppExit`, release-webview-focus →
//! clear `FocusedWebview`), and fans out the remaining effects as the four
//! per-responsibility shortcut messages (`ShortcutMessage`, `CopyModeMessage`,
//! `TypeMessage`, `WebviewForwardMessage`). The per-mode appliers
//! (`crate::input::shortcuts::default_mode`, `crate::input::shortcuts::tmux`)
//! consume those messages and apply the mode-specific events. This is the
//! sole system that steps `LeaderPhase`.

use crate::action::vi::ResolvedCopyModeKeys;
use crate::app_mode::AppMode;
use crate::input::current_modifiers;
use crate::input::focus::KeyboardFocused;
use crate::input::ime::{ImeState, resolve_focused_surface};
use crate::input::keyboard::key_effect::{
    BatchContext, ClassifiedKeys, KeyEffect, classify_key_batch,
};
use crate::input::shortcuts::{
    CopyModeMessage, HeldRepeatKey, LeaderGate, LeaderPhase, ShortcutMessage, ShortcutMessages,
    ShortcutSet, Shortcuts, TypeMessage, WebviewForwardMessage, clear_leader_phase,
};
use crate::ui::copy_mode::CopyModeState;
use crate::ui::copy_search::CopyPrompt;
use crate::ui::tmux::confirm_prompt::ConfirmState;
use crate::ui::tmux::rename_prompt::RenamePrompt;
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

/// The modal-guard and mode inputs `resolve_key_effects` reads: the tmux modal
/// prompts, IME state, and the active `AppMode` that gates the tmux-only prompt
/// guards.
#[derive(SystemParam)]
struct ModalGuards<'w> {
    copy_prompt: Res<'w, CopyPrompt>,
    confirm_state: Option<Res<'w, ConfirmState>>,
    rename_prompt: Option<Res<'w, RenamePrompt>>,
    ime: Res<'w, ImeState>,
    app_mode: Res<'w, State<AppMode>>,
}

/// The classifier inputs `resolve_key_effects` feeds to `classify_key_batch`: the
/// shortcut table, resolved copy-mode keys, held modifier keys, and the
/// real-time clock the leader timeout is measured against.
#[derive(SystemParam)]
struct ClassifyInputs<'w> {
    shortcuts: Res<'w, Shortcuts>,
    resolved_copy: Res<'w, ResolvedCopyModeKeys>,
    bevy_keys: Res<'w, ButtonInput<KeyCode>>,
    time: Res<'w, Time<Real>>,
}

/// Resolves the frame's pressed keys and fans out the per-responsibility
/// shortcut messages. Runs in both `AppMode`s (gated only on
/// `on_message::<KeyboardInput>`), in `InputPhase::FocusedKey` /
/// `ShortcutSet::Resolve` / `LeaderGate::Advance`. The sole `LeaderPhase`-stepping
/// system: on a coarse guard (a tmux modal prompt, IME composition, or an
/// unfocused window) it clears the leader, drains the frame's keys, and writes no
/// messages; otherwise it classifies the keys, applies `Quit` (`AppExit`) and
/// `ReleaseWebviewFocus` (clear `FocusedWebview`) inline, and writes every other
/// effect to its typed message (`ShortcutMessage`, `CopyModeMessage`,
/// `TypeMessage`, `WebviewForwardMessage`).
fn resolve_key_effects(
    mut exit: MessageWriter<AppExit>,
    mut events: MessageReader<KeyboardInput>,
    mut focused_webview: ResMut<FocusedWebview>,
    mut cef_filter: ResMut<CefKeyboardFilter>,
    mut leader_phase: ResMut<LeaderPhase>,
    mut held_repeat: ResMut<HeldRepeatKey>,
    mut messages: ShortcutMessages,
    guards: ModalGuards,
    inputs: ClassifyInputs,
    windows: Query<&Window, With<PrimaryWindow>>,
    focused_surface: Query<Entity, With<KeyboardFocused>>,
    copy_modes: Query<(), With<CopyModeState>>,
    forward_keys: Query<&ForwardKeys>,
) {
    let mode = guards.app_mode.get().clone();
    let focused_window = windows.single().map(|w| w.focused).unwrap_or(false);
    // NOTE: the prompt guards (copy-mode prompt, confirm-before, rename) are
    // tmux-only by design. Those resources are set by tmux actions and cleared
    // only by their own handlers — never on a mode transition — so a prompt left
    // open when the tmux connection drops (falling back to Default) would
    // otherwise drain EVERY key and freeze Default keyboard input. IME + window
    // focus guard both modes. When a guard fires, drain (don't replay) the
    // frame's keys and write no messages, so no key leaks to the terminal,
    // tmux, or the prefix state machine (and no preedit key is double-sent).
    let prompt_open = mode == AppMode::Tmux
        && (guards.copy_prompt.open.is_some()
            || guards.confirm_state.is_some()
            || guards.rename_prompt.is_some());
    if prompt_open || guards.ime.is_composing() || !focused_window {
        clear_leader_phase(&mut leader_phase);
        if held_repeat.0.is_some() {
            held_repeat.0 = None;
        }
        clear_cef_filter(&mut cef_filter);
        events.clear();
        return;
    }

    let focused = resolve_focused_surface(&focused_surface);
    let in_copy_mode = focused.is_some_and(|entity| copy_modes.get(entity).is_ok());
    let forward_chords = focused_webview
        .0
        .and_then(|entity| forward_keys.get(entity).ok())
        .map(|chords| chords.0.as_slice())
        .unwrap_or(&[]);
    let mods = current_modifiers(&inputs.bevy_keys);
    let ctx = BatchContext {
        mods,
        now: inputs.time.elapsed(),
        in_copy_mode,
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
        &inputs.resolved_copy,
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
                    in_copy_mode,
                });
            }
            KeyEffect::CopyMode(action) => {
                messages
                    .copy_mode
                    .write(CopyModeMessage { action, focused });
            }
            KeyEffect::Type { logical, key_code } => {
                messages.type_keys.write(TypeMessage {
                    logical,
                    key_code,
                    focused,
                    mods,
                });
            }
            KeyEffect::WebviewForward { logical, key_code } => {
                messages.webview_forward.write(WebviewForwardMessage {
                    logical,
                    key_code,
                    focused,
                    mods,
                });
            }
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
    use bevy::state::app::StatesPlugin;
    use orzma_configs::shortcuts::Modifiers;
    use orzma_tmux::{PaneId, TmuxPane};
    use std::time::Duration;
    use tmux_control_parser::CellDims;

    #[derive(Resource, Default)]
    struct Captured {
        app_exit: usize,
        shortcuts: Vec<(Shortcut, bool)>,
        copy_mode: usize,
        typed: usize,
        webview_forward: usize,
        focused: Option<Entity>,
        in_copy_mode: Option<bool>,
        mods: Option<Modifiers>,
        last_typed: Option<(Key, KeyCode)>,
    }

    impl Captured {
        fn message_count(&self) -> usize {
            self.shortcuts.len() + self.copy_mode + self.typed + self.webview_forward
        }
    }

    fn capture_messages(
        mut cap: ResMut<Captured>,
        mut shortcuts: MessageReader<ShortcutMessage>,
        mut copy_mode: MessageReader<CopyModeMessage>,
        mut typed: MessageReader<TypeMessage>,
        mut webview_forward: MessageReader<WebviewForwardMessage>,
    ) {
        for m in shortcuts.read() {
            cap.shortcuts.push((m.action, m.via_leader));
            cap.focused = m.focused;
            cap.in_copy_mode = Some(m.in_copy_mode);
        }
        for m in copy_mode.read() {
            cap.copy_mode += 1;
            cap.focused = m.focused;
        }
        for m in typed.read() {
            cap.typed += 1;
            cap.focused = m.focused;
            cap.mods = Some(m.mods);
            cap.last_typed = Some((m.logical.clone(), m.key_code));
        }
        for m in webview_forward.read() {
            cap.webview_forward += 1;
            cap.focused = m.focused;
            cap.mods = Some(m.mods);
        }
    }

    fn capture_exit(mut reader: MessageReader<AppExit>, mut cap: ResMut<Captured>) {
        cap.app_exit += reader.read().count();
    }

    fn resolve_app(shortcuts: Shortcuts) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(StatesPlugin)
            .add_plugins(KeyboardHandlerPlugin)
            .add_message::<KeyboardInput>()
            .add_message::<ShortcutMessage>()
            .add_message::<CopyModeMessage>()
            .add_message::<TypeMessage>()
            .add_message::<WebviewForwardMessage>()
            .add_message::<AppExit>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ImeState>()
            .init_resource::<FocusedWebview>()
            .init_resource::<CefKeyboardFilter>()
            .init_resource::<LeaderPhase>()
            .init_resource::<HeldRepeatKey>()
            .init_resource::<ResolvedCopyModeKeys>()
            .init_resource::<CopyPrompt>()
            .init_resource::<Captured>()
            .insert_resource(shortcuts)
            .insert_state(AppMode::Default)
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

    fn tmux_pane(id: u32) -> TmuxPane {
        TmuxPane {
            id: PaneId(id),
            dims: CellDims {
                width: 80,
                height: 24,
                xoff: 0,
                yoff: 0,
            },
        }
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
            Some((Key::Character("a".into()), KeyCode::KeyA)),
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
    fn focused_resolves_for_tmux_pane() {
        let mut app = resolve_app(Shortcuts::default());
        let pane = app
            .world_mut()
            .spawn((OrzmaTerminal, tmux_pane(1), KeyboardFocused))
            .id();
        press_key(&mut app, KeyCode::KeyA, Key::Character("a".into()));
        app.update();
        assert_eq!(
            app.world().resource::<Captured>().focused,
            Some(pane),
            "a KeyboardFocused tmux pane resolves as the message's focused field"
        );
    }

    #[test]
    fn in_copy_mode_flag_set() {
        let mut app = resolve_app(test_shortcuts_with_direct_chord(
            KeyCode::KeyA,
            Modifiers::default(),
            Shortcut::EnterCopyMode,
        ));
        app.world_mut()
            .spawn((OrzmaTerminal, KeyboardFocused, CopyModeState));
        press_key(&mut app, KeyCode::KeyA, Key::Character("a".into()));
        app.update();
        assert_eq!(
            app.world().resource::<Captured>().in_copy_mode,
            Some(true),
            "a focused surface in copy mode sets ShortcutMessage.in_copy_mode"
        );
    }

    #[test]
    fn in_copy_mode_flag_clear_outside_copy_mode() {
        let mut app = resolve_app(test_shortcuts_with_direct_chord(
            KeyCode::KeyA,
            Modifiers::default(),
            Shortcut::EnterCopyMode,
        ));
        app.world_mut().spawn((OrzmaTerminal, KeyboardFocused));
        press_key(&mut app, KeyCode::KeyA, Key::Character("a".into()));
        app.update();
        assert_eq!(
            app.world().resource::<Captured>().in_copy_mode,
            Some(false),
            "a focused surface NOT in copy mode sets ShortcutMessage.in_copy_mode to false"
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
            Shortcut::EnterCopyMode,
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
            Shortcut::EnterCopyMode,
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
            Shortcut::EnterCopyMode,
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
