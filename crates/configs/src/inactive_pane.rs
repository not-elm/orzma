//! Inactive-pane treatment configuration: per-pane brightness `dim` and
//! desaturation `desaturate` that the terminal renderer applies to every
//! pane that is not its workspace's active pane.

use serde::{Deserialize, Serialize};

/// Fully-resolved `[inactive_pane]` config block. The terminal renderer dims
/// (brightness) and desaturates (toward grey) every inactive pane.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct InactivePaneConfig {
    /// Whether inactive panes are treated (dimmed + desaturated) at all.
    pub enabled: bool,
    /// Inactive-pane brightness multiplier in `0.0..=1.0` (lower = dimmer);
    /// `1.0` leaves brightness untouched.
    pub dim: f32,
    /// Inactive-pane desaturation in `0.0..=1.0`: `0.0` keeps full color,
    /// `1.0` is fully grey. Applied in the terminal shader alongside `dim`.
    pub desaturate: f32,
}

impl Default for InactivePaneConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            dim: 0.5,
            desaturate: 0.7,
        }
    }
}

/// Per-field-optional patch produced by parsing `[inactive_pane]` from
/// `config.toml`. Missing keys fall through to the defaults.
#[derive(Deserialize, Default)]
pub(crate) struct InactivePaneConfigPatch {
    pub(crate) enabled: Option<bool>,
    pub(crate) dim: Option<f32>,
    pub(crate) desaturate: Option<f32>,
}

impl InactivePaneConfigPatch {
    pub(crate) fn apply_to(self, mut base: InactivePaneConfig) -> InactivePaneConfig {
        if let Some(v) = self.enabled {
            base.enabled = v;
        }
        if let Some(v) = self.dim
            && !v.is_nan()
        {
            base.dim = v.clamp(0.0, 1.0);
        }
        if let Some(v) = self.desaturate
            && !v.is_nan()
        {
            base.desaturate = v.clamp(0.0, 1.0);
        }
        base
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_expected_values() {
        let cfg = InactivePaneConfig::default();
        assert!(cfg.enabled);
        assert_eq!(cfg.dim, 0.5);
        assert_eq!(cfg.desaturate, 0.7);
    }

    #[test]
    fn patch_overrides_only_present_fields() {
        let patch = InactivePaneConfigPatch {
            desaturate: Some(0.3),
            ..Default::default()
        };
        let merged = patch.apply_to(InactivePaneConfig::default());
        assert_eq!(merged.desaturate, 0.3);
        assert!(merged.enabled);
        assert_eq!(merged.dim, 0.5);
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
    fn desaturate_clamps_into_unit_range() {
        let high = InactivePaneConfigPatch {
            desaturate: Some(4.0),
            ..Default::default()
        }
        .apply_to(InactivePaneConfig::default());
        assert_eq!(high.desaturate, 1.0);

        let low = InactivePaneConfigPatch {
            desaturate: Some(-1.0),
            ..Default::default()
        }
        .apply_to(InactivePaneConfig::default());
        assert_eq!(low.desaturate, 0.0);
    }

    #[test]
    fn nan_dim_is_rejected_and_keeps_base() {
        let patch: InactivePaneConfigPatch = toml::from_str("dim = nan").unwrap();
        let merged = patch.apply_to(InactivePaneConfig::default());
        assert_eq!(merged.dim, 0.5, "NaN dim must fall back to base");
        assert!(merged.dim.is_finite());
    }

    #[test]
    fn nan_desaturate_is_rejected_and_keeps_base() {
        let patch: InactivePaneConfigPatch = toml::from_str("desaturate = nan").unwrap();
        let merged = patch.apply_to(InactivePaneConfig::default());
        assert_eq!(
            merged.desaturate, 0.7,
            "NaN desaturate must fall back to base"
        );
        assert!(merged.desaturate.is_finite());
    }

    #[test]
    fn patch_parses_from_toml() {
        let patch: InactivePaneConfigPatch =
            toml::from_str("enabled = false\ndim = 0.3\ndesaturate = 0.9").unwrap();
        assert_eq!(patch.enabled, Some(false));
        assert_eq!(patch.dim, Some(0.3));
        assert_eq!(patch.desaturate, Some(0.9));
    }

    #[test]
    fn empty_patch_leaves_base_unchanged() {
        let patch = InactivePaneConfigPatch::default();
        let merged = patch.apply_to(InactivePaneConfig::default());
        assert_eq!(merged, InactivePaneConfig::default());
    }

    #[test]
    fn stale_veil_keys_are_ignored_without_error() {
        let patch: InactivePaneConfigPatch =
            toml::from_str("dim = 0.4\nopacity = 0.6\ncolor = \"#112233\"").unwrap();
        assert_eq!(patch.dim, Some(0.4));
        assert_eq!(patch.desaturate, None);
    }
}
