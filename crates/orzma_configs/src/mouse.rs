//! Mouse-input configuration: the `[mouse]` section's wheel, click, and
//! divider-grab settings.

use serde::{Deserialize, Serialize};

/// Which modifier triggers "fine" scrolling (1 line per notch instead
/// of `lines_per_notch`). The default is `Alt`.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum FineModifier {
    /// Shift key activates fine scrolling. It never fires on macOS.
    Shift,
    /// Ctrl key activates fine scrolling.
    Ctrl,
    /// Alt key activates fine scrolling.
    #[default]
    Alt,
    /// No modifier required; fine scrolling is always active.
    None,
}

/// Fully-resolved `[mouse]` config block.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct MouseConfig {
    /// Lines scrolled per notch in the scrollback / alt-screen paths.
    /// Mouse reports ignore it.
    pub lines_per_notch: u32,
    /// Which modifier key activates fine scrolling.
    pub fine_modifier: FineModifier,
    /// Lines scrolled per notch when the fine modifier is held.
    pub fine_lines: u32,
    /// Per axis and frame, the most mouse reports sent and the most wheel
    /// notches turned into alternate-scroll cursor keys. The excess is
    /// dropped, and cursor keys additionally stop at a fixed per-call
    /// ceiling whatever `lines_per_notch` says.
    pub max_protocol_events_per_frame: u32,
    /// Wheel-input accumulation threshold expressed in cells of input
    /// per emitted "notch". A lower value is more responsive, firing a
    /// notch after a smaller wheel movement. Mouse reports ignore it: one
    /// is sent per whole cell of travel. The default is `0.3333`.
    pub cells_per_notch: f32,
    /// Dominant-axis lock strength for trackpad scrolling. The horizontal
    /// component of a swipe is emitted only when it dominates the gesture
    /// (`|x| / hypot(x, y) >= axis_lock_ratio`), and is dropped otherwise.
    /// Clamped to `0.0..=1.0`, where a higher value is stricter: `1.0`
    /// allows horizontal motion only for a pure-horizontal gesture, and
    /// `0.0` disables the lock. The default is `0.9`.
    pub axis_lock_ratio: f32,
    /// Max gap (ms) between consecutive clicks counted as a double /
    /// triple click.
    pub double_click_timeout_ms: u32,
    /// Max cursor drift (logical px) between clicks counted as the
    /// same chord.
    pub click_drift_px: f32,
    /// Half-width (logical px) of a pane divider's grab zone for resize. A
    /// value below half a cell grabs half a cell; `inf` and `nan` fall back
    /// to the default of 4.0.
    pub divider_grab_tolerance_px: f32,
}

impl MouseConfig {
    /// Clamps `axis_lock_ratio` to `0.0..=1.0`. A non-finite
    /// `axis_lock_ratio` or `divider_grab_tolerance_px` falls back to its
    /// default.
    pub(crate) fn normalize(&mut self) {
        let default = Self::default();
        self.axis_lock_ratio = if self.axis_lock_ratio.is_finite() {
            self.axis_lock_ratio.clamp(0.0, 1.0)
        } else {
            default.axis_lock_ratio
        };
        if !self.divider_grab_tolerance_px.is_finite() {
            self.divider_grab_tolerance_px = default.divider_grab_tolerance_px;
        }
    }
}

impl Default for MouseConfig {
    fn default() -> Self {
        Self {
            lines_per_notch: 1,
            fine_modifier: FineModifier::Alt,
            fine_lines: 1,
            max_protocol_events_per_frame: 24,
            cells_per_notch: 0.3333,
            axis_lock_ratio: 0.9,
            double_click_timeout_ms: 400,
            click_drift_px: 8.0,
            divider_grab_tolerance_px: 4.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts that every `[mouse]` key defaults to its documented value.
    ///
    /// Case: a user whose config.toml has no `[mouse]` section starts orzma.
    #[test]
    fn defaults_match_expected_values() {
        let cfg = MouseConfig::default();
        assert_eq!(cfg.lines_per_notch, 1);
        assert_eq!(cfg.fine_modifier, FineModifier::Alt);
        assert_eq!(cfg.fine_lines, 1);
        assert_eq!(cfg.max_protocol_events_per_frame, 24);
        assert_eq!(cfg.cells_per_notch, 0.3333);
        assert_eq!(cfg.axis_lock_ratio, 0.9);
        assert_eq!(cfg.double_click_timeout_ms, 400);
        assert_eq!(cfg.click_drift_px, 8.0);
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
    fn normalize_clamps_axis_lock_ratio() {
        let clamp = |raw: f32| {
            let mut cfg = MouseConfig {
                axis_lock_ratio: raw,
                ..MouseConfig::default()
            };
            cfg.normalize();
            cfg.axis_lock_ratio
        };
        assert_eq!(
            clamp(9.0),
            1.0,
            "a 0.9 typo of 9 must clamp, not kill scroll"
        );
        assert_eq!(clamp(90.0), 1.0);
        assert_eq!(clamp(-1.0), 0.0);
        assert_eq!(clamp(0.7), 0.7, "an in-range value is left untouched");
        assert_eq!(clamp(f32::NAN), 0.9, "NaN falls back to the default");
        assert_eq!(clamp(f32::INFINITY), 0.9, "inf falls back to the default");
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

    /// Asserts that a non-finite divider grab tolerance falls back to the
    /// default while a finite one, however small, is kept.
    ///
    /// Case: a user writes `divider_grab_tolerance_px = inf` in config.toml,
    /// and another writes `0.5`.
    #[test]
    fn normalize_resets_a_non_finite_grab_tolerance() {
        let normalized = |raw: f32| {
            let mut cfg = MouseConfig {
                divider_grab_tolerance_px: raw,
                ..MouseConfig::default()
            };
            cfg.normalize();
            cfg.divider_grab_tolerance_px
        };
        assert_eq!(normalized(f32::INFINITY), 4.0);
        assert_eq!(normalized(f32::NAN), 4.0);
        assert_eq!(normalized(0.5), 0.5);
        assert_eq!(normalized(12.0), 12.0);
    }
}
