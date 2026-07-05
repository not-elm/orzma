//! Inactive-pane treatment configuration: a per-pane background tint toward a
//! configurable grey (`tint_color` at strength `tint`) plus an optional
//! brightness `dim`, applied by the terminal renderer to every pane that is
//! not its workspace's active pane.

use serde::{Deserialize, Serialize};

/// Fully-resolved `[inactive_pane]` config block. The terminal renderer blends
/// each inactive pane's background toward `tint_color` by `tint`, and multiplies
/// brightness by `dim`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
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
    /// `1.0` leaves brightness untouched. Applied to webview overlays
    /// only, so the background tint can stay background-only.
    pub webview_dim: f32,
    /// Inactive-webview desaturation in `0.0..=1.0`: `0.0` keeps full color,
    /// `1.0` is fully grey. Applied to webview overlays alongside
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

    /// Clamps unit-range fields to `0.0..=1.0` (NaN falls back to the default),
    /// validates `tint_color` as `#RRGGBB` (invalid falls back to the default),
    /// and lowercases a valid `tint_color`.
    pub(crate) fn normalize(&mut self) {
        let d = Self::default();
        self.dim = norm_unit(self.dim, d.dim);
        self.tint = norm_unit(self.tint, d.tint);
        self.webview_dim = norm_unit(self.webview_dim, d.webview_dim);
        self.webview_desaturate = norm_unit(self.webview_desaturate, d.webview_desaturate);
        if parse_hex_rgb(&self.tint_color).is_some() {
            self.tint_color = self.tint_color.to_ascii_lowercase();
        } else {
            self.tint_color = d.tint_color;
        }
    }
}

/// Returns `v` clamped to `0.0..=1.0`, or `default` when `v` is NaN.
fn norm_unit(v: f32, default: f32) -> f32 {
    if v.is_nan() {
        default
    } else {
        v.clamp(0.0, 1.0)
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

    fn from_toml_normalized(s: &str) -> InactivePaneConfig {
        let mut c: InactivePaneConfig = toml::from_str(s).unwrap();
        c.normalize();
        c
    }

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
    fn partial_fills_from_default_and_normalizes() {
        let cfg = from_toml_normalized("tint = 0.3");
        assert_eq!(cfg.tint, 0.3);
        assert!(cfg.enabled);
        assert_eq!(cfg.dim, 1.0);
        assert_eq!(cfg.tint_color, "#3a3b45");
    }

    #[test]
    fn unit_fields_clamp() {
        let cfg = from_toml_normalized("dim = 4.0\ntint = -1.0");
        assert_eq!(cfg.dim, 1.0);
        assert_eq!(cfg.tint, 0.0);
    }

    #[test]
    fn nan_unit_falls_back_to_default() {
        let cfg = from_toml_normalized("dim = nan\ntint = nan");
        assert_eq!(cfg.dim, 1.0);
        assert_eq!(cfg.tint, 0.85);
        assert!(cfg.dim.is_finite() && cfg.tint.is_finite());
    }

    #[test]
    fn invalid_tint_color_falls_back() {
        let cfg = from_toml_normalized(r#"tint_color = "not-a-color""#);
        assert_eq!(cfg.tint_color, "#3a3b45");
    }

    #[test]
    fn uppercase_tint_color_normalized() {
        let cfg = from_toml_normalized(r##"tint_color = "#FF00AB""##);
        assert_eq!(cfg.tint_color, "#ff00ab");
        assert_eq!(cfg.tint_color_rgb(), (0xff, 0x00, 0xab));
    }

    #[test]
    fn non_ascii_six_byte_tint_color_falls_back_without_panic() {
        let cfg = from_toml_normalized(r##"tint_color = "#中文""##);
        assert_eq!(cfg.tint_color, "#3a3b45");
        let bad = InactivePaneConfig {
            tint_color: "#中文".to_string(),
            ..Default::default()
        };
        assert_eq!(bad.tint_color_rgb(), (0, 0, 0));
    }

    #[test]
    fn stale_keys_ignored_without_error() {
        let cfg = from_toml_normalized("dim = 0.4\ndesaturate = 0.7\nopacity = 0.6");
        assert_eq!(cfg.dim, 0.4);
    }
}
