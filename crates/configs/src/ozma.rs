//! Configuration for the Ozma single-terminal mode.

use serde::Deserialize;

/// Resolved Ozma mode settings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OzmaConfig {
    /// Shell program to launch. `None` means "resolve at runtime via `$SHELL`".
    pub shell: Option<String>,
}

/// Per-field-optional `[ozma]` view for TOML deserialization.
#[derive(Deserialize, Default, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct OzmaPatch {
    /// Optional shell override.
    pub shell: Option<String>,
}

impl OzmaPatch {
    /// Applies this patch over `base`, keeping `base`'s value where unset.
    pub(crate) fn apply_to(self, base: OzmaConfig) -> OzmaConfig {
        OzmaConfig {
            shell: self.shell.or(base.shell),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_shell_is_none() {
        assert!(OzmaConfig::default().shell.is_none());
    }

    #[test]
    fn patch_overrides_shell() {
        let patched = OzmaPatch {
            shell: Some("/bin/fish".to_string()),
        }
        .apply_to(OzmaConfig::default());
        assert_eq!(patched.shell.as_deref(), Some("/bin/fish"));
    }

    #[test]
    fn empty_patch_keeps_base() {
        let patched = OzmaPatch::default().apply_to(OzmaConfig::default());
        assert_eq!(patched, OzmaConfig::default());
    }

    #[test]
    fn ozma_section_parses_from_toml() {
        let toml_str = r#"
[ozma]
shell = "/usr/bin/zsh"
"#;
        let raw: crate::raw::RawConfigs = toml::from_str(toml_str).unwrap();
        let merged = raw.apply_to(crate::OzmuxConfigs::default());
        assert_eq!(merged.ozma.shell.as_deref(), Some("/usr/bin/zsh"));
    }

    #[test]
    fn missing_ozma_section_uses_defaults() {
        let raw: crate::raw::RawConfigs = toml::from_str("").unwrap();
        let merged = raw.apply_to(crate::OzmuxConfigs::default());
        assert!(merged.ozma.shell.is_none());
    }
}
