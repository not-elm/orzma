//! Central keyboard-shortcut resolution: `resolve_shortcuts` runs in both
//! `AppMode`s, resolves the frame's pressed keys through the pure
//! `crate::input::resolve::classify_key_batch` decider, handles the two
//! mode-independent effects inline (Quit → `AppExit`, release-webview-focus →
//! clear `FocusedWebview`), and emits the remaining effects as a single
//! `ShortcutBatch` message. The per-mode appliers (`default_mode`,
//! `tmux::input`) consume that batch and apply the mode-specific events. This is
//! the sole system that steps `LeaderPhase`.

use crate::action::vi::ResolvedCopyModeKeys;
use crate::app_mode::AppMode;
use crate::input::focus::KeyboardFocused;
use crate::input::ime::{ImeState, resolve_focused_surface};
use crate::input::resolve::{BatchContext, ClassifiedKeys, KeyEffect, classify_key_batch};
use crate::input::shortcuts::{LeaderGate, LeaderPhase, Shortcuts, clear_leader_phase};
use crate::input::{InputPhase, current_modifiers};
use crate::ui::copy_mode::CopyModeState;
use crate::ui::copy_search::CopyPrompt;
use crate::ui::tmux::confirm_prompt::ConfirmState;
use crate::ui::tmux::rename_prompt::RenamePrompt;
use bevy::ecs::system::SystemParam;
use bevy::input::keyboard::{KeyCode, KeyboardInput};
use bevy::prelude::*;
use bevy::time::Real;
use bevy::window::PrimaryWindow;
use bevy_cef::prelude::{CefKeyboardFilter, FocusedWebview, ModifiersState};
use ozma_webview::ForwardKeys;
use ozmux_configs::shortcuts::{Modifiers, ShortcutAction};

/// The frame's resolved shortcut effects handed from `resolve_shortcuts` to the
/// per-mode appliers. Excludes `Quit` and `ReleaseWebviewFocus`, which
/// `resolve_shortcuts` handles inline. `focused` is the `KeyboardFocused`
/// `OzmaTerminal` (the Default terminal or the active tmux pane).
#[derive(Message)]
pub(in crate::input) struct ShortcutBatch {
    /// The mode-specific effects to apply (no `Quit` / `ReleaseWebviewFocus`).
    pub effects: Vec<KeyEffect>,
    /// The `KeyboardFocused` `OzmaTerminal`, or `None` when none is focused.
    pub focused: Option<Entity>,
    /// Whether the focused surface is in copy mode.
    pub in_copy_mode: bool,
    /// The frame's modifier snapshot, shared by every effect in the batch.
    pub mods: Modifiers,
}

/// Orders the two halves of shortcut dispatch inside `InputPhase::FocusedKey`:
/// `resolve_shortcuts` (`Resolve`) writes the `ShortcutBatch` before the
/// per-mode appliers (`Apply`) read it, so the batch is consumed the same frame.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(in crate::input) enum ShortcutSet {
    /// `resolve_shortcuts`: classifies keys and writes the `ShortcutBatch`.
    Resolve,
    /// The per-mode appliers: read the `ShortcutBatch` and apply its effects.
    Apply,
}

/// Registers `resolve_shortcuts` and the `ShortcutSet` ordering.
pub(super) struct DispatchPlugin;

impl Plugin for DispatchPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<ShortcutBatch>()
            .configure_sets(
                Update,
                (ShortcutSet::Resolve, ShortcutSet::Apply)
                    .chain()
                    .in_set(InputPhase::FocusedKey),
            )
            .add_systems(
                Update,
                resolve_shortcuts
                    .in_set(ShortcutSet::Resolve)
                    .in_set(LeaderGate::Advance)
                    .run_if(on_message::<KeyboardInput>),
            );
    }
}

/// The modal-guard and mode inputs `resolve_shortcuts` reads: the tmux modal
/// prompts, IME state, and the active `AppMode` stamped onto each batch.
#[derive(SystemParam)]
struct ModalGuards<'w> {
    copy_prompt: Res<'w, CopyPrompt>,
    confirm_state: Option<Res<'w, ConfirmState>>,
    rename_prompt: Option<Res<'w, RenamePrompt>>,
    ime: Res<'w, ImeState>,
    app_mode: Res<'w, State<AppMode>>,
}

/// The classifier inputs `resolve_shortcuts` feeds to `classify_key_batch`: the
/// shortcut table, resolved copy-mode keys, held modifier keys, and the
/// real-time clock the leader timeout is measured against.
#[derive(SystemParam)]
struct ClassifyInputs<'w> {
    shortcuts: Res<'w, Shortcuts>,
    resolved_copy: Res<'w, ResolvedCopyModeKeys>,
    bevy_keys: Res<'w, ButtonInput<KeyCode>>,
    time: Res<'w, Time<Real>>,
}

