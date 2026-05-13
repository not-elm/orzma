//! Config loader for ozmux. Reads `~/.config/ozmux/config.toml`
//! (or `$OZMUX_CONFIG` / `$XDG_CONFIG_HOME` overrides) and resolves it
//! against built-in defaults.

#![warn(missing_docs)]

pub mod error;
pub mod shortcuts;
pub mod theme;
mod defaults;
pub(crate) mod path;
pub(crate) mod raw;

pub use error::{OzmuxConfigsError, OzmuxConfigsResult};

use crate::shortcuts::Shortcuts;
use crate::theme::Theme;

/// Fully-resolved ozmux configuration.
#[derive(Clone, Debug, Default)]
pub struct OzmuxConfigs {
    /// Shortcut configuration.
    pub shortcuts: Shortcuts,
    /// Theme configuration.
    pub theme: Theme,
}
