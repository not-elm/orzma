//! Configuration for the Orzma single-terminal mode.

use serde::Deserialize;

/// Resolved Orzma mode settings.
#[derive(Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct OrzmaConfig {
    /// Shell program to launch. `None` means "resolve at runtime via `$SHELL`".
    pub shell: Option<String>,
    /// Whether orzma may make a shell it recognizes report its working
    /// directory, so a split starts where the shell last was. Has no
    /// effect outside Windows.
    pub shell_integration: bool,
}

impl Default for OrzmaConfig {
    fn default() -> Self {
        Self {
            shell: None,
            shell_integration: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts that a default config launches no particular shell and
    /// leaves the working-directory integration on.
    ///
    /// Case: a user who has never written a config file starts orzma for
    /// the first time.
    #[test]
    fn the_default_leaves_the_shell_unset_and_the_integration_on() {
        let cfg = OrzmaConfig::default();
        assert!(cfg.shell.is_none());
        assert!(cfg.shell_integration);
    }

    /// Asserts that a config file omitting the integration key leaves it
    /// on, matching the struct's own default.
    ///
    /// Case: a user who set only `shell` in their config file upgrades to
    /// a build that added the integration.
    #[test]
    fn an_omitted_integration_key_stays_on() {
        let cfg: OrzmaConfig = toml::from_str(r#"shell = "/bin/fish""#).unwrap();
        assert!(cfg.shell_integration);
    }

    /// Asserts that the integration can be turned off from the config
    /// file.
    ///
    /// Case: a user whose prompt setup conflicts with the injected hook
    /// opts out.
    #[test]
    fn the_integration_can_be_turned_off() {
        let cfg: OrzmaConfig = toml::from_str("shell_integration = false").unwrap();
        assert!(!cfg.shell_integration);
    }

    #[test]
    fn parses_shell() {
        let cfg: OrzmaConfig = toml::from_str(r#"shell = "/bin/fish""#).unwrap();
        assert_eq!(cfg.shell.as_deref(), Some("/bin/fish"));
    }

    #[test]
    fn empty_is_default() {
        let cfg: OrzmaConfig = toml::from_str("").unwrap();
        assert_eq!(cfg, OrzmaConfig::default());
    }

    #[test]
    fn rejects_unknown_field() {
        assert!(toml::from_str::<OrzmaConfig>(r#"shel = "/bin/fish""#).is_err());
    }
}
