//! `alacritty_terminal::event::EventListener` implementation.
//!
//! Routes PtyWrite reply bytes and Title / Bell / Clipboard control
//! frames over `crossbeam-channel` into Bevy systems. Per the port
//! audit (spec § Risks), capability-query / wakeup / cursor-blink /
//! child-exit variants are listed explicitly so a future alacritty
//! release that adds a new variant fails the build until the new
//! variant is reviewed.

use crossbeam_channel::Sender;
use std::path::PathBuf;

/// Verb carried by `ControlFrame::OscWebview`: inline mount/unmount of a registered view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OscWebviewVerb {
    /// Mount a registered webview INLINE at the cursor anchor, sized in cells.
    Mount {
        view_id: String,
        rows: u16,
        cols: u16,
        /// Client-assigned instance id (Kitty placement model); `None` is the
        /// implicit default instance. `(view_id, instance_id)` is the address.
        instance_id: Option<String>,
    },
    /// Unmount inline webview(s): a specific `(view_id, instance_id)`, all
    /// instances of a `view_id`, or all for this terminal.
    ///
    /// # Invariants
    /// `view_id == None` implies `instance_id == None` (an instance is
    /// addressable only alongside its view id; enforced at the capture stage).
    Unmount {
        view_id: Option<String>,
        instance_id: Option<String>,
    },
}

/// How an inline webview is anchored to its terminal.
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

/// Best-effort control frames forwarded from `TermListener`. The
/// channel is currently unbounded; see spec § Risks > "control_tx is
/// unbounded" for the back-pressure trade-off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ControlFrame {
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
        verb: OscWebviewVerb,
        anchor: Option<InlineAnchor>,
    },
}

/// Reply-required reply bytes (currently just `PtyWrite`). The
/// channel uses unbounded crossbeam — must-not-drop semantics.
pub(crate) struct TermListener {
    pub reply_tx: Sender<Vec<u8>>,
    pub control_tx: Sender<ControlFrame>,
}

impl alacritty_terminal::event::EventListener for TermListener {
    fn send_event(&self, event: alacritty_terminal::event::Event) {
        use alacritty_terminal::event::Event;

        match event {
            Event::PtyWrite(s) => {
                let _ = self.reply_tx.send(s.into_bytes());
            }
            Event::Title(s) => {
                if let Err(e) = self.control_tx.send(ControlFrame::Title(s)) {
                    tracing::warn!(?e, "control_tx send(Title) failed");
                }
            }
            Event::ResetTitle => {
                if let Err(e) = self.control_tx.send(ControlFrame::ResetTitle) {
                    tracing::warn!(?e, "control_tx send(ResetTitle) failed");
                }
            }
            Event::Bell => {
                if let Err(e) = self.control_tx.send(ControlFrame::Bell) {
                    tracing::warn!(?e, "control_tx send(Bell) failed");
                }
            }
            Event::ClipboardStore(_clip, content) => {
                let frame = ControlFrame::Clipboard { content };
                if let Err(e) = self.control_tx.send(frame) {
                    tracing::warn!(?e, "control_tx send(Clipboard) failed");
                }
            }
            // Explicitly enumerate no-op variants so an alacritty
            // upgrade that adds a new variant fails the build until
            // the new variant is reviewed.
            Event::TextAreaSizeRequest(_)
            | Event::ColorRequest(_, _)
            | Event::ClipboardLoad(_, _)
            | Event::ChildExit(_)
            | Event::Exit
            | Event::Wakeup
            | Event::MouseCursorDirty
            | Event::CursorBlinkingChange => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::event::{Event, EventListener};
    use crossbeam_channel::unbounded;

    #[test]
    fn inline_anchor_is_copy_and_eq() {
        let a = InlineAnchor {
            mode: AnchorMode::Scrollback { line: 42, col: 7 },
            frame_seq: 3,
        };
        let b = a;
        assert_eq!(a, b);
    }

    #[test]
    fn pty_write_event_is_forwarded() {
        let (reply_tx, reply_rx) = unbounded::<Vec<u8>>();
        let (control_tx, _control_rx) = unbounded::<ControlFrame>();
        let listener = TermListener {
            reply_tx,
            control_tx,
        };
        listener.send_event(Event::PtyWrite("\x1b[?6n".into()));
        assert_eq!(reply_rx.try_recv().unwrap(), b"\x1b[?6n");
    }

    #[test]
    fn title_event_is_forwarded() {
        let (reply_tx, _reply_rx) = unbounded::<Vec<u8>>();
        let (control_tx, control_rx) = unbounded::<ControlFrame>();
        let listener = TermListener {
            reply_tx,
            control_tx,
        };
        listener.send_event(Event::Title("alpha".into()));
        assert_eq!(
            control_rx.try_recv().unwrap(),
            ControlFrame::Title("alpha".to_string())
        );
    }

    #[test]
    fn reset_title_event_is_forwarded() {
        let (reply_tx, _reply_rx) = unbounded::<Vec<u8>>();
        let (control_tx, control_rx) = unbounded::<ControlFrame>();
        let listener = TermListener {
            reply_tx,
            control_tx,
        };
        listener.send_event(Event::ResetTitle);
        assert_eq!(control_rx.try_recv().unwrap(), ControlFrame::ResetTitle);
    }

    #[test]
    fn bell_event_is_forwarded() {
        let (reply_tx, _reply_rx) = unbounded::<Vec<u8>>();
        let (control_tx, control_rx) = unbounded::<ControlFrame>();
        let listener = TermListener {
            reply_tx,
            control_tx,
        };
        listener.send_event(Event::Bell);
        assert_eq!(control_rx.try_recv().unwrap(), ControlFrame::Bell);
    }
}
