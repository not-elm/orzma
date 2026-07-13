//! Shared modal-prompt building blocks for the confirm and rename prompts:
//! the "does a prompt own the keyboard" predicate, the chord-modifier
//! filter, the keys-since-open intake (`ModalKeys`), and the bottom-bar UI
//! helpers.
//!
//! The predicate is used two ways: by `confirm_prompt` / `rename_prompt`
//! themselves, to refuse opening a second modal while one is already up; and
//! by the input pipeline (`apply_type`, `resolve_key_effects`,
//! `read_ime_events`) to withhold typing, shortcuts, paste, and IME commits
//! from the focused pane while a prompt is open.

use crate::font::TerminalUiFont;
use crate::ui::multiplexer::confirm_prompt::ConfirmState;
use crate::ui::multiplexer::rename_prompt::RenameState;
use bevy::ecs::message::MessageCursor;
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;

/// Foreground (text) color shared by every modal bottom bar.
const MODAL_BAR_FG: Color = Color::srgb(0.95, 0.95, 0.95);
/// Font size of every modal bottom bar's text.
const MODAL_BAR_FONT_SIZE_PX: f32 = 12.0;

/// Whether a confirm or rename prompt currently owns the keyboard.
pub(crate) fn any_modal_open(
    confirm: Option<Res<ConfirmState>>,
    rename: Option<Res<RenameState>>,
) -> bool {
    confirm.is_some() || rename.is_some()
}

/// Whether a non-Shift chord modifier (Cmd / Ctrl / Alt) is held. A chord
/// like Cmd+Y is a command, not an answer or a typed character, so an open
/// prompt must ignore the key press it rides on. Shift is deliberately
/// excluded — `Y` and capitalized window names are legitimate prompt input.
pub(crate) fn chord_modifier_held(keys: &ButtonInput<KeyCode>) -> bool {
    keys.any_pressed([
        KeyCode::SuperLeft,
        KeyCode::SuperRight,
        KeyCode::ControlLeft,
        KeyCode::ControlRight,
        KeyCode::AltLeft,
        KeyCode::AltRight,
    ])
}

/// A modal prompt's keyboard intake: a cursor into the shared
/// `Messages<KeyboardInput>` buffer, positioned at open time past everything
/// already buffered (including the chord key that opened the prompt), so the
/// prompt consumes exactly the keys pressed after it opened — no arming
/// frame that could swallow a fast first keypress.
#[derive(Default)]
pub(crate) struct ModalKeys {
    cursor: MessageCursor<KeyboardInput>,
}

impl ModalKeys {
    /// Captures the open moment: subsequent reads see only keys buffered
    /// after this call.
    pub(crate) fn at_current(messages: &Messages<KeyboardInput>) -> Self {
        Self {
            cursor: messages.get_cursor_current(),
        }
    }

    /// Whether no unread key events exist — the cheap per-frame gate.
    pub(crate) fn is_empty(&self, messages: &Messages<KeyboardInput>) -> bool {
        self.cursor.is_empty(messages)
    }

    /// The logical keys pressed since the prompt opened, consuming the
    /// unread events. Keys riding a held chord modifier (Cmd/Ctrl/Alt) are
    /// consumed but filtered out — Cmd+V is a command, not prompt input.
    pub(crate) fn pressed<'a>(
        &'a mut self,
        messages: &'a Messages<KeyboardInput>,
        keys: &ButtonInput<KeyCode>,
    ) -> impl Iterator<Item = &'a Key> {
        let chord_held = chord_modifier_held(keys);
        self.cursor
            .read(messages)
            .filter(move |ev| ev.state == ButtonState::Pressed && !chord_held)
            .map(|ev| &ev.logical_key)
    }
}

/// Spawns the shared bottom-bar node for a modal prompt — absolute,
/// full-width, initially hidden, with one empty `Text` child — carrying
/// `marker` so the prompt's own systems can find it.
pub(super) fn spawn_bottom_bar(
    commands: &mut Commands,
    ui_font: Option<Res<TerminalUiFont>>,
    marker: impl Bundle,
    bg: Color,
    z: i32,
) {
    let ui = ui_font.as_deref().cloned().unwrap_or_default();
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                bottom: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Auto,
                display: Display::None,
                padding: UiRect::axes(Val::Px(6.0), Val::Px(3.0)),
                ..default()
            },
            BackgroundColor(bg),
            GlobalZIndex(z),
            marker,
        ))
        .with_children(|parent| {
            parent.spawn((
                Text::new(""),
                ui.text_font(FontSize::Px(MODAL_BAR_FONT_SIZE_PX)),
                TextColor(MODAL_BAR_FG),
            ));
        });
}

/// System: hides the `M`-marked bottom bar. Registered by each prompt plugin
/// with `run_if(resource_removed::<...>)`.
pub(super) fn hide_bar<M: Component>(mut bar: Query<&mut Node, With<M>>) {
    if let Ok(mut node) = bar.single_mut() {
        node.display = Display::None;
    }
}

/// Shows the `M`-marked bottom bar and writes `content` into its `Text`
/// child. Called by each prompt's show system with its own formatted label.
pub(super) fn show_bar_with_text<M: Component>(
    bar: &mut Query<(&mut Node, &Children), With<M>>,
    texts: &mut Query<&mut Text>,
    content: String,
) {
    let Ok((mut node, children)) = bar.single_mut() else {
        return;
    };
    node.display = Display::Flex;
    for child in children.iter() {
        if let Ok(mut text) = texts.get_mut(child) {
            text.0 = content.clone();
        }
    }
}
