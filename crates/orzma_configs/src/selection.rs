//! Selection configuration: the `[selection]` section.

use serde::Deserialize;

/// Fully-resolved `[selection]` config block.
#[derive(Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct SelectionConfig {
    semantic_escape_chars: Option<String>,
}

impl SelectionConfig {
    /// Builds the section; `None` leaves the built-in word separators in
    /// force.
    pub fn new(semantic_escape_chars: Option<String>) -> Self {
        Self {
            semantic_escape_chars,
        }
    }

    /// The characters that end a word for the semantic vi-mode motions,
    /// besides whitespace; `None` when the built-in set stays in force.
    pub fn semantic_escape_chars(&self) -> Option<&str> {
        self.semantic_escape_chars.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn from_toml(s: &str) -> SelectionConfig {
        toml::from_str(s).expect("the fixture parses")
    }

    /// Asserts that an empty section leaves the built-in word separators
    /// in force.
    ///
    /// Case: the user has never written a `[selection]` section.
    #[test]
    fn an_empty_section_keeps_the_built_in_separators() {
        assert_eq!(from_toml("").semantic_escape_chars(), None);
    }

    /// Asserts that a configured separator string is read back verbatim.
    ///
    /// Case: the user makes `-` and `/` end a word so `w` stops inside
    /// paths.
    #[test]
    fn a_configured_separator_string_is_read_verbatim() {
        let config = from_toml(r#"semantic_escape_chars = "-/""#);
        assert_eq!(config.semantic_escape_chars(), Some("-/"));
    }

    /// Asserts that an unknown key in the section is a parse error.
    ///
    /// Case: the user writes the key with dashes instead of underscores.
    #[test]
    fn an_unknown_key_is_rejected() {
        assert!(toml::from_str::<SelectionConfig>(r#"semantic-escape-chars = "-""#).is_err());
    }

    /// Asserts that `new` builds the section it describes.
    ///
    /// Case: a caller builds the section without a config file.
    #[test]
    fn new_builds_the_described_section() {
        let config = SelectionConfig::new(Some("-".to_string()));
        assert_eq!(config.semantic_escape_chars(), Some("-"));
    }
}
