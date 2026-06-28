//! Host keyboard input dispatch: routes `KeyboardInput` messages to the single
//! `KeyboardFocused` terminal; also provides the modifier-reading helper used
//! by the mouse dispatch.

use crate::input::bindings::{ReservedChord, TerminalInputBindings};
use crate::input::focus::{KeyboardDisabled, KeyboardFocused};
use bevy::ecs::message::MessageReader;
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;
use ozma_terminal::{OzmaTerminal, OzmaTerminalInputSet, PasteAction};
use ozma_tty_engine::{TerminalKey, TerminalKeyInput, TerminalModifiers};

/// Registers `TerminalInputBindings` and the default keyboard dispatcher.
pub(crate) struct KeyboardInputPlugin;

impl Plugin for KeyboardInputPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TerminalInputBindings>()
            .add_message::<KeyboardInput>()
            .add_systems(
                Update,
                dispatch_input
                    .in_set(OzmaTerminalInputSet)
                    .run_if(on_message::<KeyboardInput>),
            );
    }
}

/// Returns the terminal modifier state from the `ButtonInput<KeyCode>` resource.
pub(crate) fn current_terminal_modifiers(keys: &ButtonInput<KeyCode>) -> TerminalModifiers {
    TerminalModifiers {
        ctrl: keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight),
        shift: keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight),
        alt: keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight),
        meta: keys.pressed(KeyCode::SuperLeft) || keys.pressed(KeyCode::SuperRight),
    }
}

fn dispatch_input(
    mut commands: Commands,
    mut events: MessageReader<KeyboardInput>,
    bindings: Res<TerminalInputBindings>,
    keys: Res<ButtonInput<KeyCode>>,
    terminal: Query<
        Entity,
        (
            With<OzmaTerminal>,
            With<KeyboardFocused>,
            Without<KeyboardDisabled>,
        ),
    >,
) {
    // NOTE: keyboard routes to the single `KeyboardFocused` terminal. The host
    // owns focus policy and MUST keep exactly one OzmaTerminal both
    // `KeyboardFocused` and not `KeyboardDisabled`, or this `.single()` returns
    // Err and every keypress is silently dropped.
    let Ok(entity) = terminal.single() else {
        events.clear();
        return;
    };
    let mods = current_terminal_modifiers(&keys);
    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        if bindings
            .reserved
            .iter()
            .any(|c| chord_matches(c, ev.key_code, &mods))
        {
            continue;
        }
        if chord_matches(&bindings.paste, ev.key_code, &mods) {
            commands.trigger(PasteAction { entity });
            continue;
        }
        if mods.meta {
            continue;
        }
        if let Some(key) = bevy_key_to_terminal_key(&ev.logical_key) {
            commands.trigger(TerminalKeyInput {
                entity,
                key,
                modifiers: mods,
            });
        }
    }
}

fn chord_matches(chord: &ReservedChord, key_code: KeyCode, mods: &TerminalModifiers) -> bool {
    chord.key_code == key_code
        && chord.ctrl == mods.ctrl
        && chord.shift == mods.shift
        && chord.alt == mods.alt
        && chord.meta == mods.meta
}

