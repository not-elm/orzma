use std::path::PathBuf;

use crate::extension::ApcWebviewVerb;

/// Best-effort control frames forwarded from `TermListener`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlFrame {
    Bell,
    Title(String),
    ResetTitle,
    Clipboard {
        content: String,
    },
    /// A new current working directory reported via OSC 7.
    CurrentDir(PathBuf),
    /// An OSC-driven webview mount/unmount request from the PTY.
    /// `anchor` is `Some` only for `Mount` (stamped in `handle.rs`).
    OscWebview {
        verb: ApcWebviewVerb,
        anchor: Option<InlineAnchor>,
    },
}

/// Anchor stamped by the VT thread at the exact byte position of a
/// `mount` OSC: the anchor mode (scrollback vs alternate-screen) and
/// the `frame_seq` the next grid emit will carry (used by the GUI to defer
/// first projection until the grid catches up).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InlineAnchor {
    /// Where the rect is anchored.
    pub mode: AnchorMode,
    /// The seq value the next emitted frame will carry (wrap-aware compare).
    pub frame_seq: u32,
}

/// How a webview is anchored to its terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnchorMode {
    /// Anchored to an absolute scrollback line; scrolls with the text
    /// (`line = history_base + history_size + live-grid cursor row`).
    Scrollback {
        /// Absolute scrollback line of the rect's top row.
        line: u64,
        /// Cursor column at the OSC byte position.
        col: u16,
    },
    /// Anchored to a viewport-relative cell; fixed on the visible alternate
    /// screen (`row` is the 0-based grid row of the cursor at the OSC).
    FixedScreen {
        /// Viewport-relative row of the rect's top cell.
        row: u16,
        /// Cursor column at the OSC byte position.
        col: u16,
    },
}
