//! Wheel routing: decides whether wheel travel becomes mouse reports,
//! cursor keys, or a viewport scroll, from the modes the application set.

use crate::input::keyboard::TerminalKey;
use crate::input::mouse::{CellCoord, MouseButton, ProtocolModifiers};
use orzma_vt::prelude::VtModes;

/// Wheel-routing policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WheelConfig {
    /// Lines one notch moves, as a viewport scroll or as cursor keys,
    /// while the fine-scroll modifier is not held.
    pub lines_per_notch: u32,
    /// Lines one notch moves while the fine-scroll modifier is held.
    pub fine_lines: u32,
    /// The most reports, and the most notches turned into cursor keys, one
    /// routing call sends per axis; the excess is dropped. Cursor keys
    /// additionally stop at [`MAX_CURSOR_KEYS`] per call, whatever the
    /// lines per notch.
    pub max_protocol_events_per_frame: u32,
}

impl Default for WheelConfig {
    fn default() -> Self {
        Self {
            lines_per_notch: 1,
            fine_lines: 1,
            max_protocol_events_per_frame: 24,
        }
    }
}

/// The most cursor keys one routing call sends, whatever the lines per
/// notch.
pub const MAX_CURSOR_KEYS: u32 = 240;

/// The held modifiers wheel routing reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WheelModifiers {
    /// Shift is held, so the frame skips mouse reporting and its notches
    /// take the route below it.
    pub shift: bool,
    /// The fine-scroll modifier is held, so a notch moves `fine_lines`
    /// rather than `lines_per_notch`.
    pub fine: bool,
}

/// One frame's wheel travel in whole steps: notches on the vertical axis,
/// and wheel reports on both axes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WheelSteps {
    /// Vertical notches; positive is wheel-up. Cursor keys and viewport
    /// scrolls move by these.
    pub up: i32,
    /// Vertical wheel reports, one per whole cell of travel; positive is
    /// wheel-up.
    pub report_up: i32,
    /// Horizontal wheel reports, one per whole cell of travel; positive is
    /// rightward.
    pub report_right: i32,
}

/// One frame's wheel travel over a terminal, with what routing it needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WheelInput {
    /// The notches and reports the frame completed.
    pub steps: WheelSteps,
    /// The held modifiers routing reads.
    pub mods: WheelModifiers,
    /// The cell under the cursor, or `None` when no cell was resolved for
    /// it. A report needs a cell and is dropped without one.
    pub cell: Option<CellCoord>,
    /// The modifier bits every report carries.
    pub report_mods: ProtocolModifiers,
}

