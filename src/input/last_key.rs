//! Records when the user last pressed a key, which the caret's blink
//! phase counts from.

use crate::input::InputPhase;
use bevy::input::ButtonState;
use bevy::input::keyboard::KeyboardInput;
use bevy::prelude::*;
use bevy_orzma_tty_renderer::prelude::LastKeyInstant;

/// Keeps [`LastKeyInstant`] in step with the user's key presses.
pub(super) struct LastKeyPlugin;

impl Plugin for LastKeyPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            record_last_key
                .in_set(InputPhase::FocusedKey)
                .run_if(on_message::<KeyboardInput>),
        );
    }
}

fn record_last_key(
    mut last_key: ResMut<LastKeyInstant>,
    mut presses: MessageReader<KeyboardInput>,
    time: Res<Time<Real>>,
) {
    if presses
        .read()
        .filter(|input| input.state == ButtonState::Pressed)
        .count()
        > 0
    {
        last_key.0 = time.elapsed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::input::keyboard::Key;
    use bevy::time::TimeUpdateStrategy;
    use std::time::Duration;

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
                10,
            )))
            .init_resource::<LastKeyInstant>()
            .add_message::<KeyboardInput>()
            .add_plugins(LastKeyPlugin);
        app
    }

    fn key(state: ButtonState) -> KeyboardInput {
        KeyboardInput {
            key_code: KeyCode::KeyA,
            logical_key: Key::Character("a".into()),
            state,
            text: None,
            repeat: false,
            window: Entity::PLACEHOLDER,
        }
    }

    /// Asserts that a key press advances the recorded instant past zero
    /// while a release leaves it alone.
    ///
    /// Case: the user types a character and lifts the key.
    #[test]
    fn a_press_advances_the_instant_and_a_release_does_not() {
        let mut app = app();
        app.update();
        app.world_mut().write_message(key(ButtonState::Pressed));
        app.update();
        let after_press = app.world().resource::<LastKeyInstant>().0;
        assert!(after_press > Duration::ZERO);

        app.world_mut().write_message(key(ButtonState::Released));
        app.update();
        assert_eq!(app.world().resource::<LastKeyInstant>().0, after_press);
    }
}
