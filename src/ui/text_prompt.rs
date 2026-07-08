//! Shared single-line text-input overlay built on Bevy's `EditableText`.
//!
//! Both the tmux rename prompt and the vi-search prompt spawn one of these,
//! set `InputFocus` to the `EditableText` entity, and let `bevy_ui_widgets`'
//! `EditableTextInputPlugin` drive keyboard, IME, clipboard, and pointer
//! editing. This module owns keyboard suppression (via `ActiveTextPrompt`),
//! submit/cancel, and teardown; per-prompt modules observe `TextPromptSubmit`
//! and run the tmux command.

use crate::theme;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input_focus::{FocusCause, FocusedInput, InputFocus};
use bevy::prelude::*;
use bevy::text::{EditableText, TextEditChange};
use bevy::ui_widgets::SelectAllOnFocus;

const TEXT_PROMPT_Z: i32 = 340;

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

/// Describes a prompt to open: leading `label`, pre-filled `initial` text,
/// whether to submit on the first inserted char (vi jump), whether to
/// select-all on focus (rename, so the first keystroke replaces the name), and
/// the bar background / text colors (each prompt keeps its own theme).
pub(crate) struct TextPromptSpec {
    pub(crate) label: String,
    pub(crate) initial: String,
    pub(crate) submit_on_first_char: bool,
    pub(crate) select_all: bool,
    pub(crate) bg: Color,
    pub(crate) fg: Color,
}

/// Spawns the overlay bar (label + `EditableText`), points `InputFocus` and
/// `ActiveTextPrompt` at the field, and returns the `EditableText` entity so the
/// caller can attach a per-prompt intent component. Focus is set with
/// `FocusCause::Navigated` so a `SelectAllOnFocus` field selects its text.
pub(crate) fn spawn_text_prompt(
    commands: &mut Commands,
    input_focus: &mut InputFocus,
    active: &mut ActiveTextPrompt,
    font: Handle<Font>,
    spec: TextPromptSpec,
) -> Entity {
    let bar = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                bottom: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Auto,
                align_items: AlignItems::Center,
                padding: UiRect::axes(Val::Px(6.0), Val::Px(3.0)),
                ..default()
            },
            BackgroundColor(spec.bg),
            GlobalZIndex(TEXT_PROMPT_Z),
        ))
        .id();
    commands.entity(bar).with_children(|parent| {
        parent.spawn((
            Text::new(spec.label),
            TextFont {
                font: font.clone().into(),
                font_size: FontSize::Px(theme::UI_FONT_SIZE),
                ..default()
            },
            TextColor(spec.fg),
        ));
    });
    let mut editable = commands.spawn((
        EditableText::new(&spec.initial),
        TextFont {
            font: font.into(),
            font_size: FontSize::Px(theme::UI_FONT_SIZE),
            ..default()
        },
        TextColor(spec.fg),
        TextPrompt {
            submit_on_first_char: spec.submit_on_first_char,
            bar,
        },
        ChildOf(bar),
    ));
    if spec.select_all {
        editable.insert(SelectAllOnFocus);
    }
    let editable = editable.id();
    active.0 = Some(editable);
    input_focus.set(editable, FocusCause::Navigated);
    editable
}

/// Registers the shared text-prompt resource, systems, and observers.
pub(crate) struct TextPromptPlugin;

impl Plugin for TextPromptPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ActiveTextPrompt>()
            .add_observer(on_prompt_key)
            .add_observer(on_prompt_edit)
            .add_systems(Update, reconcile_active_text_prompt);
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

/// Tears the prompt down: despawns the overlay bar (and its children), clears
/// the focus and the active-prompt signal. Idempotent.
fn close_prompt(
    commands: &mut Commands,
    input_focus: &mut InputFocus,
    active: &mut ActiveTextPrompt,
    bar: Entity,
) {
    commands.entity(bar).despawn();
    input_focus.clear();
    active.0 = None;
}

