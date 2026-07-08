//! orzma-owned rename prompt. The rename-window / rename-session actions open a
//! shared `text_prompt` pre-filled with the current name; on submit this
//! module's observer rebuilds a safely-quoted rename command and sends it.

use crate::ui::text_prompt::TextPromptSubmit;
use bevy::prelude::*;
use orzma_tmux::{RenameSession, RenameWindow, SessionId, TmuxClient, TmuxCommand, WindowId};

/// What is being renamed: the captured target id plus its current name. One
/// enum so an invalid kind/id pairing is unrepresentable.
pub(crate) enum RenameSubject {
    /// A window, targeted by `@id`.
    Window {
        /// tmux window id captured at prompt-open.
        id: WindowId,
        /// The window's name at prompt-open (used to pre-fill the field).
        current_name: String,
    },
    /// A session, targeted by `$id`.
    Session {
        /// tmux session id captured at prompt-open.
        id: SessionId,
        /// The session's name at prompt-open (used to pre-fill the field).
        current_name: String,
    },
}

impl RenameSubject {
    /// The subject's current name, used to pre-fill the prompt field.
    pub(crate) fn current_name(&self) -> &str {
        match self {
            RenameSubject::Window { current_name, .. }
            | RenameSubject::Session { current_name, .. } => current_name,
        }
    }

    /// The prompt bar's leading label for this subject.
    pub(crate) fn label(&self) -> &'static str {
        match self {
            RenameSubject::Window { .. } => "Rename window: ",
            RenameSubject::Session { .. } => "Rename session: ",
        }
    }

    /// Builds the tmux rename command from the subject and the submitted text.
    fn submit_command(&self, text: &str) -> String {
        match self {
            RenameSubject::Window { id, .. } => RenameWindow {
                id: *id,
                name: text,
            }
            .into_raw_command(),
            RenameSubject::Session { id, .. } => RenameSession {
                id: *id,
                name: text,
            }
            .into_raw_command(),
        }
    }
}

/// Attached to a rename prompt's `EditableText` entity so the submit observer
/// can rebuild the rename command from the captured target.
#[derive(Component)]
pub(crate) struct RenameIntent(pub(crate) RenameSubject);

/// Registers the rename-prompt submit observer.
pub(super) struct RenamePromptPlugin;

impl Plugin for RenamePromptPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_rename_submit);
    }
}

fn on_rename_submit(
    submit: On<TextPromptSubmit>,
    mut client: Option<Single<&mut TmuxClient>>,
    intents: Query<&RenameIntent>,
) {
    let Ok(RenameIntent(subject)) = intents.get(submit.entity) else {
        return;
    };
    let cmd = subject.submit_command(&submit.text);
    if let Some(client) = client.as_deref_mut()
        && let Err(e) = client.send_raw(&cmd)
    {
        tracing::warn!(?e, "rename submit failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_submit_command_matches_legacy_format() {
        let subject = RenameSubject::Window {
            id: WindowId(2),
            current_name: "old".to_string(),
        };
        assert_eq!(
            subject.submit_command("new name"),
            "rename-window -t @2 -- 'new name'"
        );
    }

    #[test]
    fn session_submit_command_matches_legacy_format() {
        let subject = RenameSubject::Session {
            id: SessionId(1),
            current_name: "old".to_string(),
        };
        assert_eq!(
            subject.submit_command("proj"),
            "rename-session -t $1 -- proj"
        );
    }

    #[test]
    fn label_matches_subject() {
        assert_eq!(
            RenameSubject::Window {
                id: WindowId(0),
                current_name: String::new()
            }
            .label(),
            "Rename window: "
        );
        assert_eq!(
            RenameSubject::Session {
                id: SessionId(0),
                current_name: String::new()
            }
            .label(),
            "Rename session: "
        );
    }
}
