//! Test-only `CefDispatcher` that short-circuits every method as if cef_host
//! was permanently dead. Allows constructing `AppState` for unit/integration
//! tests of `http_server` without spawning cef_host.

use super::CefDispatcher;
use crate::cef_service::DispatchError;
use ozmux_browser_cef_protocol::wire::{BrowserUnavailableEvent, HostCommand, HostEvent};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{broadcast, mpsc};

/// `CefDispatcher` whose `is_dead` always returns `true`. Every `dispatch`
/// call returns `DispatchError::Dead`; `events_take` returns a receiver that
/// never fires.
pub struct StubCefDispatcher {
    is_dead: Arc<AtomicBool>,
    events: std::sync::Mutex<Option<mpsc::Receiver<HostEvent>>>,
    unavailable_tx: broadcast::Sender<BrowserUnavailableEvent>,
}

impl StubCefDispatcher {
    /// Creates a dead stub. `events_take` returns `Some` on first call (an
    /// empty receiver) so a real `spawn_event_pump` can run against it
    /// without panicking.
    pub fn dead() -> Self {
        let (_event_tx, event_rx) = mpsc::channel::<HostEvent>(1);
        let (unavailable_tx, _) = broadcast::channel(4);
        Self {
            is_dead: Arc::new(AtomicBool::new(true)),
            events: std::sync::Mutex::new(Some(event_rx)),
            unavailable_tx,
        }
    }

    /// Creates an alive stub. `is_dead` returns `false`; `dispatch` silently
    /// drops commands and returns Ok. Useful for tests that need browser
    /// endpoints to return non-`BrowserUnavailable` errors so the rest of the
    /// flow can be exercised.
    pub fn alive() -> Self {
        let mut me = Self::dead();
        me.is_dead = Arc::new(AtomicBool::new(false));
        me
    }
}

impl CefDispatcher for StubCefDispatcher {
    fn dispatch(&self, _cmd: HostCommand) -> Result<(), DispatchError> {
        if self.is_dead.load(Ordering::Acquire) {
            Err(DispatchError::Dead("stub dispatcher dead".into()))
        } else {
            Ok(())
        }
    }

    fn events_take(&self) -> Option<mpsc::Receiver<HostEvent>> {
        self.events.lock().expect("stub events poisoned").take()
    }

    fn is_dead(&self) -> bool {
        self.is_dead.load(Ordering::Acquire)
    }

    fn unavailable_subscribe(&self) -> broadcast::Receiver<BrowserUnavailableEvent> {
        self.unavailable_tx.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ozmux_browser_cef_protocol::types::ActivityId;

    #[test]
    fn dead_stub_short_circuits_dispatch() {
        let stub = StubCefDispatcher::dead();
        let err = stub
            .dispatch(HostCommand::Navigate {
                aid: ActivityId("a".into()),
                url: "about:blank".into(),
            })
            .unwrap_err();
        assert!(matches!(err, DispatchError::Dead(_)));
        assert!(stub.is_dead());
    }

    #[test]
    fn alive_stub_drops_silently() {
        let stub = StubCefDispatcher::alive();
        assert!(!stub.is_dead());
        let ok = stub.dispatch(HostCommand::Navigate {
            aid: ActivityId("a".into()),
            url: "about:blank".into(),
        });
        assert!(ok.is_ok());
    }

    #[test]
    fn events_take_returns_some_once() {
        let stub = StubCefDispatcher::dead();
        assert!(stub.events_take().is_some());
        assert!(stub.events_take().is_none());
    }
}
