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
/// mouse-wheel and mouse-button input systems; the wheel-relevant
/// subset is mapped to `ozma_tty_engine::WheelConfig`, and the
/// button-relevant subset to `ozma_tty_engine::ButtonConfig`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
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
    /// Max gap (ms) between consecutive clicks counted as a double /
    /// triple click. Default mirrors macOS HIG.
    pub double_click_timeout_ms: u32,
    /// Max cursor drift (logical px) between clicks counted as the
    /// same chord. Default sized for Retina (4 logical = 8 physical
    /// at DPR 2.0).
    pub click_drift_px: f32,
    /// Drag-scroll tick rate (ms) at the pane edge. Decreased linearly
    /// by `autoscroll_step_ms` per cell past the edge, floored at
    /// `autoscroll_min_period_ms`.
    pub autoscroll_base_period_ms: u32,
    /// Hard floor (ms) on the drag-scroll rate. Caps CPU during
    /// sustained edge drag.
    pub autoscroll_min_period_ms: u32,
    /// Linear decrement (ms per cell past the edge) applied to
    /// `autoscroll_base_period_ms`.
    pub autoscroll_step_ms: u32,
    /// Pointer travel (logical px) before a left-press is treated as a drag
    /// rather than a click. Below this, release fires a click (focus / word /
    /// line); at or above it, the gesture becomes a resize or text drag.
    pub drag_threshold_px: f32,
    /// Half-width (logical px) of a pane divider's grab zone for resize.
    pub divider_grab_tolerance_px: f32,
}

impl Default for MouseConfig {
    fn default() -> Self {
        Self {
            lines_per_notch: 3,
            fine_modifier: FineModifier::Alt,
            fine_lines: 1,
            max_protocol_events_per_frame: 8,
            cells_per_notch: 0.5,
            double_click_timeout_ms: 400,
            click_drift_px: 8.0,
            autoscroll_base_period_ms: 50,
            autoscroll_min_period_ms: 16,
            autoscroll_step_ms: 4,
            drag_threshold_px: 4.0,
            divider_grab_tolerance_px: 4.0,
        }
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
        assert_eq!(cfg.double_click_timeout_ms, 400);
        assert_eq!(cfg.click_drift_px, 8.0);
        assert_eq!(cfg.autoscroll_base_period_ms, 50);
        assert_eq!(cfg.autoscroll_min_period_ms, 16);
        assert_eq!(cfg.autoscroll_step_ms, 4);
        assert_eq!(cfg.drag_threshold_px, 4.0);
        assert_eq!(cfg.divider_grab_tolerance_px, 4.0);
    }

    #[test]
    fn partial_mouse_fills_missing_from_default() {
        let cfg: MouseConfig =
            toml::from_str("lines_per_notch = 5\nclick_drift_px = 12.0").unwrap();
        assert_eq!(cfg.lines_per_notch, 5);
        assert_eq!(cfg.click_drift_px, 12.0);
        assert_eq!(cfg.fine_modifier, FineModifier::Alt);
        assert_eq!(cfg.fine_lines, 1);
    }

    #[test]
    fn fine_modifier_parses_lowercase() {
        let cfg: MouseConfig = toml::from_str(r#"fine_modifier = "ctrl""#).unwrap();
        assert_eq!(cfg.fine_modifier, FineModifier::Ctrl);
    }

    #[test]
    fn fine_modifier_none_variant_parses() {
        let cfg: MouseConfig = toml::from_str(r#"fine_modifier = "none""#).unwrap();
        assert_eq!(cfg.fine_modifier, FineModifier::None);
    }

    #[test]
    fn unknown_key_is_ignored() {
        let cfg: MouseConfig = toml::from_str("lines_per_notch = 5\nbogus = 1").unwrap();
        assert_eq!(cfg.lines_per_notch, 5);
    }
}
