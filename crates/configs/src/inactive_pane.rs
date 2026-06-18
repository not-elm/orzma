//! Inactive-pane treatment configuration: a per-pane background tint toward a
//! configurable grey (`tint_color` at strength `tint`) plus an optional
//! brightness `dim`, applied by the terminal renderer to every pane that is
//! not its workspace's active pane.

use serde::{Deserialize, Serialize};

/// Fully-resolved `[inactive_pane]` config block. The terminal renderer blends
/// each inactive pane's background toward `tint_color` by `tint`, and multiplies
/// brightness by `dim`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct InactivePaneConfig {
    /// Whether inactive panes are treated (tinted + dimmed) at all.
    pub enabled: bool,
    /// Inactive-pane brightness multiplier in `0.0..=1.0` (lower = dimmer);
    /// `1.0` leaves brightness untouched.
    pub dim: f32,
    /// Background-tint target color as a `#RRGGBB` hex string. Inactive-pane
    /// backgrounds blend toward this color by `tint`. Case-insensitive on
    /// input, stored normalized to lowercase.
    pub tint_color: String,
    /// Background-tint strength in `0.0..=1.0`: `0.0` keeps the real background,
    /// `1.0` replaces it with `tint_color`. Only the background is tinted; text
    /// and overlays are untouched.
    pub tint: f32,
    /// Inactive-webview brightness multiplier in `0.0..=1.0` (lower = darker);
    /// `1.0` leaves brightness untouched. Applied to inline-webview overlays
    /// only, so the background tint can stay background-only.
    pub webview_dim: f32,
    /// Inactive-webview desaturation in `0.0..=1.0`: `0.0` keeps full color,
    /// `1.0` is fully grey. Applied to inline-webview overlays alongside
    /// `webview_dim`.
    pub webview_desaturate: f32,
}

impl Default for InactivePaneConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            dim: 1.0,
            tint_color: "#3a3b45".to_string(),
            tint: 0.85,
            webview_dim: 0.55,
            webview_desaturate: 0.6,
        }
    }
}

impl InactivePaneConfig {
    /// Returns the tint target's `(r, g, b)` byte components parsed from the
    /// `#RRGGBB` `tint_color` string, falling back to black on a malformed
    /// value.
    pub fn tint_color_rgb(&self) -> (u8, u8, u8) {
        parse_hex_rgb(&self.tint_color).unwrap_or((0, 0, 0))
    }
}

/// Per-field-optional patch produced by parsing `[inactive_pane]` from
/// `config.toml`. Missing keys fall through to the defaults.
#[derive(Deserialize, Default)]
pub(crate) struct InactivePaneConfigPatch {
    pub(crate) enabled: Option<bool>,
    pub(crate) dim: Option<f32>,
    pub(crate) tint_color: Option<String>,
    pub(crate) tint: Option<f32>,
    pub(crate) webview_dim: Option<f32>,
    pub(crate) webview_desaturate: Option<f32>,
}

impl InactivePaneConfigPatch {
    pub(crate) fn apply_to(self, mut base: InactivePaneConfig) -> InactivePaneConfig {
        if let Some(v) = self.enabled {
            base.enabled = v;
        }
        apply_unit(&mut base.dim, self.dim);
        if let Some(v) = self.tint_color
            && parse_hex_rgb(&v).is_some()
        {
            base.tint_color = v.to_ascii_lowercase();
        }
        apply_unit(&mut base.tint, self.tint);
        apply_unit(&mut base.webview_dim, self.webview_dim);
        apply_unit(&mut base.webview_desaturate, self.webview_desaturate);
        base
    }
}

