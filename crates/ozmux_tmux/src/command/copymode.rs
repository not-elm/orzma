//! Copy-mode / paste-buffer commands: scrolled capture, per-refresh state query,
//! ozmux prompt submit, and reading the top paste buffer.

use crate::enumerate::{COPY_STATE_FORMAT, capture_offsets};
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

/// `capture-pane -p -e -t %N -S <start> -E <end>` for the scrolled copy view.
pub struct CopyModeCapture {
    /// The target pane.
    pub pane: PaneId,
    /// Lines scrolled back from the live tail.
    pub scroll_position: u32,
    /// Visible pane height in rows.
    pub pane_height: u16,
}
impl TmuxCommand for CopyModeCapture {
    fn into_raw_command(self) -> String {
        let (start, end) = capture_offsets(self.scroll_position, self.pane_height);
        format!("capture-pane -p -e -t %{} -S {start} -E {end}", self.pane.0)
    }
}

/// `display-message -p -t %N "<COPY_STATE_FORMAT>"` — one copy-mode state snapshot.
pub struct CopyStateQuery {
    /// The target pane.
    pub pane: PaneId,
}

impl TmuxCommand for CopyStateQuery {
    fn into_raw_command(self) -> String {
        format!(
            "display-message -p -t %{} \"{COPY_STATE_FORMAT}\"",
            self.pane.0
        )
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

/// `show-buffer` — reads tmux's top paste buffer for the clipboard bridge.
pub struct ShowBuffer;
impl TmuxCommand for ShowBuffer {
    fn into_raw_command(self) -> String {
        "show-buffer".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_mode_capture_uses_scroll_offsets() {
        assert_eq!(
            CopyModeCapture {
                pane: PaneId(3),
                scroll_position: 12,
                pane_height: 8
            }
            .into_raw_command(),
            "capture-pane -p -e -t %3 -S -12 -E -5"
        );
    }

    #[test]
    fn copy_state_query_targets_pane() {
        assert_eq!(
            CopyStateQuery { pane: PaneId(4) }.into_raw_command(),
            format!("display-message -p -t %4 \"{COPY_STATE_FORMAT}\"")
        );
    }

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

    #[test]
    fn show_buffer_is_literal() {
        assert_eq!(ShowBuffer.into_raw_command(), "show-buffer");
    }
}
