//! Records the un-shifted character winit reports for each physical key.

use bevy::input::ButtonState;
use bevy::input::keyboard::KeyboardInput;
use bevy::prelude::*;
use bevy::winit::RawWinitWindowEvent;
use bevy::winit::converters::{convert_element_state, convert_physical_key_code};
use std::collections::HashMap;
use winit::event::WindowEvent;
use winit::keyboard::Key as WinitKey;
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;

/// The un-shifted character of each physical key pressed so far.
#[derive(Resource, Debug, Default)]
pub struct UnshiftedKeys(HashMap<KeyCode, char>);

impl UnshiftedKeys {
    /// Returns the un-shifted character last recorded for `key_code`, or
    /// `None` when the key has produced no single character.
    pub fn get(&self, key_code: KeyCode) -> Option<char> {
        self.0.get(&key_code).copied()
    }

    /// Records `unshifted` for `key_code`; `None` removes the earlier entry.
    pub fn record(&mut self, key_code: KeyCode, unshifted: Option<char>) {
        match unshifted {
            Some(c) => {
                self.0.insert(key_code, c);
            }
            None => {
                self.0.remove(&key_code);
            }
        }
    }
}

/// Records each pressed key's un-shifted character as winit reports it.
pub(super) struct UnshiftedKeysPlugin;

impl Plugin for UnshiftedKeysPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<UnshiftedKeys>()
            .add_message::<RawWinitWindowEvent>()
            // NOTE: The gate relies on bevy_winit writing a `KeyboardInput`
            // for every non-synthetic raw key event in the same update; a
            // frame with a real press always carries one, so none is skipped.
            .add_systems(
                PreUpdate,
                record_unshifted_keys.run_if(on_message::<KeyboardInput>),
            );
    }
}

fn record_unshifted_keys(
    mut keys: ResMut<UnshiftedKeys>,
    mut raw_events: MessageReader<RawWinitWindowEvent>,
) {
    for raw in raw_events.read() {
        let Some((key_code, unshifted)) = unshifted_press(&raw.event) else {
            continue;
        };
        if keys.get(key_code) != unshifted {
            keys.record(key_code, unshifted);
        }
    }
}

fn unshifted_press(raw: &WindowEvent) -> Option<(KeyCode, Option<char>)> {
    match raw {
        WindowEvent::KeyboardInput {
            event,
            is_synthetic,
            ..
        } if is_real_press(*is_synthetic, convert_element_state(event.state)) => Some((
            convert_physical_key_code(event.physical_key),
            single_char(&event.key_without_modifiers()),
        )),
        _ => None,
    }
}

fn is_real_press(is_synthetic: bool, state: ButtonState) -> bool {
    !is_synthetic && state == ButtonState::Pressed
}

fn single_char(key: &WinitKey) -> Option<char> {
    let WinitKey::Character(text) = key else {
        return None;
    };
    let mut chars = text.chars();
    let first = chars.next()?;
    chars.next().is_none().then_some(first)
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::keyboard::NamedKey;

    /// Asserts that only a one-character `Key::Character` yields an
    /// un-shifted character.
    ///
    /// Case: winit reports a digit key, a multi-character dead-key
    /// composition, and the Enter key.
    #[test]
    fn only_a_one_character_key_has_an_unshifted_character() {
        assert_eq!(single_char(&WinitKey::Character("1".into())), Some('1'));
        assert_eq!(single_char(&WinitKey::Character("ab".into())), None);
        assert_eq!(single_char(&WinitKey::Named(NamedKey::Enter)), None);
    }

    /// Asserts that a recorded character is returned for its key only.
    ///
    /// Case: the user presses the 1 key on a layout where it types a digit.
    #[test]
    fn a_press_records_its_unshifted_character() {
        let mut keys = UnshiftedKeys::default();
        keys.record(KeyCode::Digit1, Some('1'));
        assert_eq!(keys.get(KeyCode::Digit1), Some('1'));
        assert_eq!(keys.get(KeyCode::KeyA), None);
    }

    /// Asserts that recording `None` forgets the key's earlier entry.
    ///
    /// Case: the user switches layout so the A key no longer types a
    /// single character.
    #[test]
    fn a_press_without_a_character_forgets_the_earlier_entry() {
        let mut keys = UnshiftedKeys::default();
        keys.record(KeyCode::KeyA, Some('a'));
        keys.record(KeyCode::KeyA, None);
        assert_eq!(keys.get(KeyCode::KeyA), None);
    }

    /// Asserts that only a non-synthetic press records an un-shifted
    /// character, not a synthetic press or a release.
    ///
    /// Case: the window regains focus and winit replays a synthetic press for
    /// a held key, and the user later releases a key.
    #[test]
    fn only_a_real_press_records_an_unshifted_character() {
        assert!(is_real_press(false, ButtonState::Pressed));
        assert!(!is_real_press(true, ButtonState::Pressed));
        assert!(!is_real_press(false, ButtonState::Released));
    }

    /// Asserts that a raw winit event other than a key press records
    /// nothing.
    ///
    /// Case: the window gains focus.
    #[test]
    fn a_raw_event_other_than_a_key_press_records_nothing() {
        assert_eq!(unshifted_press(&WindowEvent::Focused(true)), None);
    }
}