/// Handles Enter/Escape for a focused `TextPrompt`: emits `TextPromptSubmit`
/// (with the current value) on Enter, or tears the prompt down on Escape.
///
/// This is a GLOBAL observer for a bubbling event: `FocusedInput` auto-propagates
/// editable → bar → window, and the widget does NOT consume `Enter`
/// (`allow_newlines == false`), so without a guard this observer fires once per
/// bubble hop — submitting three times for one Enter. The
/// `event_target() != focused_entity` guard fires it exactly once, at the
/// editable's own hop. (`propagate(false)` alone is unreliable: observer order
/// lets the widget re-set propagation.)
fn on_prompt_key(
    key: On<FocusedInput<KeyboardInput>>,
    mut commands: Commands,
    mut input_focus: ResMut<InputFocus>,
    mut active: ResMut<ActiveTextPrompt>,
    prompts: Query<(&TextPrompt, &EditableText)>,
) {
    // Fire only at the editable's own hop, not at each bubble ancestor.
    if key.event_target() != key.focused_entity {
        return;
    }
    if !key.input.state.is_pressed() {
        return;
    }
    let Ok((prompt, editable)) = prompts.get(key.focused_entity) else {
        return;
    };
    match decide_prompt_key(&key.input.logical_key, editable.is_composing()) {
        PromptAction::Continue => {}
        PromptAction::Submit => {
            let text = editable.value().to_string();
            commands.trigger(TextPromptSubmit {
                entity: key.focused_entity,
                text,
            });
            close_prompt(&mut commands, &mut input_focus, &mut active, prompt.bar);
        }
        PromptAction::Cancel => {
            close_prompt(&mut commands, &mut input_focus, &mut active, prompt.bar);
        }
    }
}

/// Submits a vi jump prompt (`submit_on_first_char`) the moment its value
/// becomes non-empty. `EditableText::new("")` queues an empty-value edit whose
/// `TextEditChange` is ignored here; the first inserted char triggers submit.
fn on_prompt_edit(
    change: On<TextEditChange>,
    mut commands: Commands,
    mut input_focus: ResMut<InputFocus>,
    mut active: ResMut<ActiveTextPrompt>,
    prompts: Query<(&TextPrompt, &EditableText)>,
) {
    let entity = change.event_target();
    let Ok((prompt, editable)) = prompts.get(entity) else {
        return;
    };
    if !prompt.submit_on_first_char {
        return;
    }
    let text = editable.value().to_string();
    if text.is_empty() {
        return;
    }
    commands.trigger(TextPromptSubmit { entity, text });
    close_prompt(&mut commands, &mut input_focus, &mut active, prompt.bar);
}

/// Clears `ActiveTextPrompt` when its entity was despawned by something other
/// than `close_prompt` (e.g. surface teardown). `InputFocus` self-heals on
/// despawn but `ActiveTextPrompt` does not; without this every input gate keeps
/// draining keys → keyboard freeze.
fn reconcile_active_text_prompt(
    mut active: ResMut<ActiveTextPrompt>,
    prompts: Query<(), With<TextPrompt>>,
) {
    if let Some(entity) = active.0
        && !prompts.contains(entity)
    {
        active.0 = None;
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

    #[test]
    fn self_heal_clears_active_when_prompt_entity_despawned() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<ActiveTextPrompt>()
            .init_resource::<InputFocus>()
            .add_systems(Update, reconcile_active_text_prompt);

        // A prompt entity carrying TextPrompt, pointed to by ActiveTextPrompt.
        let bar = app.world_mut().spawn_empty().id();
        let editable = app
            .world_mut()
            .spawn(TextPrompt {
                submit_on_first_char: false,
                bar,
            })
            .id();
        app.world_mut().resource_mut::<ActiveTextPrompt>().0 = Some(editable);

        // External teardown despawns the prompt entity without going through close_prompt.
        app.world_mut().entity_mut(editable).despawn();
        app.update();

        assert!(
            app.world().resource::<ActiveTextPrompt>().0.is_none(),
            "self-heal must clear ActiveTextPrompt when its entity no longer exists, or the keyboard freezes"
        );
    }
}
