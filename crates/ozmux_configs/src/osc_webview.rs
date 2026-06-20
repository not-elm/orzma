//! Configuration for the OSC-driven webview feature (default-on gate).

use serde::{Deserialize, Serialize};

/// OSC-driven webview settings. Enabled by default: a foreground program's
/// `OSC 5379 ; mount ; <view-id>` mounts the registered view unless
/// `enabled = false`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(default)]
pub struct OscWebviewConfig {
    /// Master switch for the OSC-driven webview feature.
    pub enabled: bool,
}

impl Default for OscWebviewConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_enabled() {
        assert!(OscWebviewConfig::default().enabled);
    }

    #[test]
    fn empty_keeps_default_on() {
        let cfg: OscWebviewConfig = toml::from_str("").unwrap();
        assert!(cfg.enabled, "missing enabled defaults to true via impl Default");
    }

    #[test]
    fn explicit_false_overrides() {
        let cfg: OscWebviewConfig = toml::from_str("enabled = false").unwrap();
        assert!(!cfg.enabled);
    }
}