/// Overwrites `dst` with `src` clamped to `0.0..=1.0`. A `None` or `NaN` `src`
/// leaves `dst` unchanged (the base value is kept).
fn apply_unit(dst: &mut f32, src: Option<f32>) {
    if let Some(v) = src
        && !v.is_nan()
    {
        *dst = v.clamp(0.0, 1.0);
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
        assert_eq!(cfg.dim, 1.0);
        assert_eq!(cfg.tint_color, "#3a3b45");
        assert_eq!(cfg.tint, 0.85);
        assert_eq!(cfg.tint_color_rgb(), (0x3a, 0x3b, 0x45));
        assert_eq!(cfg.webview_dim, 0.55);
        assert_eq!(cfg.webview_desaturate, 0.6);
    }

    #[test]
    fn patch_overrides_only_present_fields() {
        let patch = InactivePaneConfigPatch {
            tint: Some(0.3),
            ..Default::default()
        };
        let merged = patch.apply_to(InactivePaneConfig::default());
        assert_eq!(merged.tint, 0.3);
        assert!(merged.enabled);
        assert_eq!(merged.dim, 1.0);
        assert_eq!(merged.tint_color, "#3a3b45");
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
    fn tint_clamps_into_unit_range() {
        let high = InactivePaneConfigPatch {
            tint: Some(4.0),
            ..Default::default()
        }
        .apply_to(InactivePaneConfig::default());
        assert_eq!(high.tint, 1.0);

        let low = InactivePaneConfigPatch {
            tint: Some(-1.0),
            ..Default::default()
        }
        .apply_to(InactivePaneConfig::default());
        assert_eq!(low.tint, 0.0);
    }

    #[test]
    fn nan_dim_is_rejected_and_keeps_base() {
        let patch: InactivePaneConfigPatch = toml::from_str("dim = nan").unwrap();
        let merged = patch.apply_to(InactivePaneConfig::default());
        assert_eq!(merged.dim, 1.0, "NaN dim must fall back to base");
        assert!(merged.dim.is_finite());
    }

    #[test]
    fn nan_tint_is_rejected_and_keeps_base() {
        let patch: InactivePaneConfigPatch = toml::from_str("tint = nan").unwrap();
        let merged = patch.apply_to(InactivePaneConfig::default());
        assert_eq!(merged.tint, 0.85, "NaN tint must fall back to base");
        assert!(merged.tint.is_finite());
    }

    #[test]
    fn webview_fields_clamp_into_unit_range() {
        let merged = InactivePaneConfigPatch {
            webview_dim: Some(4.0),
            webview_desaturate: Some(-1.0),
            ..Default::default()
        }
        .apply_to(InactivePaneConfig::default());
        assert_eq!(merged.webview_dim, 1.0);
        assert_eq!(merged.webview_desaturate, 0.0);
    }

    #[test]
    fn nan_webview_fields_are_rejected_and_keep_base() {
        let patch: InactivePaneConfigPatch =
            toml::from_str("webview_dim = nan\nwebview_desaturate = nan").unwrap();
        let merged = patch.apply_to(InactivePaneConfig::default());
        assert_eq!(merged.webview_dim, 0.55, "NaN webview_dim falls back");
        assert_eq!(
            merged.webview_desaturate, 0.6,
            "NaN webview_desaturate falls back"
        );
        assert!(merged.webview_dim.is_finite() && merged.webview_desaturate.is_finite());
    }

    #[test]
    fn invalid_tint_color_falls_back_to_base() {
        let merged = InactivePaneConfigPatch {
            tint_color: Some("not-a-color".to_string()),
            ..Default::default()
        }
        .apply_to(InactivePaneConfig::default());
        assert_eq!(merged.tint_color, "#3a3b45");
    }

    #[test]
    fn uppercase_tint_color_is_normalized_and_parsed() {
        let merged = InactivePaneConfigPatch {
            tint_color: Some("#FF00AB".to_string()),
            ..Default::default()
        }
        .apply_to(InactivePaneConfig::default());
        assert_eq!(merged.tint_color, "#ff00ab");
        assert_eq!(merged.tint_color_rgb(), (0xff, 0x00, 0xab));
    }

    #[test]
    fn non_ascii_six_byte_tint_color_is_rejected_without_panic() {
        // "中文" is 6 UTF-8 bytes; byte-slicing it would panic at a
        // non-char-boundary if parse_hex_rgb did not guard on is_ascii.
        let merged = InactivePaneConfigPatch {
            tint_color: Some("#中文".to_string()),
            ..Default::default()
        }
        .apply_to(InactivePaneConfig::default());
        assert_eq!(merged.tint_color, "#3a3b45", "non-ASCII must fall back");

        let cfg = InactivePaneConfig {
            tint_color: "#中文".to_string(),
            ..Default::default()
        };
        assert_eq!(cfg.tint_color_rgb(), (0, 0, 0), "rgb() must not panic");
    }

    #[test]
    fn patch_parses_from_toml() {
        let patch: InactivePaneConfigPatch = toml::from_str(
            "enabled = false\ndim = 0.3\ntint_color = \"#112233\"\ntint = 0.9\nwebview_dim = 0.4\nwebview_desaturate = 0.7",
        )
        .unwrap();
        assert_eq!(patch.enabled, Some(false));
        assert_eq!(patch.dim, Some(0.3));
        assert_eq!(patch.tint_color.as_deref(), Some("#112233"));
        assert_eq!(patch.tint, Some(0.9));
        assert_eq!(patch.webview_dim, Some(0.4));
        assert_eq!(patch.webview_desaturate, Some(0.7));
    }

    #[test]
    fn empty_patch_leaves_base_unchanged() {
        let patch = InactivePaneConfigPatch::default();
        let merged = patch.apply_to(InactivePaneConfig::default());
        assert_eq!(merged, InactivePaneConfig::default());
    }

    #[test]
    fn stale_keys_are_ignored_without_error() {
        let patch: InactivePaneConfigPatch =
            toml::from_str("dim = 0.4\ndesaturate = 0.7\nopacity = 0.6").unwrap();
        assert_eq!(patch.dim, Some(0.4));
        assert_eq!(patch.tint, None);
    }
}
