//! Central keyboard-shortcut resolution: `resolve_shortcuts` runs in both
//! `AppMode`s, resolves the frame's pressed keys through the pure
//! `crate::input::resolve::classify_key_batch` decider, handles the two
//! mode-independent effects inline (Quit → `AppExit`, release-webview-focus →
//! clear `FocusedWebview`), and emits the remaining effects as a single
//! `ShortcutBatch` message. The per-mode appliers (`default_mode`,
//! `tmux::input`) consume that batch and apply the mode-specific events. This is
//! the sole system that steps `LeaderPhase`.

use crate::action::vi::ResolvedCopyModeKeys;
use crate::input::focus::KeyboardFocused;
use crate::input::ime::ImeState;
use crate::input::resolve::{BatchContext, KeyEffect, classify_key_batch};
use crate::input::shortcuts::{LeaderGate, LeaderPhase, Shortcuts, clear_leader_phase};
use crate::input::{InputPhase, current_modifiers};
use crate::surface::OzmaTerminal;
use crate::ui::copy_mode::CopyModeState;
use crate::ui::copy_search::CopyPrompt;
use crate::ui::tmux::confirm_prompt::ConfirmState;
use crate::ui::tmux::rename_prompt::RenamePrompt;
use bevy::input::keyboard::{KeyCode, KeyboardInput};
use bevy::prelude::*;
use bevy::time::Real;
use bevy::window::PrimaryWindow;
use bevy_cef::prelude::FocusedWebview;
use ozma_webview::ForwardKeys;
use ozmux_configs::shortcuts::{Modifiers, ShortcutAction};

/// The frame's resolved shortcut effects handed from `resolve_shortcuts` to the
/// per-mode appliers. Excludes `Quit` and `ReleaseWebviewFocus`, which
/// `resolve_shortcuts` handles inline. `focused` is the `KeyboardFocused`
/// `OzmaTerminal` (the Default terminal or the active tmux pane).
#[derive(Message)]
pub(in crate::input) struct ShortcutBatch {
    /// The mode-specific effects to apply (no `Quit` / `ReleaseWebviewFocus`).
    pub(in crate::input) effects: Vec<KeyEffect>,
    /// The `KeyboardFocused` `OzmaTerminal`, or `None` when none is focused.
    pub(in crate::input) focused: Option<Entity>,
    /// Whether the focused surface is in copy mode.
    pub(in crate::input) in_copy_mode: bool,
    /// The frame's modifier snapshot, shared by every effect in the batch.
    pub(in crate::input) mods: Modifiers,
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
                    .in_set(InputPhase::FocusedKey)
                    .in_set(ShortcutSet::Resolve)
                    .in_set(LeaderGate::Advance)
                    .run_if(on_message::<KeyboardInput>),
            );
    }
}

