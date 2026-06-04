//! Inactive-pane dimming configuration: the translucent veil drawn over
//! every pane that is not its workspace's active pane.

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
    /// normalized to lowercase. `opacity`/`color` dim non-terminal panes
    /// (e.g. the webview); terminal panes use `dim`.
    pub color: String,
    /// Inactive-terminal brightness multiplier in `0.0..=1.0`. The terminal
    /// renderer multiplies an inactive pane's rendered content by this
    /// (lower = dimmer); `1.0` leaves it full-bright. Separate from `opacity`
    /// because a darkening veil is invisible on a black terminal background.
    pub dim: f32,
}

impl Default for InactivePaneConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            opacity: 0.5,
            color: "#000000".to_string(),
            dim: 0.5,
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
    pub(crate) dim: Option<f32>,
}

impl InactivePaneConfigPatch {
    pub(crate) fn apply_to(self, mut base: InactivePaneConfig) -> InactivePaneConfig {
        if let Some(v) = self.enabled {
            base.enabled = v;
        }
        if let Some(v) = self.opacity
            && !v.is_nan()
        {
            base.opacity = v.clamp(0.0, 1.0);
        }
        if let Some(v) = self.color
            && parse_hex_rgb(&v).is_some()
        {
            base.color = v.to_ascii_lowercase();
        }
        if let Some(v) = self.dim
            && !v.is_nan()
        {
            base.dim = v.clamp(0.0, 1.0);
        }
        base
    }
}

/// Parses a `#RRGGBB` hex string into `(r, g, b)` bytes; returns `None` for
/// any other shape (missing `#`, wrong length, non-hex digits).
fn parse_hex_rgb(s: &str) -> Option<(u8, u8, u8)> {
    let hex = s.strip_prefix('#')?;
    // NOTE: `!hex.is_ascii()` is load-bearing — the byte-index slices below
    // panic on a 6-BYTE non-ASCII string (e.g. "#中文") whose offsets 2/4
    // fall on a non-char-boundary. Drop this and config loading crashes.
    if hex.len() != 6 || !hex.is_ascii() {
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
        assert_eq!(cfg.opacity, 0.5);
        assert_eq!(cfg.color, "#000000");
        assert_eq!(cfg.rgb(), (0, 0, 0));
        assert_eq!(cfg.dim, 0.5);
    }

    #[test]
    fn rgb_parses_hex() {
        let cfg = InactivePaneConfig {
            enabled: true,
            opacity: 0.5,
            color: "#1a2b3c".to_string(),
            dim: 0.5,
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
            toml::from_str("enabled = false\nopacity = 0.6\ncolor = \"#112233\"\ndim = 0.3")
                .unwrap();
        assert_eq!(patch.enabled, Some(false));
        assert_eq!(patch.opacity, Some(0.6));
        assert_eq!(patch.color.as_deref(), Some("#112233"));
        assert_eq!(patch.dim, Some(0.3));
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

    #[test]
    fn non_ascii_six_byte_color_is_rejected_without_panic() {
        // "中文" is 6 UTF-8 bytes; byte-slicing it would panic at a
        // non-char-boundary if parse_hex_rgb did not guard on is_ascii.
        let merged = InactivePaneConfigPatch {
            color: Some("#中文".to_string()),
            ..Default::default()
        }
        .apply_to(InactivePaneConfig::default());
        assert_eq!(
            merged.color, "#000000",
            "non-ASCII color must fall back to base"
        );

        let cfg = InactivePaneConfig {
            enabled: true,
            opacity: 0.5,
            color: "#中文".to_string(),
            dim: 0.5,
        };
        assert_eq!(
            cfg.rgb(),
            (0, 0, 0),
            "rgb() must not panic on a non-ASCII color"
        );
    }

    #[test]
    fn nan_opacity_is_rejected_and_keeps_base() {
        let patch: InactivePaneConfigPatch = toml::from_str("opacity = nan").unwrap();
        let merged = patch.apply_to(InactivePaneConfig::default());
        assert_eq!(merged.opacity, 0.5, "NaN opacity must fall back to base");
        assert!(merged.opacity.is_finite(), "stored opacity must be finite");
    }

    #[test]
    fn infinite_opacity_clamps_to_unit_range() {
        let merged = InactivePaneConfigPatch {
            opacity: Some(f32::INFINITY),
            ..Default::default()
        }
        .apply_to(InactivePaneConfig::default());
        assert_eq!(merged.opacity, 1.0, "+inf opacity clamps to 1.0");
    }

    #[test]
    fn dim_clamps_into_unit_range() {
        let high = InactivePaneConfigPatch {
            dim: Some(4.0),
            ..Default::default()
        }
        .apply_to(InactivePaneConfig::default());
        assert_eq!(high.dim, 1.0);

        let low = InactivePaneConfigPatch {
            dim: Some(-1.0),
            ..Default::default()
        }
        .apply_to(InactivePaneConfig::default());
        assert_eq!(low.dim, 0.0);
    }

    #[test]
    fn nan_dim_is_rejected_and_keeps_base() {
        let patch: InactivePaneConfigPatch = toml::from_str("dim = nan").unwrap();
        let merged = patch.apply_to(InactivePaneConfig::default());
        assert_eq!(merged.dim, 0.5, "NaN dim must fall back to base");
        assert!(merged.dim.is_finite());
    }
}
