//! Copy-mode / paste-buffer commands: scrolled capture, per-refresh state query,
//! ozmux prompt submit, and reading the top paste buffer.

use crate::enumerate::{COPY_STATE_FORMAT, capture_offsets};
use crate::input::quote;
use crate::keybindings::PromptKind;
use tmux_control::TmuxCommand;
use tmux_control_parser::PaneId;

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
