//! Configuration for the OSC-driven webview feature (default-off gate).

use serde::{Deserialize, Serialize};

/// OSC-driven webview settings. Disabled by default: a foreground program's
/// `OSC 5379 ; mount ; <view-id>` is dropped unless `enabled = true`.
#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq, Eq)]
pub struct OscWebviewConfig {
    /// Master switch for the OSC-driven webview feature.
    pub enabled: bool,
}

/// Per-field-optional view of `[osc_webview]` for TOML deserialization.
#[derive(Deserialize, Default, Clone, Debug)]
pub(crate) struct OscWebviewPatch {
    /// Optional `[osc_webview].enabled` override.
    pub enabled: Option<bool>,
}

impl OscWebviewPatch {
    /// Applies this patch over `base`, keeping `base`'s value where unset.
    pub fn apply_to(self, base: OscWebviewConfig) -> OscWebviewConfig {
        OscWebviewConfig {
            enabled: self.enabled.unwrap_or(base.enabled),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_disabled() {
        assert!(!OscWebviewConfig::default().enabled);
    }

    #[test]
    fn patch_overrides_when_present() {
        let patched =
            OscWebviewPatch { enabled: Some(true) }.apply_to(OscWebviewConfig::default());
        assert!(patched.enabled);
    }
}
