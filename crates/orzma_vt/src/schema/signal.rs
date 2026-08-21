//! Out-of-band signals the VT surfaces from the byte stream.

use crate::schema::{ApcWebviewVerb, PlacementId};
use std::path::PathBuf;

/// Out-of-band signal parsed from the VT byte stream, handed to the
/// owner in [`crate::VtUpdate::signals`].
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
    /// An APC-driven webview mount/unmount request from the PTY.
    /// `placement` is the VT-minted id, `Some` only for a `Mount` the
    /// VT accepted and registered; `None` is a policy rejection the
    /// consumer drops.
    ApcWebview {
        verb: ApcWebviewVerb,
        placement: Option<PlacementId>,
    },
    /// Placements the VT evicted on its own authority (history trim,
    /// alternate-screen teardown). Consumers despawn them by id;
    /// unknown ids are ignored. A remount's superseded id is never
    /// named here — supersession shows only as the id vanishing from
    /// the frame-carried placement lists.
    WebviewEvicted {
        placements: Vec<PlacementId>,
    },
    /// Tracked `TermMode` flags that transitioned since the previous
    /// signal drain, as mode names (e.g. "alt-screen").
    ModeChange {
        added: Vec<&'static str>,
        removed: Vec<&'static str>,
    },
}
