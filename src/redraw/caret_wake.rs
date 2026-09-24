//! Wakes the app when a painted caret next changes its blink phase.

use bevy::prelude::*;
use bevy_orzma_tty_renderer::prelude::NextCaretFlip;
use crossbeam_channel::{Receiver, RecvTimeoutError, SendError, Sender, unbounded};
use std::io;
use std::task::Waker;
use std::thread;
use std::time::Instant;

/// Wakes the app for the next blink-phase change of a painted caret.
pub(super) struct CaretWakePlugin {
    waker: Waker,
}

impl CaretWakePlugin {
    /// Builds the plugin around `waker`, which wakes the app for a scheduled
    /// repaint.
    pub fn new(waker: Waker) -> Self {
        Self { waker }
    }
}

impl Plugin for CaretWakePlugin {
    fn build(&self, app: &mut App) {
        match WakeTimer::spawn(self.waker.clone()) {
            Ok(timer) => {
                app.insert_resource(timer);
            }
            Err(error) => {
                error!(%error, "caret wake timer failed to start; the caret will not blink while idle");
            }
        }
        app.add_systems(
            Last,
            schedule_caret_wake
                .run_if(resource_exists::<WakeTimer>)
                .run_if(resource_exists_and_changed::<NextCaretFlip>),
        );
    }
}

/// A thread (named `orzma-wake-timer`) that wakes the app at one
/// scheduled instant. A new schedule replaces the previous one, and
/// `None` cancels it. The thread ends when this resource is dropped.
#[derive(Resource)]
struct WakeTimer {
    deadlines: Sender<Option<Instant>>,
}

impl WakeTimer {
    /// Starts the timer thread, which wakes `waker` at each deadline.
    ///
    /// # Errors
    ///
    /// Returns the OS error when the thread cannot be started.
    fn spawn(waker: Waker) -> io::Result<Self> {
        let (deadlines, schedule) = unbounded();
        thread::Builder::new()
            .name("orzma-wake-timer".to_string())
            .spawn(move || run_wake_timer(&schedule, &waker))?;
        Ok(Self { deadlines })
    }

    /// Schedules the next wake, replacing any earlier one; `None` cancels it.
    ///
    /// # Errors
    ///
    /// Returns the deadline when the timer thread is gone.
    fn schedule(&self, deadline: Option<Instant>) -> Result<(), SendError<Option<Instant>>> {
        self.deadlines.send(deadline)
    }
}

/// Converts the next caret flip into an `Instant` and hands it to the
/// timer.
fn schedule_caret_wake(timer: Res<WakeTimer>, flip: Res<NextCaretFlip>, time: Res<Time<Real>>) {
    let deadline = flip
        .at()
        .zip(time.first_update())
        .and_then(|(flip, first)| first.checked_add(flip));
    if let Err(error) = timer.schedule(deadline) {
        warn!(%error, "caret wake timer is gone; the caret will not blink while idle");
    }
}

