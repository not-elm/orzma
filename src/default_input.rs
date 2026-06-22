//! Host-side input for `AppMode::Default`: maintains the crate's `KeyboardDisabled` / `MouseDisabled`
//! markers from the coarse guards (picker, IME, focus, webview), and handles the
//! application-level GUI shortcuts the terminal crate does not own (Quit,
//! OpenPicker, DetachSession, ReleaseWebviewFocus). Raw-key forwarding and paste
//! are owned by `ozma_terminal`'s dispatcher and `PasteAction`.

use crate::app_mode::AppMode;
use crate::input::ime::{ImeCommit, ImeState};
use crate::input::shortcuts::ResolvedShortcuts;
use crate::input::{InputPhase, current_modifiers};
use crate::picker::SessionPicker;
use bevy::input::ButtonState;
use bevy::input::keyboard::KeyboardInput;
use bevy::prelude::*;
use bevy::window::{PrimaryWindow, Window};
use bevy_cef::prelude::FocusedWebview;
use ozma_terminal::{
    KeyboardDisabled, MouseDisabled, OzmaTerminal, OzmaTerminalInputSet, OzmaTerminalMouseSet,
};
use ozmux_configs::shortcuts::ShortcutAction;
use ozma_tty_engine::{TerminalKey, TerminalKeyInput, TerminalModifiers};
use ozmux_tmux::TmuxPane;

/// Registers the host-side input systems for `AppMode::Default`.
pub(crate) struct DefaultHostInputPlugin;

impl Plugin for DefaultHostInputPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            maintain_input_gates
                .before(OzmaTerminalInputSet)
                .before(OzmaTerminalMouseSet)
                .run_if(in_state(AppMode::Default)),
        )
        .add_systems(
            Update,
            app_shortcut_handler
                .in_set(InputPhase::FocusedKey)
                .run_if(in_state(AppMode::Default))
                .run_if(on_message::<KeyboardInput>),
        )
        .add_observer(apply_ime_commit_to_terminal);
    }
}

fn maintain_input_gates(
    mut commands: Commands,
    picker: Res<SessionPicker>,
    ime: Res<ImeState>,
    focused_webview: Res<FocusedWebview>,
    windows: Query<&Window, With<PrimaryWindow>>,
    terminals: Query<(Entity, Has<KeyboardDisabled>, Has<MouseDisabled>), With<OzmaTerminal>>,
) {
    let focused = windows.single().map(|w| w.focused).unwrap_or(false);
    let disable = should_disable_input(
        picker.open,
        ime.is_composing(),
        focused,
        focused_webview.0.is_some(),
    );
    for (entity, has_keyboard, has_mouse) in terminals.iter() {
        if disable && !has_keyboard {
            commands.entity(entity).insert(KeyboardDisabled);
        } else if !disable && has_keyboard {
            commands.entity(entity).remove::<KeyboardDisabled>();
        }
        if disable && !has_mouse {
            commands.entity(entity).insert(MouseDisabled);
        } else if !disable && has_mouse {
            commands.entity(entity).remove::<MouseDisabled>();
        }
    }
}

fn app_shortcut_handler(
    mut exit: MessageWriter<AppExit>,
    mut events: MessageReader<KeyboardInput>,
    mut picker: ResMut<SessionPicker>,
    mut focused_webview: ResMut<FocusedWebview>,
    shortcuts: Res<ResolvedShortcuts>,
    ime: Res<ImeState>,
    bevy_keys: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let focused = windows.single().map(|w| w.focused).unwrap_or(false);
    if picker.open || ime.is_composing() || !focused {
        events.clear();
        return;
    }
    let mods = current_modifiers(&bevy_keys);
    let webview_focused = focused_webview.0.is_some();
    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        if webview_focused && shortcuts.is_release_webview_focus(ev.key_code, mods) {
            focused_webview.0 = None;
            continue;
        }
        let Some(action) = shortcuts.match_gui_action(ev.key_code, mods) else {
            continue;
        };
        if gui_action_suppressed_by_webview(webview_focused, action) {
            continue;
        }
        match action {
            ShortcutAction::Quit => {
                exit.write(AppExit::Success);
            }
            ShortcutAction::OpenPicker => {
                picker.open = true;
            }
            ShortcutAction::DetachSession => {}
            ShortcutAction::Paste | ShortcutAction::ReleaseWebviewFocus => {}
        }
    }
}

