//! The lit and dark phases a blinking caret follows after a keystroke.

use std::time::Duration;

/// Whether the blink phase is lit `elapsed` after the last keystroke.
///
/// Reports `true` once `timeout` has passed, and for a zero `interval`.
/// A `None` timeout blinks indefinitely.
pub fn blink_phase_on(elapsed: Duration, interval: Duration, timeout: Option<Duration>) -> bool {
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
        assert!(blink_phase_on(Duration::ZERO, interval, timeout));
        assert!(!blink_phase_on(
            Duration::from_millis(750),
            interval,
            timeout
        ));
        assert!(blink_phase_on(
            Duration::from_millis(1500),
            interval,
            timeout
        ));
        assert!(blink_phase_on(Duration::from_secs(5), interval, timeout));
        assert!(blink_phase_on(Duration::from_secs(600), interval, timeout));
    }

    /// Asserts that a `None` timeout blinks indefinitely rather than
    /// settling.
    ///
    /// Case: the user sets `blink_timeout = 0` and walks away.
    #[test]
    fn a_none_timeout_keeps_blinking() {
        let interval = Duration::from_millis(750);
        assert!(!blink_phase_on(Duration::from_millis(750), interval, None));
        assert!(!blink_phase_on(
            Duration::from_secs(6000) + Duration::from_millis(750),
            interval,
            None
        ));
    }

    /// Asserts that a zero interval does not divide by zero, reporting a
    /// lit phase instead.
    ///
    /// Case: a caller passes an unclamped interval straight from a
    /// malformed config.
    #[test]
    fn a_zero_interval_reports_a_lit_phase() {
        assert!(blink_phase_on(Duration::from_secs(1), Duration::ZERO, None));
    }
}
