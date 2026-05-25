//! Mouse-input configuration (currently: wheel scroll behavior).

use serde::{Deserialize, Serialize};

/// Which modifier triggers "fine" scrolling (1 line per notch instead
/// of `lines_per_notch`).
///
/// Default is `Alt`. Shift is deliberately not the default because
/// macOS converts Shift+wheel into a horizontal-scroll event at the
/// system level (vertical y becomes x), so Shift+wheel reaches the
/// app as `ev.y == 0` and the fine path never fires. Alt+wheel passes
/// through unchanged on macOS, Linux, and Windows.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum FineModifier {
    /// Shift key activates fine scrolling. **Broken on macOS** —
    /// system converts Shift+wheel to horizontal scroll.
    Shift,
    /// Ctrl key activates fine scrolling. May collide with future
    /// font-zoom shortcuts (kitty / Windows Terminal convention).
    Ctrl,
    /// Alt key activates fine scrolling. Default.
    #[default]
    Alt,
    /// No modifier required; fine scrolling is always active.
    None,
}

/// Fully-resolved `[mouse]` config block. Consumed by the Bevy
/// mouse-wheel input system; mapped 1:1 to `bevy_terminal::WheelConfig`
/// for the routing layer.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
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
    /// Wheel-input accumulation threshold expressed in cells of input
    /// per emitted "notch". Lower = more responsive (each small wheel
    /// movement fires a notch sooner).
    ///
    /// Default `0.5` works well for macOS smooth-scroll devices
    /// (Magic Mouse, high-resolution wheels, trackpads) which emit
    /// fractional line deltas; raise to `1.0` for a traditional
    /// discrete-notch wheel that already emits `y = 1.0` per click.
    pub cells_per_notch: f32,
}

impl Default for MouseConfig {
    fn default() -> Self {
        Self {
            lines_per_notch: 3,
            fine_modifier: FineModifier::Alt,
            fine_lines: 1,
            max_protocol_events_per_frame: 8,
            cells_per_notch: 0.5,
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
    pub(crate) cells_per_notch: Option<f32>,
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
        if let Some(v) = self.cells_per_notch {
            base.cells_per_notch = v;
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
        assert_eq!(cfg.fine_modifier, FineModifier::Alt);
        assert_eq!(cfg.fine_lines, 1);
        assert_eq!(cfg.max_protocol_events_per_frame, 8);
        assert_eq!(cfg.cells_per_notch, 0.5);
    }

    #[test]
    fn patch_overrides_only_present_fields() {
        let patch = MousePatch {
            lines_per_notch: Some(5),
            ..Default::default()
        };
        let merged = patch.apply_to(MouseConfig::default());
        assert_eq!(merged.lines_per_notch, 5);
        assert_eq!(merged.fine_modifier, FineModifier::Alt);
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