fn bevy_key_to_terminal_key(logical_key: &Key) -> Option<TerminalKey> {
    match logical_key {
        Key::Character(s) => Some(TerminalKey::Text(s.to_string())),
        Key::Space => Some(TerminalKey::Text(" ".to_string())),
        Key::Enter => Some(TerminalKey::Enter),
        Key::Backspace => Some(TerminalKey::Backspace),
        Key::Tab => Some(TerminalKey::Tab),
        Key::Escape => Some(TerminalKey::Escape),
        Key::Delete => Some(TerminalKey::Delete),
        Key::ArrowUp => Some(TerminalKey::ArrowUp),
        Key::ArrowDown => Some(TerminalKey::ArrowDown),
        Key::ArrowLeft => Some(TerminalKey::ArrowLeft),
        Key::ArrowRight => Some(TerminalKey::ArrowRight),
        Key::Home => Some(TerminalKey::Home),
        Key::End => Some(TerminalKey::End),
        Key::PageUp => Some(TerminalKey::PageUp),
        Key::PageDown => Some(TerminalKey::PageDown),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Resource, Default)]
    struct Captured {
        paste: u32,
        keys: Vec<TerminalKey>,
        entities: Vec<Entity>,
    }

    fn test_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<Captured>()
            .add_plugins(KeyboardInputPlugin)
            .add_observer(|ev: On<PasteAction>, mut c: ResMut<Captured>| {
                let _ = ev;
                c.paste += 1;
            })
            .add_observer(|ev: On<TerminalKeyInput>, mut c: ResMut<Captured>| {
                c.keys.push(ev.key.clone());
                c.entities.push(ev.entity);
            });
        app
    }

    fn press(app: &mut App, key_code: KeyCode, logical: Key) {
        app.world_mut().write_message(KeyboardInput {
            key_code,
            logical_key: logical,
            state: ButtonState::Pressed,
            text: None,
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
    }

    fn hold_meta(app: &mut App) {
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::SuperLeft);
    }

    #[test]
    fn plain_key_forwards_as_terminal_key() {
        let mut app = test_app();
        app.world_mut().spawn((OzmaTerminal, KeyboardFocused));
        press(&mut app, KeyCode::KeyA, Key::Character("a".into()));
        app.update();
        let c = app.world().resource::<Captured>();
        assert_eq!(c.keys, vec![TerminalKey::Text("a".into())]);
        assert_eq!(c.paste, 0);
    }

    #[test]
    fn paste_chord_fires_paste_action() {
        let mut app = test_app();
        app.world_mut().spawn((OzmaTerminal, KeyboardFocused));
        hold_meta(&mut app);
        press(&mut app, KeyCode::KeyV, Key::Character("v".into()));
        app.update();
        let c = app.world().resource::<Captured>();
        assert_eq!(c.paste, 1);
        assert!(c.keys.is_empty());
    }

    #[test]
    fn reserved_chord_is_skipped() {
        let mut app = test_app();
        app.world_mut().spawn((OzmaTerminal, KeyboardFocused));
        app.world_mut()
            .resource_mut::<TerminalInputBindings>()
            .reserved = vec![ReservedChord {
            key_code: KeyCode::KeyQ,
            ctrl: false,
            shift: false,
            alt: false,
            meta: true,
        }];
        hold_meta(&mut app);
        press(&mut app, KeyCode::KeyQ, Key::Character("q".into()));
        app.update();
        let c = app.world().resource::<Captured>();
        assert_eq!(c.paste, 0);
        assert!(c.keys.is_empty());
    }

    #[test]
    fn unhandled_meta_chord_is_dropped() {
        let mut app = test_app();
        app.world_mut().spawn((OzmaTerminal, KeyboardFocused));
        hold_meta(&mut app);
        press(&mut app, KeyCode::KeyJ, Key::Character("j".into()));
        app.update();
        let c = app.world().resource::<Captured>();
        assert_eq!(c.paste, 0);
        assert!(c.keys.is_empty(), "Cmd+J must not reach the PTY");
    }

    #[test]
    fn keyboard_disabled_overrides_focus() {
        let mut app = test_app();
        app.world_mut()
            .spawn((OzmaTerminal, KeyboardFocused, KeyboardDisabled));
        press(&mut app, KeyCode::KeyA, Key::Character("a".into()));
        app.update();
        let c = app.world().resource::<Captured>();
        assert!(c.keys.is_empty());
        assert_eq!(c.paste, 0);
    }

    #[test]
    fn routes_to_keyboard_focused_terminal() {
        let mut app = test_app();
        app.world_mut().spawn(OzmaTerminal);
        let focused = app.world_mut().spawn((OzmaTerminal, KeyboardFocused)).id();
        press(&mut app, KeyCode::KeyA, Key::Character("a".into()));
        app.update();
        let c = app.world().resource::<Captured>();
        assert_eq!(c.keys, vec![TerminalKey::Text("a".into())]);
        assert_eq!(c.entities, vec![focused]);
    }

    #[test]
    fn no_focused_terminal_drops_keys() {
        let mut app = test_app();
        app.world_mut().spawn(OzmaTerminal);
        press(&mut app, KeyCode::KeyA, Key::Character("a".into()));
        app.update();
        assert!(app.world().resource::<Captured>().keys.is_empty());
    }

    #[test]
    fn two_focused_terminals_drop_keys() {
        let mut app = test_app();
        app.world_mut().spawn((OzmaTerminal, KeyboardFocused));
        app.world_mut().spawn((OzmaTerminal, KeyboardFocused));
        press(&mut app, KeyCode::KeyA, Key::Character("a".into()));
        app.update();
        assert!(app.world().resource::<Captured>().keys.is_empty());
    }

    #[test]
    fn printable_char_maps_to_text() {
        assert_eq!(
            bevy_key_to_terminal_key(&Key::Character("a".into())),
            Some(TerminalKey::Text("a".to_string()))
        );
        assert_eq!(
            bevy_key_to_terminal_key(&Key::Character("あ".into())),
            Some(TerminalKey::Text("あ".to_string()))
        );
    }

    #[test]
    fn space_maps_to_text() {
        assert_eq!(
            bevy_key_to_terminal_key(&Key::Space),
            Some(TerminalKey::Text(" ".to_string()))
        );
    }

    #[test]
    fn control_keys_map_correctly() {
        assert_eq!(
            bevy_key_to_terminal_key(&Key::Enter),
            Some(TerminalKey::Enter)
        );
        assert_eq!(
            bevy_key_to_terminal_key(&Key::Backspace),
            Some(TerminalKey::Backspace)
        );
        assert_eq!(bevy_key_to_terminal_key(&Key::Tab), Some(TerminalKey::Tab));
        assert_eq!(
            bevy_key_to_terminal_key(&Key::Escape),
            Some(TerminalKey::Escape)
        );
        assert_eq!(
            bevy_key_to_terminal_key(&Key::Delete),
            Some(TerminalKey::Delete)
        );
    }

    #[test]
    fn navigation_keys_map_correctly() {
        assert_eq!(
            bevy_key_to_terminal_key(&Key::ArrowUp),
            Some(TerminalKey::ArrowUp)
        );
        assert_eq!(
            bevy_key_to_terminal_key(&Key::ArrowDown),
            Some(TerminalKey::ArrowDown)
        );
        assert_eq!(
            bevy_key_to_terminal_key(&Key::ArrowLeft),
            Some(TerminalKey::ArrowLeft)
        );
        assert_eq!(
            bevy_key_to_terminal_key(&Key::ArrowRight),
            Some(TerminalKey::ArrowRight)
        );
        assert_eq!(
            bevy_key_to_terminal_key(&Key::Home),
            Some(TerminalKey::Home)
        );
        assert_eq!(bevy_key_to_terminal_key(&Key::End), Some(TerminalKey::End));
        assert_eq!(
            bevy_key_to_terminal_key(&Key::PageUp),
            Some(TerminalKey::PageUp)
        );
        assert_eq!(
            bevy_key_to_terminal_key(&Key::PageDown),
            Some(TerminalKey::PageDown)
        );
    }

    #[test]
    fn modifier_and_unrecognized_keys_return_none() {
        assert_eq!(bevy_key_to_terminal_key(&Key::Shift), None);
        assert_eq!(bevy_key_to_terminal_key(&Key::Control), None);
        assert_eq!(bevy_key_to_terminal_key(&Key::Alt), None);
        assert_eq!(bevy_key_to_terminal_key(&Key::Super), None);
        assert_eq!(bevy_key_to_terminal_key(&Key::F1), None);
        assert_eq!(bevy_key_to_terminal_key(&Key::Insert), None);
    }
}
