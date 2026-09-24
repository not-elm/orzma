//! The handles that wake the app's winit event loop from other threads.

use bevy::prelude::*;
use bevy::winit::{EventLoopProxy, EventLoopProxyWrapper, WinitUserEvent};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Wake, Waker};

/// The app's wake handles, built once on the winit event loop.
pub(crate) struct AppWakers {
    input: Waker,
    timer: Waker,
    gate: WakeGate,
}

impl AppWakers {
    /// Builds the wakers on `world`'s winit event loop, or `None` when the
    /// app has no winit event loop.
    pub fn new(world: &World) -> Option<Self> {
        let wrapper = world.get_resource::<EventLoopProxyWrapper>()?;
        let proxy: EventLoopProxy<WinitUserEvent> = (**wrapper).clone();
        let gate = WakeGate(Arc::new(AtomicBool::new(false)));
        let input = Waker::from(Arc::new(ProxyWake {
            proxy: proxy.clone(),
            gate: Some(gate.clone()),
        }));
        let timer = Waker::from(Arc::new(ProxyWake { proxy, gate: None }));
        Some(Self { input, timer, gate })
    }

    /// The waker for outside input.
    ///
    /// Callers wake it after every item they queue for the app. While a wake
    /// is pending, further wakes send nothing, and the update such a wake
    /// runs gets a follow-up frame.
    pub fn input(&self) -> &Waker {
        &self.input
    }

    /// The waker for scheduled repaints.
    ///
    /// Its wakes are never coalesced, and the update such a wake runs gets
    /// no follow-up frame.
    pub fn timer(&self) -> &Waker {
        &self.timer
    }

    /// The pending-wake flag the input waker sets.
    pub fn gate(&self) -> &WakeGate {
        &self.gate
    }
}

/// Whether an input wake is pending.
///
/// `take` must run before the queues the wakes announce are drained.
#[derive(Resource, Clone)]
pub(crate) struct WakeGate(Arc<AtomicBool>);

impl WakeGate {
    /// Marks a wake pending. Returns `true` when none was, so the caller
    /// sends the wake.
    pub fn arm(&self) -> bool {
        !self.0.swap(true, Ordering::AcqRel)
    }

    /// Clears the pending wake. Returns whether one was pending.
    pub fn take(&self) -> bool {
        self.0.swap(false, Ordering::AcqRel)
    }

    /// A gate connected to no waker, for tests that drive `arm` and `take`
    /// directly.
    #[cfg(test)]
    pub fn detached() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }
}

/// Wakes the winit event loop, sending only when `gate` is absent or arms.
struct ProxyWake {
    proxy: EventLoopProxy<WinitUserEvent>,
    gate: Option<WakeGate>,
}

impl Wake for ProxyWake {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        if self.gate.as_ref().is_some_and(|gate| !gate.arm()) {
            return;
        }
        if let Err(error) = self.proxy.send_event(WinitUserEvent::WakeUp) {
            debug!(%error, "the event loop is gone; wake dropped");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    /// Asserts that only the first arm asks for a wake, and that `take`
    /// reports and clears the pending wake.
    ///
    /// Case: PTY output arrives twice before the app's next update starts.
    #[test]
    fn only_the_first_arm_sends_until_the_gate_is_taken() {
        let gate = WakeGate::detached();
        assert!(gate.arm());
        assert!(!gate.arm());
        assert!(gate.take());
        assert!(!gate.take());
    }

    /// Asserts that a wake armed after the gate was taken asks to be sent
    /// again.
    ///
    /// Case: output arrives after an update took the gate but before its
    /// drain ran.
    #[test]
    fn a_wake_armed_after_take_is_sent_again() {
        let gate = WakeGate::detached();
        assert!(gate.arm());
        assert!(gate.take());
        assert!(gate.arm());
    }

    /// Asserts that among many threads arming the gate at once, exactly one
    /// is told to send the wake.
    ///
    /// Case: a flood of PTY output and a burst of control-socket requests
    /// wake the app from several threads in the same instant.
    #[test]
    fn concurrent_arms_send_exactly_one_wake() {
        let gate = WakeGate::detached();
        let arms: Vec<_> = (0..8)
            .map(|_| {
                let gate = gate.clone();
                thread::spawn(move || gate.arm())
            })
            .collect();
        let sends = arms
            .into_iter()
            .map(|arm| arm.join().expect("an arming thread"))
            .filter(|sent| *sent)
            .count();
        assert_eq!(sends, 1);
    }
}
