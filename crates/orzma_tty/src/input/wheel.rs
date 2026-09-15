//! Wheel routing: decides whether wheel notches become mouse reports,
//! cursor keys, or a viewport scroll, from the modes the application set.

use crate::input::keyboard::TerminalKey;
use crate::input::mouse::MouseButton;
use orzma_vt::prelude::VtModes;

/// Wheel-routing policy, populated from the `[mouse]` config block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WheelConfig {
    /// Lines one notch moves, as a viewport scroll or as cursor keys,
    /// while the fine-scroll modifier is not held.
    pub lines_per_notch: u32,
    /// Lines one notch moves while the fine-scroll modifier is held.
    pub fine_lines: u32,
    /// The most notches one routing call turns into mouse reports or
    /// cursor keys; the notches past it are dropped.
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

/// The held modifiers wheel routing reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WheelModifiers {
    /// Shift is held, so the notches skip mouse reporting and take the
    /// route below it.
    pub shift: bool,
    /// The fine-scroll modifier is held, so a notch moves `fine_lines`
    /// rather than `lines_per_notch`.
    pub fine: bool,
}

/// Where one axis of a wheel gesture goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WheelDecision {
    /// Wheel reports bound for the application, each a press of `button`.
    Report {
        /// The wheel button every report carries.
        button: MouseButton,
        /// How many reports to send.
        count: u32,
    },
    /// Presses of a cursor key bound for the application.
    CursorKeys {
        /// The cursor key to press.
        key: TerminalKey,
        /// How many presses to send.
        count: u32,
    },
    /// A viewport scroll by this many lines; positive moves toward older
    /// output.
    ScrollViewport(i32),
    /// Nothing to do.
    Noop,
}

impl WheelDecision {
    /// Routes vertical notches, where positive means wheel up.
    ///
    /// While a mouse tracking level is in force and Shift is not held,
    /// every notch becomes one report, whatever the lines per notch.
    /// Otherwise, while alternate scroll is in effect, the notches' lines
    /// become cursor keys, and anything else scrolls the viewport. Reports
    /// and cursor keys cover at most `max_protocol_events_per_frame`
    /// notches, and a viewport scroll saturates rather than overflowing.
    /// Zero notches, or a count that comes out zero, route to
    /// [`Self::Noop`].
    pub fn route(modes: VtModes, notches: i32, mods: WheelModifiers, cfg: &WheelConfig) -> Self {
        if notches == 0 {
            return Self::Noop;
        }
        let up = notches > 0;
        if modes.mouse_reporting_active() && !mods.shift {
            let button = if up {
                MouseButton::WheelUp
            } else {
                MouseButton::WheelDown
            };
            return Self::report(button, notches, cfg);
        }
        let lines_per = lines_per_notch(mods, cfg);
        if modes.alternate_scroll_active() {
            let key = if up {
                TerminalKey::ArrowUp
            } else {
                TerminalKey::ArrowDown
            };
            return Self::cursor_keys(key, capped_notches(notches, cfg).saturating_mul(lines_per));
        }
        match notches.saturating_mul(i32::try_from(lines_per).unwrap_or(i32::MAX)) {
            0 => Self::Noop,
            lines => Self::ScrollViewport(lines),
        }
    }

    /// Routes horizontal notches, where positive means rightward.
    ///
    /// Only a mouse tracking level in force without Shift held has a
    /// route: every notch becomes one report, capped as in
    /// [`Self::route`]. Everything else routes to [`Self::Noop`].
    pub fn route_horizontal(
        modes: VtModes,
        notches: i32,
        mods: WheelModifiers,
        cfg: &WheelConfig,
    ) -> Self {
        if notches == 0 || !modes.mouse_reporting_active() || mods.shift {
            return Self::Noop;
        }
        let button = if notches > 0 {
            MouseButton::WheelRight
        } else {
            MouseButton::WheelLeft
        };
        Self::report(button, notches, cfg)
    }

    fn report(button: MouseButton, notches: i32, cfg: &WheelConfig) -> Self {
        match capped_notches(notches, cfg) {
            0 => Self::Noop,
            count => Self::Report { button, count },
        }
    }

    fn cursor_keys(key: TerminalKey, count: u32) -> Self {
        match count {
            0 => Self::Noop,
            count => Self::CursorKeys { key, count },
        }
    }
}

fn lines_per_notch(mods: WheelModifiers, cfg: &WheelConfig) -> u32 {
    if mods.fine {
        cfg.fine_lines
    } else {
        cfg.lines_per_notch
    }
}

fn capped_notches(notches: i32, cfg: &WheelConfig) -> u32 {
    notches
        .unsigned_abs()
        .min(cfg.max_protocol_events_per_frame)
}

#[cfg(test)]
mod tests {
    use super::*;
    use orzma_vt::prelude::{AlternateScroll, MouseTracking, ScreenKind};

    const PLAIN: WheelModifiers = WheelModifiers {
        shift: false,
        fine: false,
    };
    const SHIFT: WheelModifiers = WheelModifiers {
        shift: true,
        fine: false,
    };
    const FINE: WheelModifiers = WheelModifiers {
        shift: false,
        fine: true,
    };

