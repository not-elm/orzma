//! Host-owned mouse input policy, populated from `orzma_configs` at startup.

use bevy::prelude::*;
use std::time::Duration;

/// Which modifier activates "fine" (1 line per notch) wheel scrolling.
/// On macOS, Shift+wheel becomes horizontal scroll at the OS level, so
/// Shift never reaches the app as vertical `y`.
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

/// Host-side burst cap for PTY-bound button reports. Currently unused.
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

/// Host-side wheel-routing policy. `lines_per_notch` and `fine_lines` drive
/// the viewport-scroll computation; `max_protocol_events_per_frame` is
/// currently unused.
///
/// TODO: reintroduce mouse-wheel PTY reporting against `orzma_tty`.
#[derive(Clone, Debug)]
pub(crate) struct WheelConfig {
    /// Lines scrolled per notch in the scrollback path.
    pub lines_per_notch: u32,
    /// Lines scrolled per notch when the fine-scroll modifier is held.
    pub fine_lines: u32,
    /// Upper bound on PTY-bound wheel reports emitted per dispatch call.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "read again when sub-project C ports wheel reporting"
        )
    )]
    pub max_protocol_events_per_frame: u32,
}

impl Default for WheelConfig {
    fn default() -> Self {
        Self {
            lines_per_notch: 3,
            fine_lines: 1,
            max_protocol_events_per_frame: 8,
        }
    }
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
    /// Wheel routing config (lines-per-notch, fine lines, burst cap).
    pub wheel: WheelConfig,
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

impl Default for OrzmaMouseConfig {
    fn default() -> Self {
        Self {
            buttons: ButtonConfig {
                max_protocol_events_per_frame: 8,
            },
            wheel: WheelConfig::default(),
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

    #[test]
    fn default_config_sets_button_cap_explicitly() {
        let cfg = OrzmaMouseConfig::default();
        assert_eq!(
            cfg.buttons.max_protocol_events_per_frame, 8,
            "must NOT be ButtonConfig::default()'s 0"
        );
        assert_eq!(cfg.wheel.max_protocol_events_per_frame, 8);
        assert_eq!(cfg.cells_per_notch, 0.5);
        assert_eq!(cfg.axis_lock_ratio, 0.9);
        assert_eq!(
            cfg.double_click_timeout,
            std::time::Duration::from_millis(400)
        );
        assert_eq!(cfg.click_drift_px, 8.0);
        assert_eq!(cfg.fine_modifier, FineModifier::Alt);
    }
}