/// Where one axis of a wheel gesture goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WheelDecision {
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
    /// Routes one frame's vertical travel, where positive means wheel up.
    ///
    /// While a mouse tracking level is in force and Shift is not held,
    /// each of `reports` becomes one report and `notches` is ignored.
    /// Otherwise `reports` is ignored: while alternate scroll is in effect,
    /// the notches' lines become cursor keys, and anything else scrolls the
    /// viewport. At most `max_protocol_events_per_frame` reports are sent,
    /// cursor keys cover at most that many notches and number at most
    /// [`MAX_CURSOR_KEYS`] whatever the lines per notch, and a viewport
    /// scroll saturates rather than overflowing. A route whose count is
    /// zero, or comes out zero, is [`Self::Noop`].
    pub fn route(
        modes: VtModes,
        notches: i32,
        reports: i32,
        mods: WheelModifiers,
        cfg: &WheelConfig,
    ) -> Self {
        if modes.mouse_reporting_active() && !mods.shift {
            let button = if reports > 0 {
                MouseButton::WheelUp
            } else {
                MouseButton::WheelDown
            };
            return Self::report(button, reports, cfg);
        }
        if notches == 0 {
            return Self::Noop;
        }
        let up = notches > 0;
        let lines_per = lines_per_notch(mods, cfg);
        if modes.alternate_scroll_active() {
            let key = if up {
                TerminalKey::ArrowUp
            } else {
                TerminalKey::ArrowDown
            };
            let count = capped(notches, cfg)
                .saturating_mul(lines_per)
                .min(MAX_CURSOR_KEYS);
            return Self::cursor_keys(key, count);
        }
        match notches.saturating_mul(i32::try_from(lines_per).unwrap_or(i32::MAX)) {
            0 => Self::Noop,
            lines => Self::ScrollViewport(lines),
        }
    }

    /// Routes one frame's horizontal reports, where positive means
    /// rightward.
    ///
    /// Only a mouse tracking level in force without Shift held has a
    /// route: each report is sent, up to `max_protocol_events_per_frame`.
    /// Zero reports, a count that comes out zero, and everything else
    /// route to [`Self::Noop`].
    pub fn route_horizontal(
        modes: VtModes,
        reports: i32,
        mods: WheelModifiers,
        cfg: &WheelConfig,
    ) -> Self {
        if !modes.mouse_reporting_active() || mods.shift {
            return Self::Noop;
        }
        let button = if reports > 0 {
            MouseButton::WheelRight
        } else {
            MouseButton::WheelLeft
        };
        Self::report(button, reports, cfg)
    }

    fn report(button: MouseButton, reports: i32, cfg: &WheelConfig) -> Self {
        match capped(reports, cfg) {
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

fn capped(count: i32, cfg: &WheelConfig) -> u32 {
    count.unsigned_abs().min(cfg.max_protocol_events_per_frame)
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

    /// The routing policy these tests pin: three lines per notch, one
    /// fine line, and at most eight reports or notches per call.
    fn policy() -> WheelConfig {
        WheelConfig {
            lines_per_notch: 3,
            fine_lines: 1,
            max_protocol_events_per_frame: 8,
        }
    }

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

    /// Asserts that a mouse-tracking terminal gets the frame's reports,
    /// each carrying the wheel's direction.
    ///
    /// Case: nvim runs with `mouse=nvi`, so button-event tracking is on,
    /// and the user swipes up two cells and then down three.
    #[test]
    fn a_tracking_terminal_gets_its_reports_in_the_wheel_direction() {
        let cfg = policy();
        assert_eq!(
            WheelDecision::route(tracking(), 0, 2, PLAIN, &cfg),
            WheelDecision::Report {
                button: MouseButton::WheelUp,
                count: 2
            }
        );
        assert_eq!(
            WheelDecision::route(tracking(), 0, -3, PLAIN, &cfg),
            WheelDecision::Report {
                button: MouseButton::WheelDown,
                count: 3
            }
        );
    }

    /// Asserts that the report count follows the frame's reports, not
    /// lines, whatever the lines per notch and the fine modifier say.
    ///
    /// Case: a user who set `lines_per_notch = 5` holds the fine modifier
    /// while spinning the wheel one click, three notches and one report,
    /// over nvim.
    #[test]
    fn reports_ignore_the_lines_per_notch_and_the_fine_modifier() {
        let cfg = WheelConfig {
            lines_per_notch: 5,
            ..policy()
        };
        assert_eq!(
            WheelDecision::route(tracking(), 3, 1, FINE, &cfg),
            WheelDecision::Report {
                button: MouseButton::WheelUp,
                count: 1
            }
        );
    }

    /// Asserts that reports stop at the burst limit, dropping the excess
    /// reports, and that a zero limit sends nothing.
    ///
    /// Case: a trackpad flick covers twenty cells in one frame over a
    /// tracking application, and a user who set the burst limit to zero
    /// makes the same flick.
    #[test]
    fn reports_are_capped_at_the_burst_limit_and_the_excess_is_dropped() {
        assert_eq!(
            WheelDecision::route(tracking(), 0, 20, PLAIN, &policy()),
            WheelDecision::Report {
                button: MouseButton::WheelUp,
                count: 8
            }
        );
        let silent = WheelConfig {
            max_protocol_events_per_frame: 0,
            ..policy()
        };
        assert_eq!(
            WheelDecision::route(tracking(), 0, 20, PLAIN, &silent),
            WheelDecision::Noop
        );
    }

    /// Asserts that a tracking terminal gets nothing for notches that come
    /// without a report.
    ///
    /// Case: nvim tracks the mouse, and the user nudges the trackpad a
    /// third of a cell, which completes a notch but no report.
    #[test]
    fn notches_without_a_report_route_nothing_while_tracking() {
        assert_eq!(
            WheelDecision::route(tracking(), 1, 0, PLAIN, &policy()),
            WheelDecision::Noop
        );
    }

    /// Asserts that the cursor-key and viewport routes move by the frame's
    /// notches and ignore its report count.
    ///
    /// Case: the user swipes the trackpad one notch over `less`, then at a
    /// shell prompt, in frames that also completed several reports.
    #[test]
    fn the_line_routes_count_notches_not_reports() {
        let cfg = policy();
        assert_eq!(
            WheelDecision::route(alternate_screen(), 1, 5, PLAIN, &cfg),
            WheelDecision::CursorKeys {
                key: TerminalKey::ArrowUp,
                count: 3
            }
        );
        assert_eq!(
            WheelDecision::route(VtModes::default(), 1, 5, PLAIN, &cfg),
            WheelDecision::ScrollViewport(3)
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
        let cfg = policy();
        assert_eq!(
            WheelDecision::route(alternate_screen(), 2, 0, PLAIN, &cfg),
            WheelDecision::CursorKeys {
                key: TerminalKey::ArrowUp,
                count: 6
            }
        );
        assert_eq!(
            WheelDecision::route(alternate_screen(), -1, 0, PLAIN, &cfg),
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
            WheelDecision::route(alternate_screen(), 20, 0, PLAIN, &policy()),
            WheelDecision::CursorKeys {
                key: TerminalKey::ArrowUp,
                count: 24
            }
        );
    }

    /// Asserts that cursor keys stop at the key limit however many lines
    /// each capped notch moves.
    ///
    /// Case: a user who set `lines_per_notch = 100000` spins the wheel one
    /// notch over `less`.
    #[test]
    fn cursor_keys_stop_at_the_key_limit() {
        let cfg = WheelConfig {
            lines_per_notch: 100_000,
            ..policy()
        };
        assert_eq!(
            WheelDecision::route(alternate_screen(), 1, 0, PLAIN, &cfg),
            WheelDecision::CursorKeys {
                key: TerminalKey::ArrowUp,
                count: MAX_CURSOR_KEYS
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
        let cfg = policy();
        assert_eq!(
            WheelDecision::route(alternate_screen(), 2, 0, FINE, &cfg),
            WheelDecision::CursorKeys {
                key: TerminalKey::ArrowUp,
                count: 2
            }
        );
        assert_eq!(
            WheelDecision::route(VtModes::default(), -2, 0, FINE, &cfg),
            WheelDecision::ScrollViewport(-2)
        );
    }

    /// Asserts that without tracking or alternate scroll the viewport
    /// scrolls by the notches' lines, keeping the wheel's sign.
    ///
    /// Case: the user spins the wheel up and then down at a shell prompt.
    #[test]
    fn without_tracking_or_alternate_scroll_the_viewport_scrolls_by_the_notches_lines() {
        let cfg = policy();
        assert_eq!(
            WheelDecision::route(VtModes::default(), 1, 0, PLAIN, &cfg),
            WheelDecision::ScrollViewport(3)
        );
        assert_eq!(
            WheelDecision::route(VtModes::default(), -2, 0, PLAIN, &cfg),
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
        let cfg = policy();
        assert_eq!(
            WheelDecision::route(tracking_on_the_alternate_screen(), 1, 5, SHIFT, &cfg),
            WheelDecision::CursorKeys {
                key: TerminalKey::ArrowUp,
                count: 3
            }
        );
        assert_eq!(
            WheelDecision::route(tracking(), 1, 5, SHIFT, &cfg),
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
            WheelDecision::route(modes, 1, 0, PLAIN, &policy()),
            WheelDecision::ScrollViewport(3)
        );
    }

    /// Asserts that a frame with no notches and no reports routes nowhere
    /// on either axis, whatever the modes.
    ///
    /// Case: a slow trackpad frame moves less than one notch.
    #[test]
    fn zero_travel_routes_nowhere() {
        let cfg = policy();
        for modes in [tracking(), alternate_screen(), VtModes::default()] {
            assert_eq!(
                WheelDecision::route(modes, 0, 0, PLAIN, &cfg),
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
            WheelDecision::route(VtModes::default(), i32::MAX, 0, PLAIN, &policy()),
            WheelDecision::ScrollViewport(i32::MAX)
        );
    }

    /// Asserts that horizontal reports become left or right wheel reports
    /// only while the terminal tracks the mouse and Shift is not held.
    ///
    /// Case: the user swipes sideways over a tracking application, over
    /// `less`, over a shell prompt, and over the tracking application again
    /// while holding Shift.
    #[test]
    fn a_horizontal_report_goes_left_or_right_only_while_tracking_without_shift() {
        let cfg = policy();
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
    /// Case: a fast sideways trackpad flick covers twenty cells in one frame
    /// over a tracking application.
    #[test]
    fn horizontal_reports_share_the_burst_limit() {
        assert_eq!(
            WheelDecision::route_horizontal(tracking(), 20, PLAIN, &policy()),
            WheelDecision::Report {
                button: MouseButton::WheelRight,
                count: 8
            }
        );
    }
}
