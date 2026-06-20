//! Config loader for ozmux. Reads `~/.config/ozmux/config.toml`
//! (or `$OZMUX_CONFIG` / `$XDG_CONFIG_HOME` overrides) and resolves it
//! against built-in defaults.

#![warn(missing_docs)]

use crate::font::FontConfig;
use crate::inactive_pane::InactivePaneConfig;
use crate::osc_webview::OscWebviewConfig;
use crate::shortcuts::Shortcuts;
use crate::theme::Theme;
pub use error::{OzmuxConfigsError, OzmuxConfigsResult};
use std::path::Path;

pub mod error;
pub mod font;
pub mod inactive_pane;
pub mod keyboard;
pub mod mouse;
pub mod osc_webview;
pub mod ozma;
pub mod path;
mod raw;
pub mod shortcuts;
pub mod startup;
pub mod theme;
pub mod tmux;
pub use startup::StartupMode;

/// Fully-resolved ozmux configuration.
#[derive(Clone, Debug, Default)]
pub struct OzmuxConfigs {
    /// Shortcut configuration.
    pub shortcuts: Shortcuts,
    /// Theme configuration.
    pub theme: Theme,
    /// Font configuration.
    pub font: FontConfig,
    /// Mouse-input configuration.
    pub mouse: mouse::MouseConfig,
    /// Keyboard-input configuration (macOS Option-as-Meta).
    pub keyboard: keyboard::KeyboardConfig,
    /// Inactive-pane dimming configuration.
    pub inactive_pane: InactivePaneConfig,
    /// OSC-driven webview configuration.
    pub osc_webview: OscWebviewConfig,
    /// tmux backend configuration.
    pub tmux: tmux::TmuxConfig,
    /// Ozma single-terminal mode configuration.
    pub ozma: ozma::OzmaConfig,
    /// Startup mode: which application mode launches on boot.
    pub startup_mode: StartupMode,
}

impl OzmuxConfigs {
    /// Loads the config from disk, merges it onto the built-in defaults, and
    /// validates the result.
    ///
    /// Returns `Default::default()` when the resolved path does not exist.
    /// Any other I/O failure, TOML parse error, or validation failure is
    /// surfaced as `OzmuxConfigsError`.
    pub fn load() -> OzmuxConfigsResult<Self> {
        Self::load_with_env(&path::SystemEnv)
    }

    fn load_with_env(env: &dyn path::Env) -> OzmuxConfigsResult<Self> {
        let configured_path = path::resolve_config_path(env)?;
        tracing::info!(path = %configured_path.display(), "resolving ozmux config path");

        let text = match std::fs::read_to_string(&configured_path) {
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

        Self::parse_and_validate(&text, &configured_path)
    }

    fn parse_and_validate(text: &str, path: &Path) -> OzmuxConfigsResult<Self> {
        let raw: raw::RawConfigs =
            toml::from_str(text).map_err(|source| OzmuxConfigsError::ParseToml {
                path: path.to_path_buf(),
                source,
            })?;
        let merged = raw.apply_to(Self::default());
        merged.validate()?;
        Ok(merged)
    }

    fn validate(&self) -> OzmuxConfigsResult<()> {
        if let Err(dupes) = self.shortcuts.bindings.validate_no_conflicts() {
            return Err(OzmuxConfigsError::DuplicateChords(dupes));
        }
        let size = self.font.size;
        if !(size > 0.0 && size <= 200.0) {
            return Err(OzmuxConfigsError::InvalidFontSize { size });
        }
        Ok(())
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
    pub fn load_with_overrides(
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
        OzmuxConfigs::load_with_env(&env)
    }
}

#[cfg(test)]
mod validate_tests {
    use super::*;

    #[test]
    fn validate_rejects_font_size_out_of_range() {
        let mut configs = OzmuxConfigs::default();
        configs.font.size = 0.0;
        assert!(configs.validate().is_err(), "size 0.0 must fail validation");
        configs.font.size = 11.25;
        assert!(configs.validate().is_ok(), "in-range size validates");
    }

    #[test]
    fn validate_detects_chord_conflict() {
        let toml_str = r#"
[shortcuts.bindings]
release-inline-focus = "Cmd+V"
"#;
        let raw: raw::RawConfigs = toml::from_str(toml_str).unwrap();
        let merged = raw.apply_to(OzmuxConfigs::default());
        let err = merged.validate().unwrap_err();
        match err {
            OzmuxConfigsError::DuplicateChords(dupes) => {
                assert_eq!(dupes.len(), 1);
                assert!(dupes[0].actions.contains(&"paste"));
                assert!(dupes[0].actions.contains(&"release-inline-focus"));
            }
            _ => panic!("expected DuplicateChords, got {err:?}"),
        }
    }
}

#[cfg(test)]
mod mouse_integration_tests {
    use super::*;

    #[test]
    fn parses_full_mouse_section() {
        let toml_input = r#"
[mouse]
lines_per_notch = 5
fine_modifier = "ctrl"
fine_lines = 2
max_protocol_events_per_frame = 16
"#;
        let raw: raw::RawConfigs = toml::from_str(toml_input).unwrap();
        let merged = raw.apply_to(OzmuxConfigs::default());
        assert_eq!(merged.mouse.lines_per_notch, 5);
        assert_eq!(merged.mouse.fine_modifier, mouse::FineModifier::Ctrl);
        assert_eq!(merged.mouse.fine_lines, 2);
        assert_eq!(merged.mouse.max_protocol_events_per_frame, 16);
    }

    #[test]
    fn missing_mouse_section_uses_defaults() {
        let toml_input = "";
        let raw: raw::RawConfigs = toml::from_str(toml_input).unwrap();
        let merged = raw.apply_to(OzmuxConfigs::default());
        assert_eq!(merged.mouse, mouse::MouseConfig::default());
    }
}