/// Resolves the frame's pressed keys into a `ShortcutBatch`. Runs in both
/// `AppMode`s (gated only on `on_message::<KeyboardInput>`), in
/// `InputPhase::FocusedKey` / `ShortcutSet::Resolve` / `LeaderGate::Advance`.
/// The sole `LeaderPhase`-stepping system: on a coarse guard (a tmux modal
/// prompt, IME composition, or an unfocused window) it clears the leader, drains
/// the frame's keys, and emits no batch; otherwise it classifies the keys,
/// applies `Quit` (`AppExit`) and `ReleaseWebviewFocus` (clear `FocusedWebview`)
/// inline, and writes the remaining effects as one `ShortcutBatch` stamped with
/// the resolving `AppMode`.
fn resolve_shortcuts(
    mut exit: MessageWriter<AppExit>,
    mut events: MessageReader<KeyboardInput>,
    mut focused_webview: ResMut<FocusedWebview>,
    mut cef_filter: ResMut<CefKeyboardFilter>,
    mut leader_phase: ResMut<LeaderPhase>,
    mut batch: MessageWriter<ShortcutBatch>,
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
    // frame's keys and emit no batch, so no key leaks to the terminal, tmux, or
    // the prefix state machine (and no preedit key is double-sent).
    let prompt_open = mode == AppMode::Tmux
        && (guards.copy_prompt.open.is_some()
            || guards.confirm_state.is_some()
            || guards.rename_prompt.is_some());
    if prompt_open || guards.ime.is_composing() || !focused_window {
        clear_leader_phase(&mut leader_phase);
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
    // Snapshot the webview entity BEFORE the effects loop below runs
    // `ReleaseWebviewFocus`, which sets `focused_webview.0 = None`. Building the
    // filter from `focused_webview.0` afterwards would drop the suppression on a
    // frame carrying both a leader claim and a release chord.
    let suppress_target = focused_webview.0;
    let ClassifiedKeys {
        effects: all,
        webview_suppressed,
    } = classify_key_batch(
        &mut leader_phase,
        &inputs.shortcuts,
        &inputs.resolved_copy,
        events.read(),
        ctx,
    );

    let mut effects = Vec::with_capacity(all.len());
    for effect in all {
        match effect {
            KeyEffect::Action {
                action: ShortcutAction::Quit,
                ..
            } => {
                exit.write(AppExit::Success);
            }
            KeyEffect::ReleaseWebviewFocus => focused_webview.0 = None,
            other => effects.push(other),
        }
    }
    batch.write(ShortcutBatch {
        effects,
        focused,
        in_copy_mode,
        mods,
    });
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
    use crate::surface::OzmaTerminal;
    use bevy::input::ButtonState;
    use bevy::input::keyboard::Key;
    use bevy::state::app::StatesPlugin;
    use ozmux_tmux::{PaneId, TmuxPane};
    use tmux_control_parser::CellDims;

    #[derive(Resource, Default)]
    struct Captured {
        count: usize,
        app_exit: usize,
        effects: Vec<KeyEffect>,
        focused: Option<Entity>,
        in_copy_mode: bool,
        mods: Option<Modifiers>,
    }

    fn capture_batch(mut reader: MessageReader<ShortcutBatch>, mut cap: ResMut<Captured>) {
        for b in reader.read() {
            cap.count += 1;
            cap.effects = b.effects.clone();
            cap.focused = b.focused;
            cap.in_copy_mode = b.in_copy_mode;
            cap.mods = Some(b.mods);
        }
    }

    fn capture_exit(mut reader: MessageReader<AppExit>, mut cap: ResMut<Captured>) {
        cap.app_exit += reader.read().count();
    }

    fn resolve_app(shortcuts: Shortcuts) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(StatesPlugin)
            .add_plugins(DispatchPlugin)
            .add_message::<KeyboardInput>()
            .add_message::<AppExit>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ImeState>()
            .init_resource::<FocusedWebview>()
            .init_resource::<CefKeyboardFilter>()
            .init_resource::<LeaderPhase>()
            .init_resource::<ResolvedCopyModeKeys>()
            .init_resource::<CopyPrompt>()
            .init_resource::<Captured>()
            .insert_resource(shortcuts)
            .insert_state(AppMode::Default)
            .add_systems(
                Update,
                (capture_batch, capture_exit).in_set(ShortcutSet::Apply),
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
    fn normal_batch_carries_effects_focused_in_copy_mode() {
        let mut app = resolve_app(Shortcuts::default());
        let term = app.world_mut().spawn((OzmaTerminal, KeyboardFocused)).id();
        press_key(&mut app, KeyCode::KeyA, Key::Character("a".into()));
        app.update();
        let cap = app.world().resource::<Captured>();
        assert_eq!(cap.count, 1, "exactly one ShortcutBatch per keyboard frame");
        assert_eq!(
            cap.effects,
            vec![KeyEffect::Type {
                logical: Key::Character("a".into()),
                key_code: KeyCode::KeyA,
            }],
            "a plain key resolves to one Type effect carried in the batch"
        );
        assert_eq!(cap.focused, Some(term));
        assert!(!cap.in_copy_mode);
        assert_eq!(
            cap.mods,
            Some(Modifiers::default()),
            "no modifier keys are held, so the batch carries the default modifiers"
        );
    }

    #[test]
    fn guarded_frame_emits_no_batch() {
        let mut app = resolve_app(Shortcuts::default());
        app.world_mut().spawn((OzmaTerminal, KeyboardFocused));
        // Window unfocused: a coarse guard drains the frame with no batch.
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
            app.world().resource::<Captured>().count,
            0,
            "a guarded frame emits no ShortcutBatch"
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
            ShortcutAction::Quit,
        ));
        if spawn_focused {
            app.world_mut().spawn((OzmaTerminal, KeyboardFocused));
        }
        app
    }

    #[test]
    fn quit_writes_appexit_not_in_batch() {
        let mut app = quit_test_app(true);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::SuperLeft);
        press_key(&mut app, KeyCode::KeyQ, Key::Character("q".into()));
        app.update();
        let cap = app.world().resource::<Captured>();
        assert_eq!(cap.app_exit, 1, "Cmd+Q writes AppExit");
        assert_eq!(cap.count, 1, "the batch is still emitted");
        assert!(
            !cap.effects.iter().any(|e| matches!(
                e,
                KeyEffect::Action {
                    action: ShortcutAction::Quit,
                    ..
                }
            )),
            "Quit is handled inline and never reaches the batch"
        );
    }

    #[test]
    fn release_clears_webview_not_in_batch() {
        let mut app = resolve_app(test_shortcuts_with_direct_chord(
            KeyCode::Escape,
            Modifiers {
                ctrl: true,
                shift: true,
                alt: false,
                meta: false,
            },
            ShortcutAction::ReleaseWebviewFocus,
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
        assert!(
            !cap.effects
                .iter()
                .any(|e| matches!(e, KeyEffect::ReleaseWebviewFocus)),
            "ReleaseWebviewFocus is handled inline and never reaches the batch"
        );
    }

    #[test]
    fn focused_resolves_for_tmux_pane() {
        let mut app = resolve_app(Shortcuts::default());
        let pane = app
            .world_mut()
            .spawn((OzmaTerminal, tmux_pane(1), KeyboardFocused))
            .id();
        press_key(&mut app, KeyCode::KeyA, Key::Character("a".into()));
        app.update();
        assert_eq!(
            app.world().resource::<Captured>().focused,
            Some(pane),
            "a KeyboardFocused tmux pane resolves as batch.focused"
        );
    }

    #[test]
    fn in_copy_mode_flag_set() {
        let mut app = resolve_app(Shortcuts::default());
        app.world_mut()
            .spawn((OzmaTerminal, KeyboardFocused, CopyModeState));
        press_key(&mut app, KeyCode::KeyA, Key::Character("a".into()));
        app.update();
        assert!(
            app.world().resource::<Captured>().in_copy_mode,
            "a focused surface in copy mode sets batch.in_copy_mode"
        );
    }

    #[test]
    fn batch_consumed_same_update() {
        let mut app = resolve_app(Shortcuts::default());
        app.world_mut().spawn((OzmaTerminal, KeyboardFocused));
        press_key(&mut app, KeyCode::KeyA, Key::Character("a".into()));
        app.update();
        assert_eq!(
            app.world().resource::<Captured>().count,
            1,
            "the ShortcutSet Resolve->Apply chain lets the applier consume the batch \
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
            "Quit is handled inline in resolve_shortcuts regardless of focus, so Cmd+Q with no \
             focused terminal still writes AppExit"
        );
        assert!(
            !cap.effects.iter().any(|e| matches!(
                e,
                KeyEffect::Action {
                    action: ShortcutAction::Quit,
                    ..
                }
            )),
            "no Quit effect reaches the batch even when nothing is focused"
        );
    }

    #[test]
    fn filter_holds_leader_claim_under_webview_focus() {
        let mut app = resolve_app(test_shortcuts_with_repeat_prefix(
            KeyCode::KeyS,
            ShortcutAction::EnterCopyMode,
            std::time::Duration::ZERO,
        ));
        app.world_mut().spawn((OzmaTerminal, KeyboardFocused));
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
            ShortcutAction::EnterCopyMode,
            std::time::Duration::ZERO,
        ));
        app.world_mut().spawn((OzmaTerminal, KeyboardFocused));
        let webview = app.world_mut().spawn_empty().id();
        app.world_mut().resource_mut::<FocusedWebview>().0 = Some(webview);
        app.world_mut().resource_mut::<CefKeyboardFilter>().set([(
            webview,
            KeyCode::KeyS,
            ModifiersState::default(),
        )]);
        // Window unfocused → coarse guard fires.
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
        let term = app.world_mut().spawn((OzmaTerminal, KeyboardFocused)).id();
        let stale = app.world_mut().spawn_empty().id();
        app.world_mut().resource_mut::<CefKeyboardFilter>().set([(
            stale,
            KeyCode::KeyS,
            ModifiersState::default(),
        )]);
        // No webview focused; a plain key claims nothing.
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
}
