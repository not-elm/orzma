//! Config loader for ozmux. Reads `~/.config/ozmux/config.toml`
//! (or `$OZMUX_CONFIG` / `$XDG_CONFIG_HOME` overrides) and resolves it
//! against built-in defaults.

#![warn(missing_docs)]

use crate::shortcuts::Shortcuts;
use crate::theme::Theme;
pub use error::{OzmuxConfigsError, OzmuxConfigsResult};

pub mod error;
mod path;
mod raw;
pub mod shortcuts;
pub mod theme;

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

    async fn load_with_env(env: &dyn path::Env) -> OzmuxConfigsResult<Self> {
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

        let raw: raw::RawConfigs =
            toml::from_str(&text).map_err(move |source| OzmuxConfigsError::ParseToml {
                path: configured_path,
                source,
            })?;

        let merged = raw.apply_to(Self::default());
        raw::validate(&merged)?;
        Ok(merged)
    }
}

#[cfg(feature = "test_support")]
pub mod test_support {
    //! Test-only helpers. Enabled via the `test_support` cargo feature.

    use crate::OzmuxConfigs;
    use crate::OzmuxConfigsResult;
    use crate::path;
    use std::path::PathBuf;

    /// Loads [`OzmuxConfigs`] against a caller-controlled environment instead
    /// of the process-wide one.
    pub async fn load_with_overrides(
        ozmux_config: Option<PathBuf>,
        xdg_config_home: Option<PathBuf>,
        home_dir: Option<PathBuf>,
    ) -> OzmuxConfigsResult<OzmuxConfigs> {
        struct FixedEnv {
            ozmux: Option<String>,
            xdg: Option<String>,
            home: Option<PathBuf>,
        }
        impl path::Env for FixedEnv {
            fn var(&self, key: &str) -> Option<String> {
                match key {
                    path::ENV_OZMUX_CONFIG => self.ozmux.clone(),
                    path::ENV_XDG_CONFIG_HOME => self.xdg.clone(),
                    _ => None,
                }
            }
            fn home_dir(&self) -> Option<PathBuf> {
                self.home.clone()
            }
        }
        let env = FixedEnv {
            ozmux: ozmux_config.map(|p| p.to_string_lossy().into_owned()),
            xdg: xdg_config_home.map(|p| p.to_string_lossy().into_owned()),
            home: home_dir,
        };
        OzmuxConfigs::load_with_env(&env).await
    }
}
