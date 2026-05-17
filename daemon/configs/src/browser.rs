//! Browser activity configuration: search-engine template for the toolbar
//! URL bar's Chrome-style omnibox behavior.

use serde::{Deserialize, Serialize};

const DEFAULT_SEARCH_TEMPLATE: &str = "https://duckduckgo.com/?q={query}";

/// Fully-resolved browser configuration.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct BrowserConfig {
    /// Search-engine URL template. The literal `{query}` placeholder is
    /// substituted with the URL-encoded query at navigate time.
    pub search_template: String,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            search_template: DEFAULT_SEARCH_TEMPLATE.into(),
        }
    }
}

/// Per-field-optional view of `[browser]` for TOML deserialization.
#[derive(Deserialize, Default, Clone, Debug)]
pub(crate) struct BrowserPatch {
    /// Optional `[browser].search_template` override.
    pub search_template: Option<String>,
}

impl BrowserPatch {
    /// Applies any populated fields onto `base` and returns the merged result.
    pub fn apply_to(self, base: BrowserConfig) -> BrowserConfig {
        BrowserConfig {
            search_template: self.search_template.unwrap_or(base.search_template),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_uses_duckduckgo_template() {
        let c = BrowserConfig::default();
        assert_eq!(c.search_template, "https://duckduckgo.com/?q={query}");
    }

    #[test]
    fn empty_patch_returns_base() {
        let merged = BrowserPatch::default().apply_to(BrowserConfig::default());
        assert_eq!(merged, BrowserConfig::default());
    }

    #[test]
    fn template_override_applies() {
        let patch = BrowserPatch {
            search_template: Some("https://www.google.com/search?q={query}".into()),
        };
        let merged = patch.apply_to(BrowserConfig::default());
        assert_eq!(merged.search_template, "https://www.google.com/search?q={query}");
    }
}
