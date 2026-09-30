//! Releases the modifier keys macOS reports up while `ButtonInput<KeyCode>`
//! still holds them, such as after the screenshot toolbar takes their release.

use bevy::input::InputSystems;
use bevy::prelude::*;
use bevy::winit::RawWinitWindowEvent;
use winit::event::WindowEvent;
use winit::keyboard::ModifiersState;

/// Keeps the held modifier keys in step with the modifier state macOS
/// reports.
pub(super) struct ModifierSyncPlugin;

impl Plugin for ModifierSyncPlugin {
    fn build(&self, app: &mut App) {
        if cfg!(target_os = "macos") {
            app.add_message::<RawWinitWindowEvent>().add_systems(
                PreUpdate,
                release_stale_modifiers
                    .after(InputSystems)
                    .run_if(on_message::<RawWinitWindowEvent>),
            );
        }
    }
}

// TODO: remove this module once Bevy includes bevyengine/bevy#24845 (0.20),
// which reconciles the held modifier keys with winit's modifier state.
fn release_stale_modifiers(
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut raw_events: MessageReader<RawWinitWindowEvent>,
) {
    let Some(reported) = reported_modifiers(raw_events.read().map(|raw| &raw.event)) else {
        return;
    };
    let stale = stale_modifier_keys(reported, &keys);
    if stale.is_empty() {
        return;
    }
    debug!(?stale, "releasing the modifier keys macOS reports up");
    for key in stale {
        keys.release(key);
    }
}

/// Returns the modifier state of the last `ModifiersChanged` that follows
/// every `Focused(false)` in `events`, or `None` when there is none.
fn reported_modifiers<'a>(
    events: impl IntoIterator<Item = &'a WindowEvent>,
) -> Option<ModifiersState> {
    events
        .into_iter()
        .fold(None, |reported, event| match event {
            WindowEvent::Focused(false) => None,
            WindowEvent::ModifiersChanged(modifiers) => Some(modifiers.state()),
            _ => reported,
        })
}

