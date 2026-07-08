//! Shared single-line text-input overlay built on Bevy's `EditableText`.
//!
//! Both the tmux rename prompt and the vi-search prompt spawn one of these,
//! set `InputFocus` to the `EditableText` entity, and let `bevy_ui_widgets`'
//! `EditableTextInputPlugin` drive keyboard, IME, clipboard, and pointer
//! editing. This module owns keyboard suppression (via `ActiveTextPrompt`),
//! submit/cancel, and teardown; per-prompt modules observe `TextPromptSubmit`
//! and run the tmux command.

use bevy::input::keyboard::Key;
use bevy::prelude::*;

/// Single "a text prompt is open/focused" signal, read by every input gate to
/// suppress the terminal while a prompt owns the keyboard. Mirrors `InputFocus`
/// but is app-owned; a self-heal system keeps them in sync.
#[derive(Resource, Default)]
pub(crate) struct ActiveTextPrompt(pub(crate) Option<Entity>);

/// Marks the `EditableText` entity of an open prompt. `bar` is the overlay
/// container despawned on close; `submit_on_first_char` is set for vi jump
/// prompts, which submit as soon as one character is inserted.
#[derive(Component)]
pub(crate) struct TextPrompt {
    pub(crate) submit_on_first_char: bool,
    pub(crate) bar: Entity,
}

/// Emitted (targeted at the prompt's `EditableText` entity) when the user
/// submits. Per-prompt observers read their intent component off the entity and
/// run the tmux command with `text`.
#[derive(EntityEvent, Debug, Clone)]
pub(crate) struct TextPromptSubmit {
    #[event_target]
    pub(crate) entity: Entity,
    pub(crate) text: String,
}

/// Registers the shared text-prompt resource, systems, and observers.
pub(crate) struct TextPromptPlugin;

impl Plugin for TextPromptPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ActiveTextPrompt>();
    }
}

/// What one Enter/Escape key press means for an open prompt.
#[derive(Debug, PartialEq, Eq)]
enum PromptAction {
    Submit,
    Cancel,
    Continue,
}

/// Maps a key press to a prompt action. Enter submits (unless the IME is
/// composing, so a conversion-confirming Enter is not mistaken for submit);
/// Escape cancels; every other key is left to the widget's own editing.
fn decide_prompt_key(key: &Key, composing: bool) -> PromptAction {
    match key {
        Key::Enter if !composing => PromptAction::Submit,
        Key::Escape => PromptAction::Cancel,
        _ => PromptAction::Continue,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn char_key(s: &str) -> Key {
        Key::Character(s.into())
    }

    #[test]
    fn enter_submits_when_not_composing() {
        assert_eq!(decide_prompt_key(&Key::Enter, false), PromptAction::Submit);
    }

    #[test]
    fn enter_is_continue_while_composing() {
        assert_eq!(decide_prompt_key(&Key::Enter, true), PromptAction::Continue);
    }

    #[test]
    fn escape_cancels() {
        assert_eq!(decide_prompt_key(&Key::Escape, false), PromptAction::Cancel);
    }

    #[test]
    fn other_keys_continue() {
        assert_eq!(
            decide_prompt_key(&char_key("a"), false),
            PromptAction::Continue
        );
        assert_eq!(
            decide_prompt_key(&Key::Backspace, false),
            PromptAction::Continue
        );
    }
}
