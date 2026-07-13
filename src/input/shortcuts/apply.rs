//! The shortcut appliers: reads `ShortcutMessage`, `ViModeMessage`, and
//! `TypeMessage` from `resolve_key_effects` and applies vi-mode entry, paste,
//! copy, and raw-key typing to the focused terminal.

use crate::{
    action::{
        clipboard::PasteAction,
        terminal::trigger_selection_copy,
        vi::{mode::EnterViModeActionEvent, trigger_vi_mode_action},
    },
    input::{
        keyboard::bevy_key_to_terminal_key,
        shortcuts::{ShortcutMessage, ShortcutSet, TypeMessage, ViModeMessage},
    },
    ui::multiplexer::{
        confirm_prompt::ConfirmState, modal::any_modal_open, rename_prompt::RenameState,
    },
};
use bevy::prelude::*;
use orzma_configs::shortcuts::Shortcut;
use orzma_tty_engine::{TerminalKeyInput, TerminalModifiers};

pub(super) struct ShortcutsApplyPlugin;

impl Plugin for ShortcutsApplyPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                apply_shortcuts
                    .in_set(ShortcutSet::Apply)
                    .run_if(on_message::<ShortcutMessage>),
                apply_vi_mode
                    .in_set(ShortcutSet::Apply)
                    .run_if(on_message::<ViModeMessage>)
                    .after(apply_shortcuts),
                apply_type
                    .in_set(ShortcutSet::Apply)
                    .run_if(on_message::<TypeMessage>)
                    .after(apply_shortcuts)
                    .after(apply_vi_mode),
            ),
        );
    }
}

/// Applies keyboard shortcuts from `ShortcutMessage`: vi-mode entry, paste
/// (direct paste fires outside vi mode; a leader paste fires unconditionally),
/// and copy (fires unconditionally — vi mode included; no-selection is a
/// no-op downstream). `Quit` / `ReleaseWebviewFocus` are handled upstream in
/// `resolve_key_effects`; pane/window actions are handled by the multiplexer
/// applier (`input/shortcuts/multiplexer.rs`).
/// Registered in `ShortcutSet::Apply`, gated on `on_message::<ShortcutMessage>`.
fn apply_shortcuts(mut commands: Commands, mut shortcuts: MessageReader<ShortcutMessage>) {
    for msg in shortcuts.read() {
        match msg.action {
            Shortcut::EnterViMode => {
                if let Some(entity) = msg.focused {
                    commands.trigger(EnterViModeActionEvent { entity });
                }
            }
            Shortcut::Paste => {
                if let Some(entity) = msg.focused
                    && (msg.via_leader || !msg.in_vi_mode)
                {
                    commands.trigger(PasteAction { entity });
                }
            }
            Shortcut::Copy => trigger_selection_copy(&mut commands, msg.focused),
            Shortcut::SelectPane(_)
            | Shortcut::ResizePane(_)
            | Shortcut::SplitPane(_)
            | Shortcut::KillPane
            | Shortcut::ZoomPane
            | Shortcut::NewWindow
            | Shortcut::KillWindow
            | Shortcut::NextWindow
            | Shortcut::PreviousWindow
            | Shortcut::SelectWindow(_)
            | Shortcut::RenameWindow
            | Shortcut::Quit
            | Shortcut::ReleaseWebviewFocus => {}
        }
    }
}

/// Applies matched `[vi-mode]` keys from `ViModeMessage` on the focused
/// terminal. Registered in `ShortcutSet::Apply`, gated on
/// `on_message::<ViModeMessage>`.
fn apply_vi_mode(mut commands: Commands, mut vi_mode: MessageReader<ViModeMessage>) {
    for msg in vi_mode.read() {
        if let Some(entity) = msg.focused {
            trigger_vi_mode_action(&mut commands, entity, msg.action);
        }
    }
}

