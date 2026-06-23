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
use serde::Deserialize;

pub mod error;
pub mod font;
pub mod inactive_pane;
pub mod keyboard;
pub mod mouse;
pub mod osc_webview;
pub mod ozma;
pub mod path;
pub mod shortcuts;
pub mod theme;
pub mod tmux;

/// Fully-resolved ozmux configuration.
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default, deny_unknown_fields)]
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

        let mut configs: OzmuxConfigs =
            toml::from_str(&text).map_err(|source| OzmuxConfigsError::ParseToml {
                path: configured_path.clone(),
                source,
            })?;
        configs.normalize();
        configs.validate()?;
        Ok(configs)
    }

    fn normalize(&mut self) {
        self.inactive_pane.normalize();
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
release-webview-focus = "Cmd+V"
"#;
        let mut configs: OzmuxConfigs = toml::from_str(toml_str).unwrap();
        configs.normalize();
        let err = configs.validate().unwrap_err();
        match err {
            OzmuxConfigsError::DuplicateChords(dupes) => {
                assert_eq!(dupes.len(), 1);
                assert!(dupes[0].actions.contains(&"paste"));
                assert!(dupes[0].actions.contains(&"release-webview-focus"));
            }
            _ => panic!("expected DuplicateChords, got {err:?}"),
        }
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;

    fn parse(s: &str) -> OzmuxConfigs {
        let mut c: OzmuxConfigs = toml::from_str(s).unwrap();
        c.normalize();
        c
    }

    #[test]
    fn empty_toml_is_all_defaults() {
        assert_eq!(parse("").font, OzmuxConfigs::default().font);
        assert_eq!(parse("").mouse, OzmuxConfigs::default().mouse);
    }

    #[test]
    fn empty_toml_returns_default_bindings() {
        let c = parse("");
        assert_eq!(
            c.shortcuts.bindings,
            OzmuxConfigs::default().shortcuts.bindings
        );
    }

    #[test]
    fn unknown_top_level_section_is_rejected() {
        assert!(
            toml::from_str::<OzmuxConfigs>("[shortucts]\n").is_err(),
            "a misspelled section name must error under top-level deny_unknown_fields"
        );
    }

    #[test]
    fn unknown_binding_field_is_rejected() {
        let toml_str = r#"
[shortcuts.bindings]
resize-pane-down = "Cmd+Shift+J"
"#;
        let err = toml::from_str::<OzmuxConfigs>(toml_str).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("resize-pane-down") || msg.contains("unknown field"),
            "error message should mention the unknown field; got: {msg}"
        );
    }

    #[test]
    fn old_nested_font_table_fails_at_top_level() {
        assert!(
            toml::from_str::<OzmuxConfigs>("[font.normal]\npath = \"/x.ttf\"").is_err(),
            "old nested font schema must fail to load through OzmuxConfigs, not be shimmed"
        );
    }

    #[test]
    fn inactive_pane_section_merges_and_clamps() {
        let c = parse("[inactive_pane]\nenabled = false\ntint = 2.0\ntint_color = \"#102030\"\n");
        assert!(!c.inactive_pane.enabled);
        assert_eq!(c.inactive_pane.tint, 1.0, "tint clamps to 1.0");
        assert_eq!(c.inactive_pane.tint_color, "#102030");
        assert_eq!(c.inactive_pane.dim, 1.0, "dim keeps default");
    }

    #[test]
    fn missing_inactive_pane_section_uses_defaults() {
        let c = parse("");
        assert_eq!(c.inactive_pane, InactivePaneConfig::default());
    }

    #[test]
    fn inactive_pane_is_normalized_through_pipeline() {
        let c = parse("[inactive_pane]\ndim = 4.0\ntint_color = \"#FF00AB\"");
        assert_eq!(c.inactive_pane.dim, 1.0);
        assert_eq!(c.inactive_pane.tint_color, "#ff00ab");
    }

    #[test]
    fn osc_webview_setting_merges_from_toml() {
        let disabled = parse("[osc_webview]\nenabled = false\n");
        assert!(
            !disabled.osc_webview.enabled,
            "[osc_webview] enabled = false must flip the default-on gate off"
        );
        let defaulted = parse("");
        assert!(
            defaulted.osc_webview.enabled,
            "empty TOML must keep the default-on gate"
        );
    }

    #[test]
    fn empty_tmux_section_is_accepted() {
        let c = parse("[tmux]\n");
        assert_eq!(c.tmux, tmux::TmuxConfig::default());
    }

    #[test]
    fn missing_tmux_section_uses_defaults() {
        let c = parse("");
        assert_eq!(c.tmux, tmux::TmuxConfig::default());
    }

    #[test]
    fn keyboard_section_merges_from_toml() {
        let c = parse("[keyboard]\noption_as_alt = \"both\"\n");
        assert_eq!(c.keyboard.option_as_alt, keyboard::OptionAsAlt::Both);
    }

    #[test]
    fn missing_keyboard_section_uses_defaults() {
        let c = parse("");
        assert_eq!(c.keyboard, keyboard::KeyboardConfig::default());
    }

    #[test]
    fn ozma_section_parses_from_toml() {
        let c = parse("[ozma]\nshell = \"/usr/bin/zsh\"\n");
        assert_eq!(c.ozma.shell.as_deref(), Some("/usr/bin/zsh"));
    }

    #[test]
    fn missing_ozma_section_uses_defaults() {
        let c = parse("");
        assert!(c.ozma.shell.is_none());
    }

    #[test]
    fn parses_full_mouse_section() {
        let c = parse(
            "[mouse]\nlines_per_notch = 5\nfine_modifier = \"ctrl\"\nfine_lines = 2\nmax_protocol_events_per_frame = 16\n",
        );
        assert_eq!(c.mouse.lines_per_notch, 5);
        assert_eq!(c.mouse.fine_modifier, mouse::FineModifier::Ctrl);
        assert_eq!(c.mouse.fine_lines, 2);
        assert_eq!(c.mouse.max_protocol_events_per_frame, 16);
    }

    #[test]
    fn missing_mouse_section_uses_defaults() {
        let c = parse("");
        assert_eq!(c.mouse, mouse::MouseConfig::default());
    }

    #[test]
    fn user_override_replaces_one_binding_keeps_others() {
        let c = parse("[shortcuts.bindings]\nquit = \"Cmd+Shift+Q\"\n");
        let quit = c.shortcuts.bindings.quit.as_ref().unwrap();
        assert_eq!(quit.key, shortcuts::Key::Char('q'));
        assert!(quit.modifiers.meta && quit.modifiers.shift);
        assert!(
            c.shortcuts.bindings.paste.is_some(),
            "unspecified active bindings keep their defaults",
        );
    }

    #[test]
    fn user_unbind_with_empty_string_sets_field_to_none() {
        let c = parse("[shortcuts.bindings]\nquit = \"\"\n");
        assert!(c.shortcuts.bindings.quit.is_none());
    }
}