/// Resolves the frame's pressed keys into a `ShortcutBatch`. Runs in both
/// `AppMode`s (gated only on `on_message::<KeyboardInput>`), in
/// `InputPhase::FocusedKey` / `ShortcutSet::Resolve` / `LeaderGate::Advance`.
/// The sole `LeaderPhase`-stepping system: on a coarse guard (a modal prompt,
/// IME composition, or an unfocused window) it clears the leader, drains the
/// frame's keys, and emits no batch; otherwise it classifies the keys, applies
/// `Quit` (`AppExit`) and `ReleaseWebviewFocus` (clear `FocusedWebview`) inline,
/// and writes the remaining effects as one `ShortcutBatch`.
fn resolve_shortcuts(
    mut exit: MessageWriter<AppExit>,
    mut events: MessageReader<KeyboardInput>,
    mut focused_webview: ResMut<FocusedWebview>,
    mut leader_phase: ResMut<LeaderPhase>,
    mut batch: MessageWriter<ShortcutBatch>,
    (copy_prompt, confirm_state, rename_prompt, ime): (
        Res<CopyPrompt>,
        Option<Res<ConfirmState>>,
        Option<Res<RenamePrompt>>,
        Res<ImeState>,
    ),
    (shortcuts, resolved_copy, bevy_keys, time): (
        Res<Shortcuts>,
        Res<ResolvedCopyModeKeys>,
        Res<ButtonInput<KeyCode>>,
        Res<Time<Real>>,
    ),
    windows: Query<&Window, With<PrimaryWindow>>,
    focused_surface: Query<Entity, (With<OzmaTerminal>, With<KeyboardFocused>)>,
    copy_modes: Query<(), With<CopyModeState>>,
    forward_keys: Query<&ForwardKeys>,
) {
    // NOTE: each modal owner (copy-mode prompt, confirm-before prompt, rename
    // prompt) holds the keyboard and reads raw keys in its own system; while
    // composing, replaying preedit keys would garble IME + double-send; an
    // unfocused window must not act. Drain (don't replay) so no key leaks to the
    // terminal, tmux, or the prefix state machine — and emit no batch.
    let focused_window = windows.single().map(|w| w.focused).unwrap_or(false);
    if copy_prompt.open.is_some()
        || confirm_state.is_some()
        || rename_prompt.is_some()
        || ime.is_composing()
        || !focused_window
    {
        clear_leader_phase(&mut leader_phase);
        events.clear();
        return;
    }

    let focused = focused_surface.single().ok();
    let in_copy_mode = focused.is_some_and(|entity| copy_modes.get(entity).is_ok());
    let webview_focused = focused_webview.0.is_some();
    let forward_chords = focused_webview
        .0
        .and_then(|entity| forward_keys.get(entity).ok())
        .map(|chords| chords.0.as_slice())
        .unwrap_or(&[]);
    let mods = current_modifiers(&bevy_keys);
    let ctx = BatchContext {
        mods,
        now: time.elapsed(),
        in_copy_mode,
        webview_focused,
        forward_chords,
    };
    let all = classify_key_batch(
        &mut leader_phase,
        &shortcuts,
        &resolved_copy,
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::shortcuts::test_shortcuts_with_direct_chord;
    use crate::ui::tmux::pane_focus::sync_keyboard_focus_to_active_pane;
    use bevy::input::ButtonState;
    use bevy::input::keyboard::Key;
    use ozmux_tmux::{ActivePane, PaneId, TmuxPane};
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
            .add_plugins(DispatchPlugin)
            .add_message::<KeyboardInput>()
            .add_message::<AppExit>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ImeState>()
            .init_resource::<FocusedWebview>()
            .init_resource::<LeaderPhase>()
            .init_resource::<ResolvedCopyModeKeys>()
            .init_resource::<CopyPrompt>()
            .init_resource::<Captured>()
            .insert_resource(shortcuts)
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
        assert!(
            cap.mods.is_some(),
            "the batch carries the frame's modifiers"
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

    #[test]
    fn quit_writes_appexit_not_in_batch() {
        let mut app = resolve_app(test_shortcuts_with_direct_chord(
            KeyCode::KeyQ,
            Modifiers {
                ctrl: false,
                shift: false,
                alt: false,
                meta: true,
            },
            ShortcutAction::Quit,
        ));
        app.world_mut().spawn((OzmaTerminal, KeyboardFocused));
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
    fn mirror_freshness_before_focusedkey() {
        let mut app = resolve_app(Shortcuts::default());
        app.add_systems(
            Update,
            sync_keyboard_focus_to_active_pane.before(InputPhase::FocusedKey),
        );
        let p1 = app
            .world_mut()
            .spawn((OzmaTerminal, tmux_pane(1), ActivePane, KeyboardFocused))
            .id();
        let p2 = app.world_mut().spawn((OzmaTerminal, tmux_pane(2))).id();
        // ActivePane moves p1 -> p2 this tick; the mirror (before FocusedKey)
        // must refresh KeyboardFocused so resolve_shortcuts reads p2 as focused.
        app.world_mut().entity_mut(p1).remove::<ActivePane>();
        app.world_mut().entity_mut(p2).insert(ActivePane);
        press_key(&mut app, KeyCode::KeyA, Key::Character("a".into()));
        app.update();
        assert_eq!(
            app.world().resource::<Captured>().focused,
            Some(p2),
            "the mirror edge makes batch.focused reflect the new active pane the same frame"
        );
    }
}
