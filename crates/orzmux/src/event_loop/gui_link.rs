//! The backend's sending end of the GUI event channel, which wakes the GUI
//! after a batch that sent at least one event, and once more after it
//! disconnects.

use crate::backend::OrzmuxEvent;
use crossbeam_channel::{Receiver, Sender, unbounded};
use std::task::Waker;

/// The backend's link to the GUI: sends events, wakes the GUI after a
/// batch that sent at least one event, and when dropped disconnects the
/// event channel before a last wake, so the woken GUI observes the
/// disconnect.
///
/// # Invariants
///
/// Holds the only sender of its event channel.
pub(crate) struct GuiLink {
    events: Option<Sender<OrzmuxEvent>>,
    waker: Waker,
}

impl GuiLink {
    /// A link on a new event channel that wakes the GUI through `waker`,
    /// paired with the channel's receiving end for the GUI.
    pub fn channel(waker: Waker) -> (Self, Receiver<OrzmuxEvent>) {
        let (events, receiver) = unbounded();
        let link = Self {
            events: Some(events),
            waker,
        };
        (link, receiver)
    }

    /// Sends `batch` in order and wakes the GUI once when at least one
    /// event was sent. Returns `false` when a send finds the receiver
    /// gone.
    pub fn send_batch(&self, batch: impl IntoIterator<Item = OrzmuxEvent>) -> bool {
        let Some(events) = self.events.as_ref() else {
            return false;
        };
        let mut sent = false;
        let mut connected = true;
        for event in batch {
            if events.send(event).is_ok() {
                sent = true;
            } else {
                connected = false;
            }
        }
        if sent {
            self.waker.wake_by_ref();
        }
        connected
    }

    /// The number of events queued and not yet received by the GUI.
    pub fn depth(&self) -> usize {
        self.events.as_ref().map_or(0, Sender::len)
    }
}

impl Drop for GuiLink {
    fn drop(&mut self) {
        drop(self.events.take());
        self.waker.wake_by_ref();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::WakeCount;
    use crossbeam_channel::TryRecvError;
    use std::iter;
    use std::sync::{Arc, Mutex, OnceLock};
    use std::task::Wake;
    use std::thread;

    /// Records, at each wake, whether the watched receiver reported the
    /// disconnect.
    #[derive(Default)]
    struct DisconnectProbe {
        receiver: OnceLock<Receiver<OrzmuxEvent>>,
        seen: Mutex<Vec<bool>>,
    }

    impl DisconnectProbe {
        fn watch(&self, receiver: Receiver<OrzmuxEvent>) {
            self.receiver
                .set(receiver)
                .expect("the probe watches a single receiver");
        }

        fn seen(&self) -> Vec<bool> {
            self.seen.lock().unwrap().clone()
        }
    }

    impl Wake for DisconnectProbe {
        fn wake(self: Arc<Self>) {
            self.wake_by_ref();
        }

        fn wake_by_ref(self: &Arc<Self>) {
            let disconnected = self.receiver.get().is_some_and(|receiver| {
                matches!(receiver.try_recv(), Err(TryRecvError::Disconnected))
            });
            self.seen.lock().unwrap().push(disconnected);
        }
    }

    fn answer() -> OrzmuxEvent {
        OrzmuxEvent::SelectionText { text: None }
    }

    /// Asserts that a batch with events wakes the GUI exactly once and an
    /// empty batch does not wake it.
    ///
    /// Case: the backend flushes two events from one loop turn, and on the
    /// next turn has nothing to send.
    #[test]
    fn a_batch_with_events_wakes_once_and_an_empty_batch_does_not() {
        let wakes = Arc::new(WakeCount::default());
        let (link, receiver) = GuiLink::channel(Waker::from(Arc::clone(&wakes)));
        assert!(link.send_batch([answer(), answer()]));
        assert_eq!(wakes.get(), 1);
        assert_eq!(receiver.len(), 2);
        assert!(link.send_batch(iter::empty()));
        assert_eq!(wakes.get(), 1);
    }

    /// Asserts that a batch reports a receiver that is gone and sends no
    /// wake.
    ///
    /// Case: the GUI has shut down while the backend still had output to
    /// deliver.
    #[test]
    fn a_batch_reports_a_gone_receiver() {
        let wakes = Arc::new(WakeCount::default());
        let (link, receiver) = GuiLink::channel(Waker::from(Arc::clone(&wakes)));
        drop(receiver);
        assert!(!link.send_batch([answer()]));
        assert_eq!(wakes.get(), 0);
    }

    /// Asserts that dropping the link disconnects the channel before its
    /// last wake, so the woken GUI sees the disconnect.
    ///
    /// Case: the GUI drops its client at exit and the backend loop returns.
    #[test]
    fn dropping_the_link_disconnects_before_the_last_wake() {
        let probe = Arc::new(DisconnectProbe::default());
        let (link, receiver) = GuiLink::channel(Waker::from(Arc::clone(&probe)));
        probe.watch(receiver);
        drop(link);
        assert_eq!(probe.seen(), vec![true]);
    }

    /// Asserts that a link dropped while its thread unwinds from a panic
    /// still disconnects the channel before its last wake.
    ///
    /// Case: the backend thread panics mid-loop.
    #[test]
    fn a_panicking_thread_still_disconnects_before_the_last_wake() {
        let probe = Arc::new(DisconnectProbe::default());
        let (link, receiver) = GuiLink::channel(Waker::from(Arc::clone(&probe)));
        probe.watch(receiver);
        let outcome = thread::spawn(move || {
            let _link = link;
            panic!("injected backend failure");
        })
        .join();
        assert!(outcome.is_err());
        assert_eq!(probe.seen(), vec![true]);
    }
}
