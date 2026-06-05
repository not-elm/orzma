//! Logical surface model — a pane's content. GUI/host-side surface
//! metadata (`ExtensionSurfaceId`, `OwningExtension`, webview wiring) stays
//! in the Bevy/extension layer; only logical data lives here.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// What a surface renders.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SurfaceKind {
    /// PTY-backed terminal.
    Terminal,
    /// Extension surface served from a Node process; `entry` is the HTML
    /// entry path relative to the extension dir.
    Extension {
        /// HTML entry path relative to the extension dir.
        entry: PathBuf,
    },
    /// Embedded browser surface.
    Browser {
        /// URL to open on creation, or `None` for the browser default.
        initial_url: Option<String>,
        /// Storage profile.
        profile: BrowserProfile,
    },
}

/// Storage profile for a browser surface.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BrowserProfile {
    /// Named persistent profile.
    Named {
        /// Profile directory name relative to the browser data root.
        name: String,
    },
    /// Temporary profile discarded on close.
    Incognito,
}

impl Default for BrowserProfile {
    fn default() -> Self {
        BrowserProfile::Named {
            name: "default".to_string(),
        }
    }
}

/// A pane's content plus its working directory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Surface {
    /// What this surface renders.
    pub kind: SurfaceKind,
    /// Working directory: live via OSC 7 for terminals, creation-time
    /// otherwise. `None` = unknown.
    pub cwd: Option<PathBuf>,
}