/// Returns the held modifier keys, both sides, whose modifier `reported`
/// leaves out.
fn stale_modifier_keys(reported: ModifiersState, held: &ButtonInput<KeyCode>) -> Vec<KeyCode> {
    /// Each modifier key with the modifier it presses.
    const MODIFIER_KEYS: [(ModifiersState, KeyCode); 8] = [
        (ModifiersState::SHIFT, KeyCode::ShiftLeft),
        (ModifiersState::SHIFT, KeyCode::ShiftRight),
        (ModifiersState::CONTROL, KeyCode::ControlLeft),
        (ModifiersState::CONTROL, KeyCode::ControlRight),
        (ModifiersState::ALT, KeyCode::AltLeft),
        (ModifiersState::ALT, KeyCode::AltRight),
        (ModifiersState::SUPER, KeyCode::SuperLeft),
        (ModifiersState::SUPER, KeyCode::SuperRight),
    ];

    MODIFIER_KEYS
        .iter()
        .filter(|(modifier, key)| !reported.contains(*modifier) && held.pressed(*key))
        .map(|(_, key)| *key)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modifiers_changed(state: ModifiersState) -> WindowEvent {
        WindowEvent::ModifiersChanged(state.into())
    }

    fn held(keys: &[KeyCode]) -> ButtonInput<KeyCode> {
        let mut input = ButtonInput::default();
        for key in keys {
            input.press(*key);
        }
        input
    }

    /// Asserts that an empty report makes both held Cmd and Shift stale.
    ///
    /// Case: the user presses Cmd+Shift+5 and releases both keys while the
    /// screenshot toolbar holds the keyboard.
    #[test]
    fn an_empty_report_makes_held_cmd_and_shift_stale() {
        let stale = stale_modifier_keys(
            ModifiersState::empty(),
            &held(&[KeyCode::SuperLeft, KeyCode::ShiftLeft]),
        );
        assert_eq!(stale, vec![KeyCode::ShiftLeft, KeyCode::SuperLeft]);
    }

    /// Asserts that a modifier the report still carries stays held while
    /// the one it leaves out becomes stale.
    ///
    /// Case: the user still holds Cmd when the event that reports Shift up
    /// arrives.
    #[test]
    fn a_reported_modifier_stays_held() {
        let stale = stale_modifier_keys(
            ModifiersState::SUPER,
            &held(&[KeyCode::SuperLeft, KeyCode::ShiftLeft]),
        );
        assert_eq!(stale, vec![KeyCode::ShiftLeft]);
    }

    /// Asserts that both sides of a modifier the report leaves out become
    /// stale.
    ///
    /// Case: the user held left and right Cmd together when the toolbar
    /// took their release.
    #[test]
    fn both_sides_of_a_released_modifier_become_stale() {
        let stale = stale_modifier_keys(
            ModifiersState::empty(),
            &held(&[KeyCode::SuperLeft, KeyCode::SuperRight]),
        );
        assert_eq!(stale, vec![KeyCode::SuperLeft, KeyCode::SuperRight]);
    }

    /// Asserts that nothing is stale while no modifier key is held.
    ///
    /// Case: the user moves the pointer over the window with no key held.
    #[test]
    fn nothing_is_stale_without_a_held_modifier() {
        let stale = stale_modifier_keys(ModifiersState::empty(), &held(&[KeyCode::KeyA]));
        assert_eq!(stale, Vec::<KeyCode>::new());
    }

    /// Asserts that Control and Alt become stale like Shift and Cmd.
    ///
    /// Case: the user held left Control and right Option when the toolbar
    /// took their release, and still holds Shift and Cmd.
    #[test]
    fn control_and_alt_become_stale() {
        let stale = stale_modifier_keys(
            ModifiersState::SHIFT | ModifiersState::SUPER,
            &held(&[
                KeyCode::ControlLeft,
                KeyCode::AltRight,
                KeyCode::ShiftLeft,
                KeyCode::SuperLeft,
            ]),
        );
        assert_eq!(stale, vec![KeyCode::ControlLeft, KeyCode::AltRight]);
    }

    /// Asserts that a report followed by a focus loss is ignored, even when
    /// the window regains focus in the same frame.
    ///
    /// Case: the user holds Cmd while the window resigns key and becomes
    /// key again before the next update.
    #[test]
    fn a_report_before_a_focus_loss_is_ignored() {
        let events = [
            modifiers_changed(ModifiersState::empty()),
            WindowEvent::Focused(false),
            WindowEvent::Focused(true),
        ];
        assert_eq!(reported_modifiers(&events), None);
    }

    /// Asserts that a report after the focus returns is used.
    ///
    /// Case: the window regains focus and the next key the user types
    /// carries modifier flags that differ from winit's record.
    #[test]
    fn a_report_after_the_focus_returns_is_used() {
        let events = [
            WindowEvent::Focused(false),
            WindowEvent::Focused(true),
            modifiers_changed(ModifiersState::empty()),
        ];
        assert_eq!(reported_modifiers(&events), Some(ModifiersState::empty()));
    }

    /// Asserts that the last of several reports is used.
    ///
    /// Case: the user presses and releases Cmd within one frame.
    #[test]
    fn the_last_report_is_used() {
        let events = [
            modifiers_changed(ModifiersState::SUPER),
            modifiers_changed(ModifiersState::empty()),
        ];
        assert_eq!(reported_modifiers(&events), Some(ModifiersState::empty()));
    }

    /// Asserts that a frame with no report yields none.
    ///
    /// Case: the user resizes the window without touching a key.
    #[test]
    fn a_frame_without_a_report_yields_none() {
        let events = [WindowEvent::Focused(true)];
        assert_eq!(reported_modifiers(&events), None);
    }

    #[cfg(target_os = "macos")]
    mod app {
        use super::*;
        use crate::input::current_modifiers;
        use bevy::input::ButtonState;
        use bevy::input::InputPlugin;
        use bevy::input::keyboard::{Key, KeyboardInput};
        use orzma_configs::shortcuts::Modifiers;
        use winit::window::WindowId;

        /// The modifiers `current_modifiers` returned in `Update` for each
        /// `KeyA` press.
        #[derive(Resource, Default)]
        struct Observed(Vec<Modifiers>);

        fn observe_key_a(
            mut observed: ResMut<Observed>,
            mut presses: MessageReader<KeyboardInput>,
            keys: Res<ButtonInput<KeyCode>>,
        ) {
            for press in presses.read() {
                if press.key_code == KeyCode::KeyA {
                    observed.0.push(current_modifiers(&keys));
                }
            }
        }

        fn sync_app() -> App {
            let mut app = App::new();
            app.add_plugins((MinimalPlugins, InputPlugin, ModifierSyncPlugin))
                .init_resource::<Observed>()
                .add_systems(Update, observe_key_a);
            app
        }

        fn press(app: &mut App, key_code: KeyCode, logical_key: Key) {
            app.world_mut().write_message(KeyboardInput {
                key_code,
                logical_key,
                state: ButtonState::Pressed,
                text: None,
                repeat: false,
                window: Entity::PLACEHOLDER,
            });
        }

        fn raw(app: &mut App, event: WindowEvent) {
            app.world_mut().write_message(RawWinitWindowEvent {
                window_id: WindowId::dummy(),
                event,
            });
        }

        fn hold_cmd_and_shift(app: &mut App) {
            press(app, KeyCode::SuperLeft, Key::Super);
            press(app, KeyCode::ShiftLeft, Key::Shift);
            app.update();
        }

        /// Asserts that a key typed in the frame that reports Cmd and Shift
        /// up already reads no modifier in `Update`.
        ///
        /// Case: the user starts a recording from the Cmd+Shift+5 toolbar and
        /// types `a` into orzma.
        #[test]
        fn a_key_after_an_empty_report_reads_no_modifier() {
            let mut app = sync_app();
            hold_cmd_and_shift(&mut app);
            raw(&mut app, modifiers_changed(ModifiersState::empty()));
            press(&mut app, KeyCode::KeyA, Key::Character("a".into()));
            app.update();
            assert_eq!(
                app.world().resource::<Observed>().0,
                vec![Modifiers::default()]
            );
        }

        /// Asserts that a held Cmd stays pressed when an empty report precedes
        /// a focus loss and return in the same frame.
        ///
        /// Case: the user holds Cmd while the window resigns key and becomes
        /// key again before the next update.
        #[test]
        fn a_focus_round_trip_keeps_a_held_cmd() {
            let mut app = sync_app();
            hold_cmd_and_shift(&mut app);
            raw(&mut app, modifiers_changed(ModifiersState::empty()));
            raw(&mut app, WindowEvent::Focused(false));
            raw(&mut app, WindowEvent::Focused(true));
            app.update();
            assert!(
                app.world()
                    .resource::<ButtonInput<KeyCode>>()
                    .pressed(KeyCode::SuperLeft)
            );
        }

        /// Asserts that a report matching the held modifiers leaves
        /// `ButtonInput<KeyCode>` unwritten.
        ///
        /// Case: the window resigns key and becomes key again while the user
        /// holds Cmd and Shift, and the pointer then moves, so winit reports
        /// both modifiers again.
        #[test]
        fn a_matching_report_leaves_the_keys_unwritten() {
            let mut app = sync_app();
            hold_cmd_and_shift(&mut app);
            let before = app
                .world()
                .resource_ref::<ButtonInput<KeyCode>>()
                .last_changed();
            raw(
                &mut app,
                modifiers_changed(ModifiersState::SHIFT | ModifiersState::SUPER),
            );
            app.update();
            assert_eq!(
                app.world()
                    .resource_ref::<ButtonInput<KeyCode>>()
                    .last_changed(),
                before
            );
        }
    }
}
