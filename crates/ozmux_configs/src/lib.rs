//! Config loader for ozmux. Reads `~/.config/ozmux/config.toml`
//! (or `$OZMUX_CONFIG` / `$XDG_CONFIG_HOME` overrides) and resolves it
//! against built-in defaults.

#![warn(missing_docs)]

use crate::font::FontConfig;
use crate::inactive_pane::InactivePaneConfig;
use crate::shortcuts::Shortcuts;
pub use error::{OzmuxConfigsError, OzmuxConfigsResult};
use serde::Deserialize;

pub mod copy_mode;
pub mod error;
pub mod font;
pub mod inactive_pane;
pub mod keyboard;
pub mod mouse;
pub mod ozma;
pub mod path;
pub mod shortcuts;

/// Fully-resolved ozmux configuration.
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default, deny_unknown_fields)]
pub struct OzmuxConfigs {
    /// Shortcut configuration.
    pub shortcuts: Shortcuts,
    /// `[copy-mode]` table: copy-mode key bindings shared by both modes.
    #[serde(rename = "copy-mode")]
    pub copy_mode: copy_mode::CopyModeConfig,
    /// Font configuration.
    pub font: FontConfig,
    /// Mouse-input configuration.
    pub mouse: mouse::MouseConfig,
    /// Keyboard-input configuration (macOS Option-as-Meta).
    pub keyboard: keyboard::KeyboardConfig,
    /// Inactive-pane dimming configuration.
    pub inactive_pane: InactivePaneConfig,
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
        self.shortcuts.normalize();
        self.inactive_pane.normalize();
        self.mouse.normalize();
    }

    fn validate(&self) -> OzmuxConfigsResult<()> {
        let sc = &self.shortcuts;
        if let Err(dupes) = sc.validate_no_direct_conflicts() {
            return Err(OzmuxConfigsError::DuplicateChords(dupes));
        }
        if let Err(dupes) = sc.validate_no_leader_conflicts() {
            return Err(OzmuxConfigsError::DuplicatePrefixChords(dupes));
        }
        if let Err(dupes) = self.copy_mode.validate_no_duplicate_keys() {
            return Err(OzmuxConfigsError::DuplicateCopyModeKeys(dupes));
        }
        if let Some(shortcuts::Leader::Chord(leader)) = sc.leader.as_ref() {
            if let Some((action, _, _)) = sc.direct_chords().find(|(_, chord, _)| *chord == leader)
            {
                return Err(OzmuxConfigsError::LeaderShadowsDirectBinding {
                    chord: leader.clone(),
                    action,
                });
            }
            if !leader.key.maps_to_physical_key() {
                return Err(OzmuxConfigsError::UnmappableLeader {
                    chord: leader.clone(),
                });
            }
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

    fn parse_validated(s: &str) -> OzmuxConfigsResult<OzmuxConfigs> {
        let mut c: OzmuxConfigs = toml::from_str(s).unwrap();
        c.normalize();
        c.validate().map(|()| c)
    }

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
        let toml_str = "[shortcuts]\nrelease-webview-focus = \"Cmd+Q\"\n";
        let mut configs: OzmuxConfigs = toml::from_str(toml_str).unwrap();
        configs.normalize();
        let err = configs.validate().unwrap_err();
        match err {
            OzmuxConfigsError::DuplicateChords(dupes) => {
                assert_eq!(dupes.len(), 1);
                assert!(dupes[0].actions.contains(&"quit"));
                assert!(dupes[0].actions.contains(&"release-webview-focus"));
            }
            _ => panic!("expected DuplicateChords, got {err:?}"),
        }
    }

    #[test]
    fn validate_rejects_leader_shadowing_direct_binding() {
        let toml_str = "[shortcuts]\nleader = \"Cmd+Q\"\ndetach-session = \"<Leader>d\"\n";
        let err = parse_validated(toml_str).unwrap_err();
        match err {
            OzmuxConfigsError::LeaderShadowsDirectBinding { action, .. } => {
                assert_eq!(action, "quit")
            }
            other => panic!("expected LeaderShadowsDirectBinding, got {other:?}"),
        }
    }

    #[test]
    fn validate_rejects_leader_table_internal_dupe() {
        let toml_str = "[shortcuts]\nleader = \"Ctrl+A\"\ndetach-session = \"<Leader>d\"\nenter-copy-mode = \"<Leader>d\"\n";
        let err = parse_validated(toml_str).unwrap_err();
        assert!(matches!(err, OzmuxConfigsError::DuplicatePrefixChords(_)));
    }

    #[test]
    fn validate_allows_cross_keyspace_same_key() {
        let toml_str = "[shortcuts]\nleader = \"Ctrl+A\"\nenter-copy-mode = \"s\"\ndetach-session = \"<Leader>s\"\n";
        assert!(
            parse_validated(toml_str).is_ok(),
            "direct `s` and leader-scoped `s` occupy different key-spaces"
        );
    }

    #[test]
    fn validate_allows_leader_with_bindings() {
        let toml_str = "[shortcuts]\nleader = \"Ctrl+A\"\ndetach-session = \"<Leader>d\"\n";
        assert!(parse_validated(toml_str).is_ok());
    }

    #[test]
    fn validate_rejects_unmappable_leader() {
        let toml_str = "[shortcuts]\nleader = \"Cmd+Plus\"\ndetach-session = \"<Leader>d\"\n";
        let err = parse_validated(toml_str).unwrap_err();
        assert!(matches!(err, OzmuxConfigsError::UnmappableLeader { .. }));
    }

    #[test]
    fn validate_accepts_mappable_leader() {
        let toml_str = "[shortcuts]\nleader = \"Ctrl+A\"\nenter-copy-mode = \"<Leader>s\"\n";
        assert!(parse_validated(toml_str).is_ok());
    }

    #[test]
    fn validate_accepts_bare_modifier_tap_leader() {
        let toml_str = "[shortcuts]\nleader = \"Cmd\"\ndetach-session = \"<Leader>d\"\n";
        assert!(parse_validated(toml_str).is_ok());
    }

    #[test]
    fn validate_tap_leader_coexists_with_cmd_direct_bindings() {
        let toml_str = "[shortcuts]\nleader = \"Cmd\"\n";
        assert!(parse_validated(toml_str).is_ok());
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
    fn empty_toml_returns_default_shortcuts() {
        let c = parse("");
        assert_eq!(c.shortcuts, OzmuxConfigs::default().shortcuts);
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
        let toml_str = "[shortcuts]\nresize-pane-down = \"Cmd+Shift+J\"\n";
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
        use shortcuts::Binding;
        let c = parse("[shortcuts]\nquit = \"Cmd+Shift+Q\"\n");
        let quit = c.shortcuts.quit.as_ref().unwrap().chord();
        assert_eq!(quit.key, shortcuts::Key::Char('q'));
        assert!(quit.modifiers.meta && quit.modifiers.shift);
        assert!(
            matches!(c.shortcuts.paste, Some(Binding::Leader(_))),
            "unspecified active bindings keep their defaults",
        );
    }

    #[test]
    fn user_unbind_with_empty_string_sets_field_to_none() {
        let c = parse("[shortcuts]\nquit = \"\"\n");
        assert!(c.shortcuts.quit.is_none());
    }
}
