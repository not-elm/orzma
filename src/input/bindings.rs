//! Host-owned mouse input policy, populated from `orzma_configs` at startup.

use bevy::prelude::*;
use orzma_configs::mouse::{FineModifier as CfgFineModifier, MouseConfig};
use std::time::Duration;

/// Which modifier activates "fine" (1 line per notch) wheel scrolling.
///
/// On macOS, `Shift` never activates fine scrolling.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum FineModifier {
    /// Shift key activates fine scrolling.
    Shift,
    /// Control key activates fine scrolling.
    Ctrl,
    /// Alt/Option key activates fine scrolling.
    #[default]
    Alt,
    /// No modifier required; fine scrolling is always active.
    None,
}

/// Host-side burst cap for PTY-bound button reports.
///
/// TODO: reintroduce mouse-button routing against `orzma_tty`.
#[derive(Clone, Debug, Default)]
pub(crate) struct ButtonConfig {
    /// Hard cap on the number of PTY-bound reports emitted per route call.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "read again when sub-project C ports button reporting"
        )
    )]
    pub max_protocol_events_per_frame: u32,
}

/// Host-supplied mouse policy. `Default` is a working spawn-and-go config; the
/// host overrides it from `orzma_configs`.
#[derive(Resource)]
pub(crate) struct OrzmaMouseConfig {
    /// Button-report burst cap. MUST be non-zero or forwarded clicks are dropped.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "read again when sub-project C ports button reporting"
        )
    )]
    pub buttons: ButtonConfig,
    /// Cells of wheel travel per emitted notch (smooth-scroll accumulation).
    pub cells_per_notch: f32,
    /// Dominant-axis lock strength: horizontal scroll survives only when
    /// `|x| / hypot(x, y) >= axis_lock_ratio`, else it is dropped. Range
    /// `0.0..=1.0` (`0.0` = off, `1.0` = pure-horizontal only).
    pub axis_lock_ratio: f32,
    /// Max gap between clicks counted as a double / triple click.
    pub double_click_timeout: Duration,
    /// Max cursor drift (logical px) between clicks of one chord.
    pub click_drift_px: f32,
    /// Which modifier activates fine scrolling.
    pub fine_modifier: FineModifier,
}

impl OrzmaMouseConfig {
    /// The policy the resolved `[mouse]` block selects.
    pub(crate) fn from_config(mc: &MouseConfig) -> Self {
        Self {
            buttons: ButtonConfig {
                max_protocol_events_per_frame: mc.max_protocol_events_per_frame,
            },
            cells_per_notch: mc.cells_per_notch,
            axis_lock_ratio: mc.axis_lock_ratio,
            double_click_timeout: Duration::from_millis(mc.double_click_timeout_ms as u64),
            click_drift_px: mc.click_drift_px,
            fine_modifier: match mc.fine_modifier {
                CfgFineModifier::Shift => FineModifier::Shift,
                CfgFineModifier::Ctrl => FineModifier::Ctrl,
                CfgFineModifier::Alt => FineModifier::Alt,
                CfgFineModifier::None => FineModifier::None,
            },
        }
    }
}

impl Default for OrzmaMouseConfig {
    fn default() -> Self {
        Self {
            buttons: ButtonConfig {
                max_protocol_events_per_frame: 8,
            },
            cells_per_notch: 0.5,
            axis_lock_ratio: 0.9,
            double_click_timeout: Duration::from_millis(400),
            click_drift_px: 8.0,
            fine_modifier: FineModifier::Alt,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts that the spawn-and-go default sets every field to its
    /// documented value, with a non-zero button cap.
    ///
    /// Case: the app starts before the `[mouse]` block has been applied.
    #[test]
    fn default_config_sets_button_cap_explicitly() {
        let cfg = OrzmaMouseConfig::default();
        assert_eq!(
            cfg.buttons.max_protocol_events_per_frame, 8,
            "must NOT be ButtonConfig::default()'s 0"
        );
        assert_eq!(cfg.cells_per_notch, 0.5);
        assert_eq!(cfg.axis_lock_ratio, 0.9);
        assert_eq!(cfg.double_click_timeout, Duration::from_millis(400));
        assert_eq!(cfg.click_drift_px, 8.0);
        assert_eq!(cfg.fine_modifier, FineModifier::Alt);
    }

    /// Asserts that each `[mouse]` field lands on its counterpart.
    ///
    /// Case: a user sets `fine_modifier = "ctrl"`,
    /// `max_protocol_events_per_frame = 5`, `cells_per_notch = 1.0`, and
    /// `axis_lock_ratio = 0.5` in config.toml.
    #[test]
    fn mouse_config_maps_from_orzma_config() {
        let mc = MouseConfig {
            fine_modifier: CfgFineModifier::Ctrl,
            max_protocol_events_per_frame: 5,
            cells_per_notch: 1.0,
            axis_lock_ratio: 0.5,
            ..MouseConfig::default()
        };
        let out = OrzmaMouseConfig::from_config(&mc);
        assert_eq!(out.buttons.max_protocol_events_per_frame, 5);
        assert_eq!(out.cells_per_notch, 1.0);
        assert_eq!(
            out.axis_lock_ratio, 0.5,
            "non-default value must flow through"
        );
        assert_eq!(out.fine_modifier, FineModifier::Ctrl);
        assert_eq!(
            out.double_click_timeout,
            Duration::from_millis(mc.double_click_timeout_ms as u64)
        );
        assert_eq!(out.click_drift_px, mc.click_drift_px);
    }
}
