//! The lit and dark phases a blinking caret follows after a keystroke.

use std::time::Duration;

/// Whether the blink phase is lit `elapsed` after the last keystroke.
///
/// Reports `true` for a caret that does not blink, once `timeout` has
/// passed, and for a zero `interval`. A `None` timeout blinks
/// indefinitely.
pub fn blink_phase_on(
    elapsed: Duration,
    interval: Option<Duration>,
    timeout: Option<Duration>,
) -> bool {
    let Some(interval) = interval else {
        return true;
    };
    if let Some(timeout) = timeout
        && elapsed >= timeout
    {
        return true;
    }
    let interval_ms = interval.as_millis();
    if interval_ms == 0 {
        return true;
    }
    (elapsed.as_millis() / interval_ms).is_multiple_of(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts that the phase alternates on the interval and settles
    /// lit once the timeout passes.
    ///
    /// Case: the user types, watches the caret blink, then leaves the
    /// keyboard alone.
    #[test]
    fn the_phase_alternates_then_settles_lit_until_the_next_keystroke() {
        let interval = Duration::from_millis(750);
        let timeout = Some(Duration::from_secs(5));
        assert!(blink_phase_on(Duration::ZERO, Some(interval), timeout));
        assert!(!blink_phase_on(
            Duration::from_millis(750),
            Some(interval),
            timeout
        ));
        assert!(blink_phase_on(
            Duration::from_millis(1500),
            Some(interval),
            timeout
        ));
        assert!(blink_phase_on(
            Duration::from_secs(5),
            Some(interval),
            timeout
        ));
        assert!(blink_phase_on(
            Duration::from_secs(600),
            Some(interval),
            timeout
        ));
    }

    /// Asserts that a `None` timeout blinks indefinitely rather than
    /// settling.
    ///
    /// Case: the user sets `blink_timeout = 0` and walks away.
    #[test]
    fn a_none_timeout_keeps_blinking() {
        let interval = Duration::from_millis(750);
        assert!(!blink_phase_on(
            Duration::from_millis(750),
            Some(interval),
            None
        ));
        assert!(!blink_phase_on(
            Duration::from_secs(6000) + Duration::from_millis(750),
            Some(interval),
            None
        ));
    }

    /// Asserts that a caret with no interval reports a lit phase, and
    /// that a zero interval does the same rather than dividing by zero.
    ///
    /// Case: the user turns blinking off with `blink_interval = 0`, and
    /// separately a caller builds a `CaretStyle` with a zero interval by
    /// hand.
    #[test]
    fn a_caret_that_does_not_blink_reports_a_lit_phase() {
        assert!(blink_phase_on(Duration::from_secs(1), None, None));
        assert!(blink_phase_on(
            Duration::from_secs(1),
            Some(Duration::ZERO),
            None
        ));
    }
}
