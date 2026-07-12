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
/// `resolve_key_effects`; pane/window actions are no-ops until the built-in
/// multiplexer lands.
/// Registered in `ShortcutSet::Apply`, gated on `on_message::<ShortcutMessage>`.
pub(in crate::input) fn apply_shortcuts(
    mut commands: Commands,
    mut shortcuts: MessageReader<ShortcutMessage>,
) {
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
pub(in crate::input) fn apply_vi_mode(
    mut commands: Commands,
    mut vi_mode: MessageReader<ViModeMessage>,
) {
    for msg in vi_mode.read() {
        if let Some(entity) = msg.focused {
            trigger_vi_mode_action(&mut commands, entity, msg.action);
        }
    }
}

/// Types raw keys from `TypeMessage` into the focused terminal as
/// `TerminalKeyInput`. Runs after the shortcut/copy appliers. Registered in
/// `ShortcutSet::Apply`, gated on `on_message::<TypeMessage>`.
pub(in crate::input) fn apply_type(
    mut commands: Commands,
    mut type_keys: MessageReader<TypeMessage>,
) {
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

    /// Builds an app running the three Default appliers as bare
    /// per-message consumers, capturing the events they trigger.
    fn build_default_dispatch_app(shortcuts: Shortcuts) -> App {
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

    fn default_dispatch_app(shortcuts: Shortcuts) -> (App, Entity) {
        let mut app = build_default_dispatch_app(shortcuts);
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
        let (mut app, term) = default_dispatch_app(Shortcuts::default());
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
        let (mut app, term) = default_dispatch_app(Shortcuts::default());
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
            "a Default-mode pane action resolves to a no-op: no event, no typing"
        );
    }

    #[test]
    fn direct_paste_outside_vi_mode_pastes() {
        let (mut app, term) = default_dispatch_app(Shortcuts::default());
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
        let (mut app, term) = default_dispatch_app(Shortcuts::default());
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
        let (mut app, term) = default_dispatch_app(Shortcuts::default());
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
        let (mut app, term) = default_dispatch_app(Shortcuts::default());
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
        let (mut app, term) = default_dispatch_app(Shortcuts::default());
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
        let (mut app, term) = default_dispatch_app(Shortcuts::default());
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
            "EnterViMode must fire unconditionally in Default mode, even when vi mode is \
             already active"
        );
    }
}
