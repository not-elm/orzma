//! Out-of-band signals the VT surfaces from the byte stream.

use crate::schema::{ApcWebviewVerb, InlineAnchor};
use std::path::PathBuf;

/// Out-of-band signal parsed from the VT byte stream, drained by the
/// owner via `OrzmaVt::drain_signals`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VtSignal {
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
    ApcWebview {
        verb: ApcWebviewVerb,
        anchor: Option<InlineAnchor>,
    },
    /// Tracked `TermMode` flags that transitioned since the previous
    /// signal drain, as wire mode names (e.g. "alt-screen").
    ModeChange {
        added: Vec<&'static str>,
        removed: Vec<&'static str>,
    },
}
