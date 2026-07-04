//! Copy-mode prompt command: the ozmux search/jump prompt submit.

use crate::input::quote;
use tmux_control::TmuxCommand;
use tmux_control_parser::PaneId;

/// The copy command an ozmux prompt feeds once the user submits text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    /// `/` — search down (regex prompt).
    SearchForward,
    /// `?` — search up (regex prompt).
    SearchBackward,
    /// `f` — jump to char forward (single-char prompt).
    JumpForward,
    /// `F` — jump to char backward (single-char prompt).
    JumpBackward,
    /// `t` — jump till char forward (single-char prompt).
    JumpToForward,
    /// `T` — jump till char backward (single-char prompt).
    JumpToBackward,
}

impl PromptKind {
    /// The tmux `-X` copy command name this prompt feeds.
    pub fn copy_command(self) -> &'static str {
        match self {
            PromptKind::SearchForward => "search-forward",
            PromptKind::SearchBackward => "search-backward",
            PromptKind::JumpForward => "jump-forward",
            PromptKind::JumpBackward => "jump-backward",
            PromptKind::JumpToForward => "jump-to-forward",
            PromptKind::JumpToBackward => "jump-to-backward",
        }
    }

    /// True for jump prompts, which read exactly one character.
    pub fn is_single_char(self) -> bool {
        !matches!(self, PromptKind::SearchForward | PromptKind::SearchBackward)
    }
}

/// `send-keys -X -t %N <copy-command> -- '<text>'` — an ozmux prompt submit.
pub struct Prompt<'a> {
    /// The target pane.
    pub pane: PaneId,
    /// Which copy-mode command to run on submit.
    pub kind: PromptKind,
    /// The user-supplied search text or jump character.
    pub text: &'a str,
}
impl TmuxCommand for Prompt<'_> {
    fn into_raw_command(self) -> String {
        format!(
            "send-keys -X -t %{} {} -- {}",
            self.pane.0,
            self.kind.copy_command(),
            quote(self.text)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_quotes_text_and_targets_pane() {
        assert_eq!(
            Prompt {
                pane: PaneId(2),
                kind: PromptKind::SearchForward,
                text: "foo bar"
            }
            .into_raw_command(),
            "send-keys -X -t %2 search-forward -- 'foo bar'"
        );
    }
}