/// Types raw keys from `TypeMessage` into the focused terminal as
/// `TerminalKeyInput`. Runs after the shortcut/copy appliers. Registered in
/// `ShortcutSet::Apply`, gated on `on_message::<TypeMessage>`. While a
/// `ConfirmState` or `RenameState` prompt is open, this DRAINS `TypeMessage`s
/// instead of typing them, so the y/n answering a kill-pane / kill-window
/// confirm, or a character typed into the rename prompt, never leaks into
/// the terminal's PTY.
fn apply_type(
    mut commands: Commands,
    mut type_keys: MessageReader<TypeMessage>,
    confirm: Option<Res<ConfirmState>>,
    rename: Option<Res<RenameState>>,
) {
    // NOTE: while a confirm or rename prompt owns the keyboard, DRAIN (clear)
    // TypeMessages rather than gating this system off with run_if — a run_if
    // would leave the confirming/typed key buffered on apply_type's reader
    // cursor and inject it into the PTY on the next ungated frame (the reader
    // cursor only advances when the body runs).
    if any_modal_open(confirm, rename) {
        type_keys.clear();
        return;
    }
    for msg in type_keys.read() {
        if let Some(entity) = msg.focused
            && let Some(key) = bevy_key_to_terminal_key(&msg.logical)
        {
            let terminal_mods = TerminalModifiers {
                ctrl: msg.mods.ctrl,
                shift: msg.mods.shift,
                alt: msg.mods.alt,
                meta: msg.mods.meta,
            };
            commands.trigger(TerminalKeyInput {
                entity,
                key,
                modifiers: terminal_mods,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::terminal::TerminalSelectionCopy;
    use crate::input::keyboard::key_effect::KeyEffect;
    use crate::input::shortcuts::Shortcuts;
    use crate::surface::OrzmaTerminal;
    use bevy::ecs::resource::Resource;
    use bevy::input::keyboard::{Key, KeyCode};
    use bevy::prelude::{Entity, MinimalPlugins, On, ResMut};
    use orzma_configs::shortcuts::Modifiers;
    use orzma_tty_engine::TerminalKey;

    #[derive(Resource, Default)]
    struct Captured {
        vi_mode: u32,
        paste: u32,
        copy: u32,
        keys: Vec<TerminalKey>,
    }

    /// Builds an app running the three appliers as bare
    /// per-message consumers, capturing the events they trigger.
    fn build_dispatch_app(shortcuts: Shortcuts) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<ShortcutMessage>()
            .add_message::<ViModeMessage>()
            .add_message::<TypeMessage>()
            .init_resource::<Captured>()
            .insert_resource(shortcuts)
            .add_systems(
                Update,
                (
                    apply_shortcuts,
                    apply_vi_mode.after(apply_shortcuts),
                    apply_type.after(apply_shortcuts).after(apply_vi_mode),
                ),
            )
            .add_observer(|_ev: On<EnterViModeActionEvent>, mut c: ResMut<Captured>| {
                c.vi_mode += 1;
            })
            .add_observer(|_ev: On<PasteAction>, mut c: ResMut<Captured>| {
                c.paste += 1;
            })
            .add_observer(|_ev: On<TerminalSelectionCopy>, mut c: ResMut<Captured>| {
                c.copy += 1;
            })
            .add_observer(|ev: On<TerminalKeyInput>, mut c: ResMut<Captured>| {
                c.keys.push(ev.key.clone());
            });
        app
    }

    fn dispatch_app(shortcuts: Shortcuts) -> (App, Entity) {
        let mut app = build_dispatch_app(shortcuts);
        let term = app.world_mut().spawn(OrzmaTerminal).id();
        (app, term)
    }

    fn meta_mods() -> Modifiers {
        Modifiers {
            ctrl: false,
            shift: false,
            alt: false,
            meta: true,
        }
    }

    fn dispatch(
        app: &mut App,
        effects: Vec<KeyEffect>,
        focused: Option<Entity>,
        in_vi_mode: bool,
        mods: Modifiers,
    ) {
        for effect in effects {
            match effect {
                KeyEffect::Shortcut { action, via_leader } => {
                    app.world_mut().write_message(ShortcutMessage {
                        action,
                        via_leader,
                        focused,
                        in_vi_mode,
                    });
                }
                KeyEffect::ViMode(action) => {
                    app.world_mut()
                        .write_message(ViModeMessage { action, focused });
                }
                KeyEffect::Type { logical, .. } => {
                    app.world_mut().write_message(TypeMessage {
                        logical,
                        focused,
                        mods,
                    });
                }
                KeyEffect::WebviewForward { .. } => {}
            }
        }
        app.update();
    }

    fn type_effect(logical: Key, key_code: KeyCode) -> KeyEffect {
        KeyEffect::Type { logical, key_code }
    }

    fn action_effect(action: Shortcut, via_leader: bool) -> KeyEffect {
        KeyEffect::Shortcut { action, via_leader }
    }

    #[test]
    fn plain_key_triggers_terminal_key_input() {
        let (mut app, term) = dispatch_app(Shortcuts::default());
        dispatch(
            &mut app,
            vec![type_effect(Key::Character("a".into()), KeyCode::KeyA)],
            Some(term),
            false,
            Modifiers::default(),
        );
        assert_eq!(
            app.world().resource::<Captured>().keys,
            vec![TerminalKey::Text("a".into())],
            "a Type effect must forward to the focused terminal as a TerminalKeyInput"
        );
    }

    #[test]
    fn pane_action_is_noop() {
        let (mut app, term) = dispatch_app(Shortcuts::default());
        dispatch(
            &mut app,
            vec![action_effect(Shortcut::ZoomPane, true)],
            Some(term),
            false,
            Modifiers::default(),
        );
        let c = app.world().resource::<Captured>();
        assert_eq!(
            (c.vi_mode, c.paste, c.keys.len()),
            (0, 0, 0),
            "a pane action resolves to a no-op: no event, no typing"
        );
    }

    #[test]
    fn direct_paste_outside_vi_mode_pastes() {
        let (mut app, term) = dispatch_app(Shortcuts::default());
        dispatch(
            &mut app,
            vec![action_effect(Shortcut::Paste, false)],
            Some(term),
            false,
            meta_mods(),
        );
        assert_eq!(
            app.world().resource::<Captured>().paste,
            1,
            "a direct paste (via_leader=false) outside vi mode must fire PasteAction"
        );
    }

    #[test]
    fn direct_copy_outside_vi_mode_fires_selection_copy() {
        let (mut app, term) = dispatch_app(Shortcuts::default());
        dispatch(
            &mut app,
            vec![action_effect(Shortcut::Copy, false)],
            Some(term),
            false,
            meta_mods(),
        );
        assert_eq!(
            app.world().resource::<Captured>().copy,
            1,
            "a direct copy must fire TerminalSelectionCopy on the focused terminal"
        );
    }

    #[test]
    fn direct_copy_in_vi_mode_also_fires_selection_copy() {
        let (mut app, term) = dispatch_app(Shortcuts::default());
        dispatch(
            &mut app,
            vec![action_effect(Shortcut::Copy, false)],
            Some(term),
            true,
            meta_mods(),
        );
        assert_eq!(
            app.world().resource::<Captured>().copy,
            1,
            "copy fires unconditionally: vi mode must not suppress it (unlike paste)"
        );
    }

    #[test]
    fn direct_paste_in_vi_mode_suppressed() {
        let (mut app, term) = dispatch_app(Shortcuts::default());
        dispatch(
            &mut app,
            vec![action_effect(Shortcut::Paste, false)],
            Some(term),
            true,
            meta_mods(),
        );
        assert_eq!(
            app.world().resource::<Captured>().paste,
            0,
            "a direct paste in vi mode must be suppressed (via_leader || !in_vi_mode)"
        );
    }

    #[test]
    fn leader_paste_in_vi_mode_pastes() {
        let (mut app, term) = dispatch_app(Shortcuts::default());
        dispatch(
            &mut app,
            vec![action_effect(Shortcut::Paste, true)],
            Some(term),
            true,
            Modifiers::default(),
        );
        assert_eq!(
            app.world().resource::<Captured>().paste,
            1,
            "a leader-scoped paste (via_leader=true) must fire even in vi mode"
        );
    }

    #[test]
    fn enter_vi_mode_fires_even_when_already_in_vi_mode() {
        let (mut app, term) = dispatch_app(Shortcuts::default());
        dispatch(
            &mut app,
            vec![action_effect(Shortcut::EnterViMode, false)],
            Some(term),
            true,
            Modifiers::default(),
        );
        assert_eq!(
            app.world().resource::<Captured>().vi_mode,
            1,
            "EnterViMode must fire unconditionally, even when vi mode is already active"
        );
    }

    /// Builds an app running the real `ShortcutsApplyPlugin` registration (not
    /// a hand-rolled re-registration of `apply_type`), so a future change to
    /// how the plugin gates `apply_type` is caught by these tests too. Proves
    /// typing is suppressed while a kill-pane / kill-window confirm prompt or
    /// the rename prompt is open, and that a confirming/typed key never leaks
    /// into a later frame.
    fn build_confirm_gated_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<ShortcutMessage>()
            .add_message::<ViModeMessage>()
            .add_message::<TypeMessage>()
            .init_resource::<Captured>()
            .add_plugins(ShortcutsApplyPlugin)
            .add_observer(|ev: On<TerminalKeyInput>, mut c: ResMut<Captured>| {
                c.keys.push(ev.key.clone());
            });
        app
    }

    #[test]
    fn apply_type_suppressed_while_confirm_state_present() {
        let mut app = build_confirm_gated_app();
        let term = app.world_mut().spawn(OrzmaTerminal).id();
        app.world_mut()
            .insert_resource(ConfirmState::kill_pane(term));
        app.world_mut().write_message(TypeMessage {
            logical: Key::Character("y".into()),
            focused: Some(term),
            mods: Modifiers::default(),
        });
        app.update();
        assert!(
            app.world().resource::<Captured>().keys.is_empty(),
            "apply_type must be suppressed while a ConfirmState prompt is open, so an \
             answering y/n never reaches the terminal's PTY"
        );
    }

    #[test]
    fn apply_type_suppressed_while_rename_state_present() {
        let mut app = build_confirm_gated_app();
        let term = app.world_mut().spawn(OrzmaTerminal).id();
        app.world_mut()
            .insert_resource(RenameState::new("build".to_string()));
        app.world_mut().write_message(TypeMessage {
            logical: Key::Character("x".into()),
            focused: Some(term),
            mods: Modifiers::default(),
        });
        app.update();
        assert!(
            app.world().resource::<Captured>().keys.is_empty(),
            "apply_type must be suppressed while a RenameState prompt is open, so a \
             character typed into the rename prompt never reaches the terminal's PTY"
        );
    }

    #[test]
    fn apply_type_runs_normally_without_confirm_state() {
        let mut app = build_confirm_gated_app();
        let term = app.world_mut().spawn(OrzmaTerminal).id();
        app.world_mut().write_message(TypeMessage {
            logical: Key::Character("y".into()),
            focused: Some(term),
            mods: Modifiers::default(),
        });
        app.update();
        assert_eq!(
            app.world().resource::<Captured>().keys,
            vec![TerminalKey::Text("y".into())],
            "apply_type must run normally when no ConfirmState is present"
        );
    }

    /// Reproduces the cross-frame key-leak race: a confirming key typed while
    /// `ConfirmState` is present must be drained on its own frame, not merely
    /// suppressed, so it cannot resurface once `ConfirmState` is removed and a
    /// later key is typed. A `run_if`-only gate fails this test because the
    /// suppressed frame never advances `apply_type`'s `MessageReader` cursor,
    /// leaving the confirming key buffered for the next ungated frame.
    #[test]
    fn confirm_key_does_not_leak_into_pty_on_next_frame() {
        let mut app = build_confirm_gated_app();
        let term = app.world_mut().spawn(OrzmaTerminal).id();
        app.world_mut()
            .insert_resource(ConfirmState::kill_pane(term));
        app.world_mut().write_message(TypeMessage {
            logical: Key::Character("y".into()),
            focused: Some(term),
            mods: Modifiers::default(),
        });
        app.update();
        assert!(
            app.world().resource::<Captured>().keys.is_empty(),
            "the confirming key must be drained on the confirming frame, not deferred"
        );

        app.world_mut().remove_resource::<ConfirmState>();
        app.world_mut().write_message(TypeMessage {
            logical: Key::Character("x".into()),
            focused: Some(term),
            mods: Modifiers::default(),
        });
        app.update();
        assert_eq!(
            app.world().resource::<Captured>().keys,
            vec![TerminalKey::Text("x".into())],
            "only the new key must reach the PTY; the drained confirming y must not leak in"
        );
    }
}