    fn tracking() -> VtModes {
        VtModes {
            mouse_tracking: MouseTracking::Drag,
            ..VtModes::default()
        }
    }

    fn alternate_screen() -> VtModes {
        VtModes {
            active_screen: ScreenKind::Alternate,
            ..VtModes::default()
        }
    }

    fn tracking_on_the_alternate_screen() -> VtModes {
        VtModes {
            mouse_tracking: MouseTracking::Drag,
            active_screen: ScreenKind::Alternate,
            ..VtModes::default()
        }
    }

    /// Asserts that a mouse-tracking terminal gets one wheel report per
    /// notch, carrying the wheel's direction.
    ///
    /// Case: nvim runs with `mouse=nvi`, so button-event tracking is on,
    /// and the user spins the wheel up two notches and then down three.
    #[test]
    fn a_tracking_terminal_gets_one_report_per_notch_in_the_wheel_direction() {
        let cfg = WheelConfig::default();
        assert_eq!(
            WheelDecision::route(tracking(), 2, PLAIN, &cfg),
            WheelDecision::Report {
                button: MouseButton::WheelUp,
                count: 2
            }
        );
        assert_eq!(
            WheelDecision::route(tracking(), -3, PLAIN, &cfg),
            WheelDecision::Report {
                button: MouseButton::WheelDown,
                count: 3
            }
        );
    }

    /// Asserts that reports count notches, not lines, whatever the lines
    /// per notch and the fine modifier say.
    ///
    /// Case: a user who set `lines_per_notch = 5` holds the fine modifier
    /// while spinning the wheel one notch over nvim.
    #[test]
    fn reports_ignore_the_lines_per_notch_and_the_fine_modifier() {
        let cfg = WheelConfig {
            lines_per_notch: 5,
            ..WheelConfig::default()
        };
        assert_eq!(
            WheelDecision::route(tracking(), 1, FINE, &cfg),
            WheelDecision::Report {
                button: MouseButton::WheelUp,
                count: 1
            }
        );
    }

    /// Asserts that reports stop at the burst limit, dropping the excess
    /// notches, and that a zero limit sends nothing.
    ///
    /// Case: a trackpad flick produces twenty notches in one frame over a
    /// tracking application.
    #[test]
    fn reports_are_capped_at_the_burst_limit_and_the_excess_is_dropped() {
        assert_eq!(
            WheelDecision::route(tracking(), 20, PLAIN, &WheelConfig::default()),
            WheelDecision::Report {
                button: MouseButton::WheelUp,
                count: 8
            }
        );
        let silent = WheelConfig {
            max_protocol_events_per_frame: 0,
            ..WheelConfig::default()
        };
        assert_eq!(
            WheelDecision::route(tracking(), 20, PLAIN, &silent),
            WheelDecision::Noop
        );
    }

    /// Asserts that alternate scroll sends the notches' lines as cursor
    /// keys in the wheel's direction.
    ///
    /// Case: `less` shows a long file on the alternate screen without
    /// tracking the mouse, and the user spins the wheel up two notches and
    /// then down one.
    #[test]
    fn alternate_scroll_sends_the_notches_lines_as_cursor_keys() {
        let cfg = WheelConfig::default();
        assert_eq!(
            WheelDecision::route(alternate_screen(), 2, PLAIN, &cfg),
            WheelDecision::CursorKeys {
                key: TerminalKey::ArrowUp,
                count: 6
            }
        );
        assert_eq!(
            WheelDecision::route(alternate_screen(), -1, PLAIN, &cfg),
            WheelDecision::CursorKeys {
                key: TerminalKey::ArrowDown,
                count: 3
            }
        );
    }

    /// Asserts that cursor keys apply the burst limit to the notches
    /// before multiplying by the lines per notch.
    ///
    /// Case: a trackpad flick produces twenty notches in one frame over
    /// `man`.
    #[test]
    fn cursor_keys_cap_the_notches_before_multiplying_by_the_lines() {
        assert_eq!(
            WheelDecision::route(alternate_screen(), 20, PLAIN, &WheelConfig::default()),
            WheelDecision::CursorKeys {
                key: TerminalKey::ArrowUp,
                count: 24
            }
        );
    }

    /// Asserts that the fine modifier moves `fine_lines` per notch on both
    /// the cursor-key route and the viewport route.
    ///
    /// Case: the user holds the fine modifier to scroll slowly, first in
    /// `less` and then at a shell prompt.
    #[test]
    fn the_fine_modifier_moves_fine_lines_on_both_line_routes() {
        let cfg = WheelConfig::default();
        assert_eq!(
            WheelDecision::route(alternate_screen(), 2, FINE, &cfg),
            WheelDecision::CursorKeys {
                key: TerminalKey::ArrowUp,
                count: 2
            }
        );
        assert_eq!(
            WheelDecision::route(VtModes::default(), -2, FINE, &cfg),
            WheelDecision::ScrollViewport(-2)
        );
    }

