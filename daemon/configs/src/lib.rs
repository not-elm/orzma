//! Config loader for ozmux. Reads `~/.config/ozmux/config.toml`
//! (or `$OZMUX_CONFIG` / `$XDG_CONFIG_HOME` overrides) and resolves it
//! against built-in defaults.

#![warn(missing_docs)]

pub mod error;
pub mod shortcuts;
pub mod theme;

pub use error::{OzmuxConfigsError, OzmuxConfigsResult};
