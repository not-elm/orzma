//! Host-owned keyboard and mouse input policy: `TerminalInputBindings` /
//! `ReservedChord` for keyboard dispatch, and `OzmaMouseConfig` / `FineModifier`
//! for mouse. Keyboard bindings are populated from the resolved shortcut table
//! (`crate::input::shortcuts::populate_input_bindings`); mouse config from
//! `ozmux_configs` at startup (`crate::input::shortcuts::populate_mouse_config`).

use bevy::prelude::*;
use ozma_tty_engine::{ButtonConfig, WheelConfig};
use std::time::Duration;

/// A keyboard chord, as a physical `KeyCode` plus the four modifier bits.
/// Config-agnostic plain data the host supplies in `TerminalInputBindings`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReservedChord {
    /// The physical key.
    pub key_code: KeyCode,
    /// Control held.
    pub ctrl: bool,
    /// Shift held.
    pub shift: bool,
    /// Alt/Option held.
    pub alt: bool,
    /// Meta/Cmd/Super held.
    pub meta: bool,
}

/// Host-supplied input policy: the chord that triggers the built-in Paste
/// action, plus the chords the dispatcher must skip (the host handles those).
/// Both are populated together so the "paste is not also reserved" invariant
/// lives in one place.
///
/// `Default` is `Cmd+V` paste + empty reserved, so a spawn-and-go consumer
/// still gets working paste and forwards everything else.
#[derive(Resource)]
pub(crate) struct TerminalInputBindings {
    /// The chord that triggers `PasteAction`, when paste is direct-bound.
    /// `None` when paste is leader-scoped or unbound (the leader path in
    /// `app_shortcut_handler` owns paste then).
    pub paste: Option<ReservedChord>,
    /// Chords the dispatcher skips for the host to handle.
    pub reserved: Vec<ReservedChord>,
}

impl Default for TerminalInputBindings {
    fn default() -> Self {
        Self {
            paste: Some(ReservedChord {
                key_code: KeyCode::KeyV,
                ctrl: false,
                shift: false,
                alt: false,
                meta: true,
            }),
            reserved: Vec::new(),
        }
    }
}

/// Which modifier activates "fine" (1 line per notch) wheel scrolling.
/// Default `Alt`: on macOS Shift+wheel becomes horizontal scroll at the OS
/// level, so Shift never reaches the app as vertical `y`.
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

/// Host-supplied mouse policy. `Default` is a working spawn-and-go config; the
/// host overrides it from `ozmux_configs`.
#[derive(Resource)]
pub(crate) struct OzmaMouseConfig {
    /// Button-report burst cap. MUST be non-zero or forwarded clicks are dropped.
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

impl Default for OzmaMouseConfig {
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
        let cfg = OzmaMouseConfig::default();
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