    /// Asserts that without tracking or alternate scroll the viewport
    /// scrolls by the notches' lines, keeping the wheel's sign.
    ///
    /// Case: the user spins the wheel up and then down at a shell prompt.
    #[test]
    fn without_tracking_or_alternate_scroll_the_viewport_scrolls_by_the_notches_lines() {
        let cfg = WheelConfig::default();
        assert_eq!(
            WheelDecision::route(VtModes::default(), 1, PLAIN, &cfg),
            WheelDecision::ScrollViewport(3)
        );
        assert_eq!(
            WheelDecision::route(VtModes::default(), -2, PLAIN, &cfg),
            WheelDecision::ScrollViewport(-6)
        );
    }

    /// Asserts that Shift over a tracking terminal skips the reports and
    /// takes the route below: cursor keys on the alternate screen, and a
    /// viewport scroll on the primary one.
    ///
    /// Case: the user holds Shift while spinning the wheel, first over
    /// nvim and then over fzf, which tracks the mouse on the primary
    /// screen.
    #[test]
    fn shift_over_a_tracking_terminal_falls_through_to_the_route_below() {
        let cfg = WheelConfig::default();
        assert_eq!(
            WheelDecision::route(tracking_on_the_alternate_screen(), 1, SHIFT, &cfg),
            WheelDecision::CursorKeys {
                key: TerminalKey::ArrowUp,
                count: 3
            }
        );
        assert_eq!(
            WheelDecision::route(tracking(), 1, SHIFT, &cfg),
            WheelDecision::ScrollViewport(3)
        );
    }

    /// Asserts that a disabled alternate scroll leaves the alternate
    /// screen to the viewport route.
    ///
    /// Case: a pager turned DECSET 1007 off, and the user spins the wheel
    /// over it.
    #[test]
    fn a_disabled_alternate_scroll_leaves_the_alternate_screen_to_the_viewport() {
        let modes = VtModes {
            alternate_scroll: AlternateScroll::Disabled,
            ..alternate_screen()
        };
        assert_eq!(
            WheelDecision::route(modes, 1, PLAIN, &WheelConfig::default()),
            WheelDecision::ScrollViewport(3)
        );
    }

    /// Asserts that zero notches route nowhere on either axis, whatever
    /// the modes.
    ///
    /// Case: a slow trackpad frame moves less than one notch.
    #[test]
    fn zero_notches_route_nowhere() {
        let cfg = WheelConfig::default();
        for modes in [tracking(), alternate_screen(), VtModes::default()] {
            assert_eq!(
                WheelDecision::route(modes, 0, PLAIN, &cfg),
                WheelDecision::Noop
            );
            assert_eq!(
                WheelDecision::route_horizontal(modes, 0, PLAIN, &cfg),
                WheelDecision::Noop
            );
        }
    }

    /// Asserts that a viewport scroll saturates rather than overflowing.
    ///
    /// Case: a misbehaving input device reports an absurd notch count at a
    /// shell prompt.
    #[test]
    fn a_viewport_scroll_saturates_rather_than_overflowing() {
        assert_eq!(
            WheelDecision::route(VtModes::default(), i32::MAX, PLAIN, &WheelConfig::default()),
            WheelDecision::ScrollViewport(i32::MAX)
        );
    }

    /// Asserts that horizontal notches become left or right wheel reports
    /// only while the terminal tracks the mouse and Shift is not held.
    ///
    /// Case: the user swipes sideways over a tracking application, over
    /// `less`, over a shell prompt, and over the tracking application again
    /// while holding Shift.
    #[test]
    fn a_horizontal_notch_reports_left_or_right_only_while_tracking_without_shift() {
        let cfg = WheelConfig::default();
        assert_eq!(
            WheelDecision::route_horizontal(tracking(), 1, PLAIN, &cfg),
            WheelDecision::Report {
                button: MouseButton::WheelRight,
                count: 1
            }
        );
        assert_eq!(
            WheelDecision::route_horizontal(tracking(), -2, PLAIN, &cfg),
            WheelDecision::Report {
                button: MouseButton::WheelLeft,
                count: 2
            }
        );
        assert_eq!(
            WheelDecision::route_horizontal(alternate_screen(), 1, PLAIN, &cfg),
            WheelDecision::Noop
        );
        assert_eq!(
            WheelDecision::route_horizontal(VtModes::default(), 1, PLAIN, &cfg),
            WheelDecision::Noop
        );
        assert_eq!(
            WheelDecision::route_horizontal(tracking(), 1, SHIFT, &cfg),
            WheelDecision::Noop
        );
    }

    /// Asserts that horizontal reports share the vertical burst limit.
    ///
    /// Case: a fast sideways trackpad flick produces twenty notches in one
    /// frame over a tracking application.
    #[test]
    fn horizontal_reports_share_the_burst_limit() {
        assert_eq!(
            WheelDecision::route_horizontal(tracking(), 20, PLAIN, &WheelConfig::default()),
            WheelDecision::Report {
                button: MouseButton::WheelRight,
                count: 8
            }
        );
    }
}
