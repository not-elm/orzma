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

/// The time since the last keystroke at which the blink phase next
/// changes, or `None` when it never changes again.
///
/// The phase changes on each whole multiple of `interval` before
/// `timeout`, and at `timeout` itself when the caret is dark just before
/// it, since the phase settles lit there. Reports `None` for a caret that
/// does not blink, for a zero `interval`, and once no change remains.
pub fn next_blink_flip(
    elapsed: Duration,
    interval: Option<Duration>,
    timeout: Option<Duration>,
) -> Option<Duration> {
    let interval_ms = interval?.as_millis();
    if interval_ms == 0 {
        return None;
    }
    if timeout.is_some_and(|timeout| elapsed >= timeout) {
        return None;
    }
    let boundary_ms = (elapsed.as_millis() / interval_ms + 1).checked_mul(interval_ms)?;
    let boundary = Duration::from_millis(u64::try_from(boundary_ms).ok()?);
    match timeout {
        Some(timeout) if boundary >= timeout => {
            let before_timeout = timeout.saturating_sub(Duration::from_millis(1));
            let dark = !blink_phase_on(before_timeout, interval, None);
            dark.then_some(timeout)
        }
        _ => Some(boundary),
    }
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

    /// Asserts that the next phase change is the next whole multiple of the
    /// interval after the time since the keystroke.
    ///
    /// Case: the user types, and the caret blinks at the default 750 ms
    /// interval.
    #[test]
    fn the_next_flip_is_the_next_interval_boundary() {
        let interval = Some(Duration::from_millis(750));
        let timeout = Some(Duration::from_secs(5));
        let flip =
            |elapsed_ms| next_blink_flip(Duration::from_millis(elapsed_ms), interval, timeout);
        assert_eq!(flip(0), Some(Duration::from_millis(750)));
        assert_eq!(flip(749), Some(Duration::from_millis(750)));
        assert_eq!(flip(750), Some(Duration::from_millis(1500)));
        assert_eq!(flip(751), Some(Duration::from_millis(1500)));
        assert_eq!(flip(4400), Some(Duration::from_millis(4500)));
    }

    /// Asserts that the timeout is reported as a change only when the
    /// caret is dark just before it.
    ///
    /// Case: the user stops typing and the caret's last blink runs into the
    /// blink timeout.
    #[test]
    fn the_timeout_is_a_flip_only_when_the_caret_is_dark_before_it() {
        let interval = Some(Duration::from_millis(750));
        let lit_before = Some(Duration::from_secs(5));
        assert_eq!(
            next_blink_flip(Duration::from_millis(4600), interval, lit_before),
            None
        );
        let dark_before = Some(Duration::from_millis(4000));
        assert_eq!(
            next_blink_flip(Duration::from_millis(3800), interval, dark_before),
            Some(Duration::from_millis(4000))
        );
    }

    /// Asserts that no change remains for a steady caret, a zero or
    /// sub-millisecond interval, or once the timeout has passed.
    ///
    /// Case: the user turns blinking off, and separately leaves the keyboard
    /// alone past the blink timeout.
    #[test]
    fn no_flip_remains_for_a_steady_caret_or_after_the_timeout() {
        let timeout = Some(Duration::from_secs(5));
        assert_eq!(next_blink_flip(Duration::ZERO, None, timeout), None);
        assert_eq!(
            next_blink_flip(Duration::ZERO, Some(Duration::ZERO), timeout),
            None
        );
        assert_eq!(
            next_blink_flip(Duration::ZERO, Some(Duration::from_micros(500)), timeout),
            None
        );
        let interval = Some(Duration::from_millis(750));
        assert_eq!(
            next_blink_flip(Duration::from_secs(5), interval, timeout),
            None
        );
        assert_eq!(
            next_blink_flip(Duration::from_secs(6), interval, timeout),
            None
        );
    }

    /// Asserts that a caret without a timeout keeps reporting the next
    /// boundary.
    ///
    /// Case: the user sets `blink_timeout = 0` and walks away.
    #[test]
    fn a_none_timeout_keeps_reporting_the_next_boundary() {
        assert_eq!(
            next_blink_flip(
                Duration::from_secs(6000),
                Some(Duration::from_millis(750)),
                None
            ),
            Some(Duration::from_millis(6_000_750))
        );
    }
}
