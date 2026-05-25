//! Mouse-input configuration (currently: wheel scroll behavior).

use serde::{Deserialize, Serialize};

/// Which modifier triggers "fine" scrolling (1 line per notch instead
/// of `lines_per_notch`).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum FineModifier {
    /// Shift key activates fine scrolling.
    #[default]
    Shift,
    /// Ctrl key activates fine scrolling.
    Ctrl,
    /// Alt key activates fine scrolling.
    Alt,
    /// No modifier required; fine scrolling is always active.
    None,
}

/// Fully-resolved `[mouse]` config block. Consumed by the Bevy
/// mouse-wheel input system; mapped 1:1 to `bevy_terminal::WheelConfig`
/// for the routing layer.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct MouseConfig {
    /// Lines scrolled per notch in the scrollback / alt-screen paths.
    pub lines_per_notch: u32,
    /// Which modifier key activates fine scrolling.
    pub fine_modifier: FineModifier,
    /// Lines scrolled per notch when the fine modifier is held.
    pub fine_lines: u32,
    /// Upper bound on mouse-protocol events emitted per frame —
    /// protects the PTY from input bursts when the user spins the
    /// wheel rapidly while an app has SGR mouse tracking enabled.
    pub max_protocol_events_per_frame: u32,
}

impl Default for MouseConfig {
    fn default() -> Self {
        Self {
            lines_per_notch: 3,
            fine_modifier: FineModifier::Shift,
            fine_lines: 1,
            max_protocol_events_per_frame: 8,
        }
    }
}

/// Per-field-optional patch produced by parsing `[mouse]` from
/// `config.toml`. Missing keys fall through to the defaults.
#[derive(Deserialize, Default)]
pub(crate) struct MousePatch {
    pub(crate) lines_per_notch: Option<u32>,
    pub(crate) fine_modifier: Option<FineModifier>,
    pub(crate) fine_lines: Option<u32>,
    pub(crate) max_protocol_events_per_frame: Option<u32>,
}

impl MousePatch {
    pub(crate) fn apply_to(self, mut base: MouseConfig) -> MouseConfig {
        if let Some(v) = self.lines_per_notch {
            base.lines_per_notch = v;
        }
        if let Some(v) = self.fine_modifier {
            base.fine_modifier = v;
        }
        if let Some(v) = self.fine_lines {
            base.fine_lines = v;
        }
        if let Some(v) = self.max_protocol_events_per_frame {
            base.max_protocol_events_per_frame = v;
        }
        base
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_expected_values() {
        let cfg = MouseConfig::default();
        assert_eq!(cfg.lines_per_notch, 3);
        assert_eq!(cfg.fine_modifier, FineModifier::Shift);
        assert_eq!(cfg.fine_lines, 1);
        assert_eq!(cfg.max_protocol_events_per_frame, 8);
    }

    #[test]
    fn patch_overrides_only_present_fields() {
        let patch = MousePatch {
            lines_per_notch: Some(5),
            ..Default::default()
        };
        let merged = patch.apply_to(MouseConfig::default());
        assert_eq!(merged.lines_per_notch, 5);
        assert_eq!(merged.fine_modifier, FineModifier::Shift);
        assert_eq!(merged.fine_lines, 1);
    }

    #[test]
    fn fine_modifier_parses_lowercase_string() {
        let patch: MousePatch = toml::from_str(r#"fine_modifier = "ctrl""#).unwrap();
        assert_eq!(patch.fine_modifier, Some(FineModifier::Ctrl));
    }

    #[test]
    fn fine_modifier_none_variant_parses() {
        let patch: MousePatch = toml::from_str(r#"fine_modifier = "none""#).unwrap();
        assert_eq!(patch.fine_modifier, Some(FineModifier::None));
    }

    #[test]
    fn empty_patch_leaves_base_unchanged() {
        let patch = MousePatch::default();
        let merged = patch.apply_to(MouseConfig::default());
        assert_eq!(merged, MouseConfig::default());
    }
}