fn apply_ime_commit_to_terminal(
    ev: On<ImeCommit>,
    mut commands: Commands,
    terminals: Query<(), (With<OzmaTerminal>, Without<TmuxPane>)>,
) {
    // NOTE: discriminate on TmuxPane absence — tmux panes are also OzmaTerminal
    // entities (src/tmux/render.rs), and their commits go out via the tmux
    // observer in src/tmux/forward.rs. Without this filter the commit would be
    // double-delivered.
    if terminals.get(ev.entity).is_err() {
        return;
    }
    commands.trigger(TerminalKeyInput {
        entity: ev.entity,
        key: TerminalKey::Text(ev.text.clone()),
        modifiers: TerminalModifiers::default(),
    });
}

fn should_disable_input(
    picker_open: bool,
    composing: bool,
    window_focused: bool,
    webview_focused: bool,
) -> bool {
    picker_open || composing || !window_focused || webview_focused
}

fn gui_action_suppressed_by_webview(webview_focused: bool, action: ShortcutAction) -> bool {
    webview_focused && action != ShortcutAction::ReleaseWebviewFocus
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::app::App;
    use bevy::ecs::resource::Resource;
    use bevy::prelude::{Entity, MinimalPlugins, On, ResMut};
    use ozma_tty_engine::TerminalKeyInput;
    use ozmux_tmux::{PaneId, TmuxPane};
    use tmux_control_parser::CellDims;

    #[test]
    fn ime_commit_fires_terminal_key_input_for_plain_terminal() {
        use crate::input::ime::ImeCommit;
        use ozma_terminal::OzmaTerminal;
        use ozma_tty_engine::TerminalKey;

        #[derive(Resource, Default)]
        struct Hits(Vec<(Entity, TerminalKey)>);

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<Hits>()
            .add_observer(apply_ime_commit_to_terminal)
            .add_observer(|ev: On<TerminalKeyInput>, mut h: ResMut<Hits>| {
                h.0.push((ev.entity, ev.key.clone()));
            });

        let term = app.world_mut().spawn(OzmaTerminal).id();
        app.world_mut().trigger(ImeCommit {
            entity: term,
            text: "あ".into(),
        });
        app.update();

        assert_eq!(
            app.world().resource::<Hits>().0,
            vec![(term, TerminalKey::Text("あ".into()))]
        );
    }

    #[test]
    fn ime_commit_is_noop_for_tmux_pane_target() {
        use crate::input::ime::ImeCommit;
        use ozma_terminal::OzmaTerminal;

        #[derive(Resource, Default)]
        struct Hits(u32);

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<Hits>()
            .add_observer(apply_ime_commit_to_terminal)
            .add_observer(|_ev: On<TerminalKeyInput>, mut h: ResMut<Hits>| h.0 += 1);

        let pane = app
            .world_mut()
            .spawn((
                OzmaTerminal,
                TmuxPane {
                    id: PaneId(1),
                    dims: CellDims { width: 0, height: 0, xoff: 0, yoff: 0 },
                },
            ))
            .id();
        app.world_mut().trigger(ImeCommit { entity: pane, text: "x".into() });
        app.update();

        assert_eq!(app.world().resource::<Hits>().0, 0);
    }

    #[test]
    fn disables_input_on_any_guard() {
        assert!(!should_disable_input(false, false, true, false));
        assert!(should_disable_input(true, false, true, false));
        assert!(should_disable_input(false, true, true, false));
        assert!(should_disable_input(false, false, false, false));
        assert!(should_disable_input(false, false, true, true));
    }

    #[test]
    fn webview_focus_suppresses_all_but_release() {
        assert!(gui_action_suppressed_by_webview(true, ShortcutAction::Quit));
        assert!(gui_action_suppressed_by_webview(
            true,
            ShortcutAction::OpenPicker
        ));
        assert!(gui_action_suppressed_by_webview(
            true,
            ShortcutAction::DetachSession
        ));
        assert!(!gui_action_suppressed_by_webview(
            true,
            ShortcutAction::ReleaseWebviewFocus
        ));
        assert!(!gui_action_suppressed_by_webview(
            false,
            ShortcutAction::Quit
        ));
    }
}