/// Waits for the scheduled deadline and wakes `waker` when it passes,
/// until the scheduling side hangs up.
fn run_wake_timer(schedule: &Receiver<Option<Instant>>, waker: &Waker) {
    let mut deadline = None;
    loop {
        let next = match deadline {
            Some(at) => schedule.recv_deadline(at),
            None => schedule.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match next {
            Ok(scheduled) => deadline = scheduled,
            Err(RecvTimeoutError::Timeout) => {
                waker.wake_by_ref();
                deadline = None;
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::time::TimeUpdateStrategy;
    use std::sync::Arc;
    use std::task::Wake;
    use std::time::Duration;

    /// Sends the instant of each wake to the test.
    struct ChannelWake(Sender<Instant>);

    impl Wake for ChannelWake {
        fn wake(self: Arc<Self>) {
            self.wake_by_ref();
        }

        fn wake_by_ref(self: &Arc<Self>) {
            let _ = self.0.send(Instant::now());
        }
    }

    fn channel_waker() -> (Waker, Receiver<Instant>) {
        let (sender, receiver) = unbounded();
        (Waker::from(Arc::new(ChannelWake(sender))), receiver)
    }

    /// Asserts that the timer wakes the app once, no earlier than the
    /// deadline.
    ///
    /// Case: the caret's next blink change is 50 ms away and nothing else
    /// happens.
    #[test]
    fn the_timer_wakes_once_the_deadline_passes() {
        let (waker, woke) = channel_waker();
        let timer = WakeTimer::spawn(waker).expect("the timer thread starts");
        let deadline = Instant::now() + Duration::from_millis(50);
        timer
            .schedule(Some(deadline))
            .expect("the timer thread is alive");
        let at = woke.recv_timeout(Duration::from_secs(2)).expect("a wake");
        assert!(at >= deadline);
        assert!(woke.recv_timeout(Duration::from_millis(200)).is_err());
    }

    /// Asserts that a new schedule replaces the earlier one.
    ///
    /// Case: the user types just before the caret's next blink change,
    /// which reschedules it.
    #[test]
    fn a_new_schedule_replaces_the_earlier_one() {
        let (waker, woke) = channel_waker();
        let timer = WakeTimer::spawn(waker).expect("the timer thread starts");
        let now = Instant::now();
        let replaced = now + Duration::from_secs(1);
        let replacement = now + Duration::from_millis(100);
        timer
            .schedule(Some(replaced))
            .expect("the timer thread is alive");
        timer
            .schedule(Some(replacement))
            .expect("the timer thread is alive");
        let at = woke.recv_timeout(Duration::from_secs(2)).expect("a wake");
        assert!(at >= replacement && at < replaced);
        assert!(woke.recv_timeout(Duration::from_millis(1200)).is_err());
    }

    /// Asserts that scheduling `None` cancels the pending wake.
    ///
    /// Case: the user switches to another application, so the caret stops
    /// blinking before its next change.
    #[test]
    fn none_cancels_the_pending_wake() {
        let (waker, woke) = channel_waker();
        let timer = WakeTimer::spawn(waker).expect("the timer thread starts");
        timer
            .schedule(Some(Instant::now() + Duration::from_millis(500)))
            .expect("the timer thread is alive");
        timer.schedule(None).expect("the timer thread is alive");
        assert!(woke.recv_timeout(Duration::from_millis(800)).is_err());
    }

    /// Asserts that a deadline already in the past wakes the app once, right
    /// away.
    ///
    /// Case: a long frame overran the caret's next blink change before the
    /// change was scheduled.
    #[test]
    fn a_past_deadline_wakes_once_right_away() {
        let (waker, woke) = channel_waker();
        let timer = WakeTimer::spawn(waker).expect("the timer thread starts");
        let past = Instant::now()
            .checked_sub(Duration::from_millis(50))
            .expect("the clock reads past 50 ms");
        timer
            .schedule(Some(past))
            .expect("the timer thread is alive");
        woke.recv_timeout(Duration::from_secs(2))
            .expect("a prompt wake");
        assert!(woke.recv_timeout(Duration::from_millis(200)).is_err());
    }

    /// Asserts that a changed flip reading is scheduled as `first_update`
    /// plus the reading.
    ///
    /// Case: the renderer publishes the caret's next blink change at 750 ms
    /// after the first update.
    #[test]
    fn a_changed_flip_is_scheduled_from_the_first_update() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
                10,
            )))
            .init_resource::<NextCaretFlip>()
            .add_plugins(CaretWakePlugin::new(Waker::noop().clone()));
        let (deadlines, scheduled) = unbounded();
        app.insert_resource(WakeTimer { deadlines });
        app.update();
        assert_eq!(scheduled.try_recv(), Ok(None));

        *app.world_mut().resource_mut::<NextCaretFlip>() =
            NextCaretFlip::new(Some(Duration::from_millis(750)));
        app.update();
        let first = app
            .world()
            .resource::<Time<Real>>()
            .first_update()
            .expect("an update ran");
        assert_eq!(
            scheduled.try_recv(),
            Ok(Some(first + Duration::from_millis(750)))
        );
    }
}
