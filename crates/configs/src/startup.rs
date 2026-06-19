//! Startup mode: which application mode launches on boot.

use serde::Deserialize;

/// Determines which mode the application enters on launch.
#[derive(Deserialize, Default, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum StartupMode {
    /// Single PTY terminal, no tmux (default).
    #[default]
    Ozma,
    /// Show the tmux session picker.
    Ozmux,
    /// Auto-attach to the most recently active tmux session.
    AutoAttach,
}
