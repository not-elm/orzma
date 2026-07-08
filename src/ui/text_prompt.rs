//! Shared single-line text-input overlay built on Bevy's `EditableText`.
//!
//! Both the tmux rename prompt and the vi-search prompt spawn one of these,
//! set `InputFocus` to the `EditableText` entity, and let `bevy_ui_widgets`'
//! `EditableTextInputPlugin` drive keyboard, IME, clipboard, and pointer
//! editing. This module owns keyboard suppression (via `ActiveTextPrompt`),
//! submit/cancel, and teardown; per-prompt modules observe `TextPromptSubmit`
//! and run the tmux command.

use crate::app_mode::AppMode;
use crate::theme;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input_focus::{FocusCause, FocusedInput, InputFocus};
use bevy::prelude::*;
use bevy::text::{EditableText, EditableTextSystems, TextEditChange};
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
    submit_on_first_char: bool,
    bar: Entity,
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
///
/// The bar carries `DespawnOnExit(AppMode::Tmux)`: both callers open the prompt
/// only in tmux mode, so a prompt still open when the tmux connection drops is
/// torn down with the mode instead of surviving to freeze Default-mode input.
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
            DespawnOnExit(AppMode::Tmux),
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
            .add_systems(Update, reconcile_active_text_prompt)
            .add_systems(PostUpdate, apply_prompt_close.after(EditableTextSystems));
    }
}

/// What one Enter/Escape key press means for an open prompt.
#[derive(Debug, PartialEq, Eq)]
enum PromptAction {
    Submit,
    Cancel,
    Continue,
}

/// Marks a prompt whose submit/cancel has been decided. `apply_prompt_close`
/// (`PostUpdate`, after `EditableTextSystems`) reads the field's final value and,
/// when `submit` is set, fires `TextPromptSubmit` — after the same-frame
/// keystrokes have been applied — then tears the prompt down. Teardown is
/// deferred (not done in the `FocusedInput` observer, which runs in `PreUpdate`)
/// so `ActiveTextPrompt` stays set through the Update-phase input gates on the
/// close frame — otherwise the terminating Enter/Escape leaks past the drain
/// guard into the focused pane. Because each entity carries exactly one
/// `PromptClosing` and is processed once, submit fires exactly once even if two
/// Enter events land in the same frame.
#[derive(Component)]
struct PromptClosing {
    submit: bool,
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

/// Applies submit and tears down prompts marked `PromptClosing`: for each closing
/// entity, fires `TextPromptSubmit` with the post-edit value when `submit` is set,
/// then despawns the bar and clears focus and the active signal. Runs in
/// `PostUpdate` after `EditableTextSystems`, so `value()` already reflects the
/// same-frame keystrokes (no stale read) and, because each entity carries one
/// `PromptClosing` processed once here, submit fires exactly once (no double
/// submit from two Enter events). Running after the Update-phase input gates also
/// keeps the closing frame's terminating key drained while the prompt was
/// nominally open. Idempotent and tolerant of an already-cleared prompt.
fn apply_prompt_close(
    mut commands: Commands,
    mut input_focus: ResMut<InputFocus>,
    mut active: ResMut<ActiveTextPrompt>,
    closing: Query<(Entity, &TextPrompt, &EditableText, &PromptClosing)>,
) {
    for (entity, prompt, editable, closing) in &closing {
        if closing.submit {
            commands.trigger(TextPromptSubmit {
                entity,
                text: editable.value().to_string(),
            });
        }
        commands.entity(prompt.bar).despawn();
        if active.0 == Some(entity) {
            active.0 = None;
        }
        if input_focus.get() == Some(entity) {
            input_focus.clear();
        }
    }
}

/// Decides submit/cancel for a focused `TextPrompt` and marks it
/// `PromptClosing { submit }` — Enter marks `submit: true`, Escape marks
/// `submit: false`, every other key is left to the widget. It does NOT read the
/// value or fire `TextPromptSubmit` itself: `apply_prompt_close` (PostUpdate,
/// after `EditableTextSystems`) reads the post-edit value and fires submit once
/// per closing entity, which avoids both a stale same-frame value read and a
/// double submit from two Enter events in one frame. The `Without<PromptClosing>`
/// filter skips an already-closing entity as belt-and-suspenders. The only
/// `EditableText` read left is `is_composing()`.
///
/// This is a GLOBAL observer for a bubbling event: `FocusedInput` auto-propagates
/// editable → bar → window, and the widget does NOT consume `Enter`
/// (`allow_newlines == false`), so without a guard this observer fires once per
/// bubble hop — closing three times for one Enter. `event_target()` is
/// `focused_entity`, which Bevy mutates in place at each hop, so it always
/// equals `event_target()`; the `original_event_target()` guard below compares
/// against the propagation-invariant original target instead, firing exactly
/// once, at the editable's own hop. (`propagate(false)` alone is unreliable:
/// observer order lets the widget re-set propagation.)
fn on_prompt_key(
    key: On<FocusedInput<KeyboardInput>>,
    mut commands: Commands,
    prompts: Query<&EditableText, (With<TextPrompt>, Without<PromptClosing>)>,
) {
    if key.event_target() != key.original_event_target() {
        return;
    }
    if !key.input.state.is_pressed() {
        return;
    }
    let Ok(editable) = prompts.get(key.focused_entity) else {
        return;
    };
    match decide_prompt_key(&key.input.logical_key, editable.is_composing()) {
        PromptAction::Continue => {}
        PromptAction::Submit => {
            commands
                .entity(key.focused_entity)
                .insert(PromptClosing { submit: true });
        }
        PromptAction::Cancel => {
            commands
                .entity(key.focused_entity)
                .insert(PromptClosing { submit: false });
        }
    }
}

/// Marks a vi jump prompt (`submit_on_first_char`) `PromptClosing { submit: true }`
/// the moment its value becomes non-empty. `EditableText::new("")` queues an
/// empty-value edit whose `TextEditChange` is ignored here; the first inserted
/// char marks the prompt. The actual submitted text is re-read by
/// `apply_prompt_close` (which runs after `EditableTextSystems`, so `value()` is
/// current); the non-empty check here only decides whether to submit at all.
fn on_prompt_edit(
    change: On<TextEditChange>,
    mut commands: Commands,
    prompts: Query<(&TextPrompt, &EditableText), Without<PromptClosing>>,
) {
    let entity = change.event_target();
    let Ok((prompt, editable)) = prompts.get(entity) else {
        return;
    };
    if !prompt.submit_on_first_char {
        return;
    }
    if editable.value() == "" {
        return;
    }
    commands
        .entity(entity)
        .insert(PromptClosing { submit: true });
}

/// Clears `ActiveTextPrompt` when its entity was despawned by something other
/// than `apply_prompt_close` (e.g. surface teardown). `InputFocus` self-heals on
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
    use bevy::input::keyboard::KeyCode;
    use bevy::input::{ButtonState, InputPlugin};
    use bevy::input_focus::{InputDispatchPlugin, InputFocusPlugin};
    use bevy::window::{PrimaryWindow, Window};

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

