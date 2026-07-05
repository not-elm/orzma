//! Configuration for the Orzma single-terminal mode.

use serde::Deserialize;

/// Resolved Orzma mode settings.
#[derive(Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct OrzmaConfig {
    /// Shell program to launch. `None` means "resolve at runtime via `$SHELL`".
    pub shell: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_shell_is_none() {
        assert!(OrzmaConfig::default().shell.is_none());
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
