//! Inactive-pane dimming configuration: the translucent veil drawn over
//! every pane that is not its session's active pane.

use serde::{Deserialize, Serialize};

/// Fully-resolved `[inactive_pane]` config block. The UI layer draws a
/// veil over each inactive pane using `color` (RGB) at `opacity` (alpha).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct InactivePaneConfig {
    /// Whether inactive panes are dimmed at all.
    pub enabled: bool,
    /// Veil alpha in `0.0..=1.0`. Higher = darker inactive panes.
    pub opacity: f32,
    /// Veil color as a `#RRGGBB` hex string. Alpha comes from `opacity`,
    /// not from this string. Hex is case-insensitive on input and stored
    /// normalized to lowercase.
    pub color: String,
}

impl Default for InactivePaneConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            opacity: 0.45,
            color: "#000000".to_string(),
        }
    }
}

impl InactivePaneConfig {
    /// Returns the veil color's `(r, g, b)` byte components parsed from the
    /// `#RRGGBB` `color` string, falling back to black on a malformed value.
    pub fn rgb(&self) -> (u8, u8, u8) {
        parse_hex_rgb(&self.color).unwrap_or((0, 0, 0))
    }
}

/// Per-field-optional patch produced by parsing `[inactive_pane]` from
/// `config.toml`. Missing keys fall through to the defaults.
#[derive(Deserialize, Default)]
pub(crate) struct InactivePaneConfigPatch {
    pub(crate) enabled: Option<bool>,
    pub(crate) opacity: Option<f32>,
    pub(crate) color: Option<String>,
}

impl InactivePaneConfigPatch {
    pub(crate) fn apply_to(self, mut base: InactivePaneConfig) -> InactivePaneConfig {
        if let Some(v) = self.enabled {
            base.enabled = v;
        }
        if let Some(v) = self.opacity {
            base.opacity = v.clamp(0.0, 1.0);
        }
        if let Some(v) = self.color
            && parse_hex_rgb(&v).is_some()
        {
            base.color = v.to_ascii_lowercase();
        }
        base
    }
}

/// Parses a `#RRGGBB` hex string into `(r, g, b)` bytes; returns `None` for
/// any other shape (missing `#`, wrong length, non-hex digits).
fn parse_hex_rgb(s: &str) -> Option<(u8, u8, u8)> {
    let hex = s.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some((r, g, b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_expected_values() {
        let cfg = InactivePaneConfig::default();
        assert!(cfg.enabled);
        assert_eq!(cfg.opacity, 0.45);
        assert_eq!(cfg.color, "#000000");
        assert_eq!(cfg.rgb(), (0, 0, 0));
    }

    #[test]
    fn rgb_parses_hex() {
        let cfg = InactivePaneConfig {
            enabled: true,
            opacity: 0.5,
            color: "#1a2b3c".to_string(),
        };
        assert_eq!(cfg.rgb(), (0x1a, 0x2b, 0x3c));
    }

    #[test]
    fn patch_overrides_only_present_fields() {
        let patch = InactivePaneConfigPatch {
            opacity: Some(0.8),
            ..Default::default()
        };
        let merged = patch.apply_to(InactivePaneConfig::default());
        assert_eq!(merged.opacity, 0.8);
        assert!(merged.enabled);
        assert_eq!(merged.color, "#000000");
    }

    #[test]
    fn opacity_clamps_into_unit_range() {
        let high = InactivePaneConfigPatch {
            opacity: Some(5.0),
            ..Default::default()
        }
        .apply_to(InactivePaneConfig::default());
        assert_eq!(high.opacity, 1.0);

        let low = InactivePaneConfigPatch {
            opacity: Some(-2.0),
            ..Default::default()
        }
        .apply_to(InactivePaneConfig::default());
        assert_eq!(low.opacity, 0.0);
    }

    #[test]
    fn invalid_color_falls_back_to_base() {
        let merged = InactivePaneConfigPatch {
            color: Some("not-a-color".to_string()),
            ..Default::default()
        }
        .apply_to(InactivePaneConfig::default());
        assert_eq!(merged.color, "#000000");
    }

    #[test]
    fn patch_parses_from_toml() {
        let patch: InactivePaneConfigPatch =
            toml::from_str("enabled = false\nopacity = 0.6\ncolor = \"#112233\"").unwrap();
        assert_eq!(patch.enabled, Some(false));
        assert_eq!(patch.opacity, Some(0.6));
        assert_eq!(patch.color.as_deref(), Some("#112233"));
    }

    #[test]
    fn uppercase_color_is_normalized_and_parsed() {
        let merged = InactivePaneConfigPatch {
            color: Some("#FF00AB".to_string()),
            ..Default::default()
        }
        .apply_to(InactivePaneConfig::default());
        assert_eq!(merged.color, "#ff00ab");
        assert_eq!(merged.rgb(), (0xff, 0x00, 0xab));
    }

    #[test]
    fn empty_patch_leaves_base_unchanged() {
        let patch = InactivePaneConfigPatch::default();
        let merged = patch.apply_to(InactivePaneConfig::default());
        assert_eq!(merged, InactivePaneConfig::default());
    }
}