        let bar = app.world_mut().spawn_empty().id();
        let editable = app
            .world_mut()
            .spawn(TextPrompt {
                submit_on_first_char: false,
                bar,
            })
            .id();
        app.world_mut().resource_mut::<ActiveTextPrompt>().0 = Some(editable);

        app.world_mut().entity_mut(editable).despawn();
        app.update();

        assert!(
            app.world().resource::<ActiveTextPrompt>().0.is_none(),
            "self-heal must clear ActiveTextPrompt when its entity no longer exists, or the keyboard freezes"
        );
    }

    #[test]
    fn on_prompt_key_fires_once_even_when_ancestor_also_matches_query() {
        #[derive(Resource, Default)]
        struct SubmitCount(u32);

        fn count_submits(_submit: On<TextPromptSubmit>, mut count: ResMut<SubmitCount>) {
            count.0 += 1;
        }

        let mut app = App::new();
        app.add_plugins((InputPlugin, InputFocusPlugin, InputDispatchPlugin))
            .init_resource::<ActiveTextPrompt>()
            .init_resource::<SubmitCount>()
            .add_observer(on_prompt_key)
            .add_observer(count_submits)
            .add_systems(PostUpdate, apply_prompt_close.after(EditableTextSystems));

        let window = app
            .world_mut()
            .spawn((Window::default(), PrimaryWindow))
            .id();
        app.update();

        let bar = app
            .world_mut()
            .spawn((
                EditableText::new(""),
                TextPrompt {
                    submit_on_first_char: false,
                    bar: Entity::PLACEHOLDER,
                },
            ))
            .id();
        let editable = app
            .world_mut()
            .spawn((
                EditableText::new("x"),
                TextPrompt {
                    submit_on_first_char: false,
                    bar,
                },
                ChildOf(bar),
            ))
            .id();

        app.world_mut()
            .resource_mut::<InputFocus>()
            .set(editable, FocusCause::Navigated);
        app.world_mut().write_message(KeyboardInput {
            key_code: KeyCode::Enter,
            logical_key: Key::Enter,
            state: ButtonState::Pressed,
            text: None,
            repeat: false,
            window,
        });
        app.update();

        assert_eq!(
            app.world().resource::<SubmitCount>().0,
            1,
            "on_prompt_key must fire exactly once, at the editable's own hop, not at every \
             bubble ancestor that also matches the (TextPrompt, EditableText) query"
        );
    }

    #[test]
    fn two_enter_events_in_one_frame_submit_exactly_once() {
        #[derive(Resource, Default)]
        struct SubmitCount(u32);

        fn count_submits(_submit: On<TextPromptSubmit>, mut count: ResMut<SubmitCount>) {
            count.0 += 1;
        }

        let mut app = App::new();
        app.add_plugins((InputPlugin, InputFocusPlugin, InputDispatchPlugin))
            .init_resource::<ActiveTextPrompt>()
            .init_resource::<SubmitCount>()
            .add_observer(on_prompt_key)
            .add_observer(count_submits)
            .add_systems(PostUpdate, apply_prompt_close.after(EditableTextSystems));

        let window = app
            .world_mut()
            .spawn((Window::default(), PrimaryWindow))
            .id();
        app.update();

        let bar = app.world_mut().spawn_empty().id();
        let editable = app
            .world_mut()
            .spawn((
                EditableText::new("x"),
                TextPrompt {
                    submit_on_first_char: false,
                    bar,
                },
                ChildOf(bar),
            ))
            .id();
        app.world_mut()
            .resource_mut::<InputFocus>()
            .set(editable, FocusCause::Navigated);

        for _ in 0..2 {
            app.world_mut().write_message(KeyboardInput {
                key_code: KeyCode::Enter,
                logical_key: Key::Enter,
                state: ButtonState::Pressed,
                text: None,
                repeat: false,
                window,
            });
        }
        app.update();

        assert_eq!(
            app.world().resource::<SubmitCount>().0,
            1,
            "two Enter events buffered in one frame (auto-repeat) must submit exactly once: \
             submit is applied once per PromptClosing entity in PostUpdate, not once per \
             FocusedInput event in PreUpdate"
        );
    }

    #[test]
    fn active_text_prompt_stays_some_through_update_on_close_frame() {
        #[derive(Resource, Default)]
        struct SawActiveDuringUpdate(bool);

        fn probe_active_during_update(
            mut saw: ResMut<SawActiveDuringUpdate>,
            active: Res<ActiveTextPrompt>,
        ) {
            if active.0.is_some() {
                saw.0 = true;
            }
        }

        let mut app = App::new();
        app.add_plugins((InputPlugin, InputFocusPlugin, InputDispatchPlugin))
            .init_resource::<ActiveTextPrompt>()
            .init_resource::<SawActiveDuringUpdate>()
            .add_observer(on_prompt_key)
            .add_systems(Update, probe_active_during_update)
            .add_systems(PostUpdate, apply_prompt_close.after(EditableTextSystems));

        let window = app
            .world_mut()
            .spawn((Window::default(), PrimaryWindow))
            .id();
        app.update();

        let bar = app.world_mut().spawn_empty().id();
        let editable = app
            .world_mut()
            .spawn((
                EditableText::new("x"),
                TextPrompt {
                    submit_on_first_char: false,
                    bar,
                },
                ChildOf(bar),
            ))
            .id();
        app.world_mut().resource_mut::<ActiveTextPrompt>().0 = Some(editable);
        app.world_mut()
            .resource_mut::<InputFocus>()
            .set(editable, FocusCause::Navigated);

        app.world_mut().write_message(KeyboardInput {
            key_code: KeyCode::Enter,
            logical_key: Key::Enter,
            state: ButtonState::Pressed,
            text: None,
            repeat: false,
            window,
        });
        app.update();

        assert!(
            app.world().resource::<SawActiveDuringUpdate>().0,
            "ActiveTextPrompt must remain Some through Update on the close frame so the \
             terminating key is still drained — else it leaks to the pane"
        );
        assert!(
            app.world().resource::<ActiveTextPrompt>().0.is_none(),
            "PostUpdate teardown must clear ActiveTextPrompt after the close frame"
        );
        assert!(
            app.world().get_entity(bar).is_err(),
            "PostUpdate teardown must despawn the prompt bar"
        );
    }

    #[test]
    fn open_prompt_despawns_and_active_clears_on_tmux_exit() {
        use bevy::state::app::StatesPlugin;

        let mut app = App::new();
        app.add_plugins((MinimalPlugins, StatesPlugin))
            .init_resource::<ActiveTextPrompt>()
            .init_resource::<InputFocus>()
            .add_systems(Update, reconcile_active_text_prompt);
        app.insert_state(AppMode::Tmux);

        let bar = app
            .world_mut()
            .spawn((Node::default(), DespawnOnExit(AppMode::Tmux)))
            .id();
        let editable = app
            .world_mut()
            .spawn((
                EditableText::new("x"),
                TextPrompt {
                    submit_on_first_char: false,
                    bar,
                },
                ChildOf(bar),
            ))
            .id();
        app.world_mut().resource_mut::<ActiveTextPrompt>().0 = Some(editable);
        app.update();

        app.world_mut()
            .resource_mut::<NextState<AppMode>>()
            .set(AppMode::Default);
        app.update();
        app.update();

        assert!(
            app.world().get_entity(bar).is_err(),
            "leaving Tmux mode must despawn the DespawnOnExit-scoped prompt bar"
        );
        assert!(
            app.world().resource::<ActiveTextPrompt>().0.is_none(),
            "reconcile must clear ActiveTextPrompt after the tmux-scoped prompt despawns, \
             else Default-mode mouse/IME gates keep suppressing input (frozen keyboard)"
        );
    }
}
