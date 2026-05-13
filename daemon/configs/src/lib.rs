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

impl OzmuxConfigs {
    /// Loads the config from disk, merges it onto the built-in defaults, and
    /// validates the result.
    ///
    /// Returns `Default::default()` when the resolved path does not exist.
    /// Any other I/O failure, TOML parse error, or validation failure is
    /// surfaced as `OzmuxConfigsError`.
    pub async fn load() -> OzmuxConfigsResult<Self> {
        Self::load_with_env(&path::SystemEnv).await
    }

    pub(crate) async fn load_with_env(env: &dyn path::Env) -> OzmuxConfigsResult<Self> {
        let configured_path = path::resolve_config_path(env)?;
        tracing::info!(path = %configured_path.display(), "resolving ozmux config path");

        let text = match tokio::fs::read_to_string(&configured_path).await {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::info!(
                    path = %configured_path.display(),
                    "ozmux config not found; using defaults"
                );
                return Ok(Self::default());
            }
            Err(source) => {
                return Err(OzmuxConfigsError::Io {
                    path: configured_path,
                    source,
                });
            }
        };

        let raw: raw::RawConfigs = toml::from_str(&text).map_err(|source| {
            OzmuxConfigsError::ParseToml {
                path: configured_path.clone(),
                source,
            }
        })?;

        let merged = raw.apply_to(Self::default());
        raw::validate(&merged)?;
        Ok(merged)
    }
}
