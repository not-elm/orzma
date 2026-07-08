//! Vi-mode search / jump prompt.
//!
//! Inert: nothing opens this prompt yet (the tmux VI applier that used to
//! trigger it was removed in the vi-mode-to-local migration). This module now
//! provides the EditableText-based submit path and `ViPromptIntent` so a future
//! local search applier can open a shared `text_prompt` (with
//! `submit_on_first_char = kind.is_single_char()`, label `prompt_label(kind)`,
//! and colors `bg: theme::PANEL` / `fg: theme::FOREGROUND`) and attach
//! `ViPromptIntent`. On submit, this observer runs
//! `send-keys -X -t %N <kind> -- '<text>'` against the active tmux connection.

use crate::ui::text_prompt::TextPromptSubmit;
use bevy::prelude::*;
use orzma_tmux::{PaneId, Prompt, PromptKind, TmuxClient};

/// Attached to a vi prompt's `EditableText` entity so the submit observer can
/// target the right pane and copy-mode command.
#[derive(Component)]
pub(crate) struct ViPromptIntent {
    pub(crate) pane: PaneId,
    pub(crate) kind: PromptKind,
}

/// Registers the vi-mode prompt submit observer.
pub(crate) struct ViModePromptPlugin;

impl Plugin for ViModePromptPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_vi_prompt_submit);
    }
}

/// The prompt label glyph shown before the typed text (`/`, `?`, `f`, …).
// NOTE: `#[expect]` is impractical here — dead in the non-test build (inert
// until a local search applier reconnects this prompt) but live under tests.
#[allow(dead_code)]
pub(crate) fn prompt_label(kind: PromptKind) -> &'static str {
    match kind {
        PromptKind::SearchForward => "/",
        PromptKind::SearchBackward => "?",
        PromptKind::JumpForward => "f",
        PromptKind::JumpBackward => "F",
        PromptKind::JumpToForward => "t",
        PromptKind::JumpToBackward => "T",
    }
}

fn on_vi_prompt_submit(
    submit: On<TextPromptSubmit>,
    mut client: Option<Single<&mut TmuxClient>>,
    intents: Query<&ViPromptIntent>,
) {
    let Ok(intent) = intents.get(submit.entity) else {
        return;
    };
    if let Some(client) = client.as_deref_mut()
        && let Err(e) = client.send(Prompt {
            pane: intent.pane,
            kind: intent.kind,
            text: &submit.text,
        })
    {
        tracing::warn!(?e, "vi-mode prompt submit failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_label_returns_correct_glyphs() {
        assert_eq!(prompt_label(PromptKind::SearchForward), "/");
        assert_eq!(prompt_label(PromptKind::SearchBackward), "?");
        assert_eq!(prompt_label(PromptKind::JumpForward), "f");
        assert_eq!(prompt_label(PromptKind::JumpBackward), "F");
        assert_eq!(prompt_label(PromptKind::JumpToForward), "t");
        assert_eq!(prompt_label(PromptKind::JumpToBackward), "T");
    }

    #[test]
    fn jump_kinds_submit_on_first_char() {
        assert!(PromptKind::JumpForward.is_single_char());
        assert!(!PromptKind::SearchForward.is_single_char());
    }
}
