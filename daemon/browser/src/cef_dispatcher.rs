//! Abstraction over the cef dispatch transport. Production uses
//! `LiveCefDispatcher` which posts `CefCommand`s to the in-process CEF UI
//! thread. Tests use `StubCefDispatcher` to construct an `AppState` without
//! spinning up CEF.

use crate::cef_service::DispatchError;
use ozmux_browser_cef_protocol::wire::{BrowserUnavailableReason, HostCommand, HostEvent};
use tokio::sync::{broadcast, mpsc};

pub mod live;
pub mod stub;

/// Transport-agnostic interface used by the daemon's HTTP handlers and event
/// pump. Live implementations post `CefCommand`s to the in-process CEF UI
/// thread; stub implementations short-circuit every method so tests can
/// construct an `AppState` without spawning anything.
pub trait CefDispatcher: Send + Sync {
    /// Dispatches a `HostCommand` to the in-process CEF UI thread.
    fn dispatch(&self, cmd: HostCommand) -> Result<(), DispatchError>;

    /// Takes ownership of the event receiver. Returns `Some` on the first
    /// call; subsequent calls return `None`. Called exactly once by
    /// `spawn_event_pump`.
    fn events_take(&self) -> Option<mpsc::Receiver<HostEvent>>;

    /// Reads the dead flag. Always `false` for live in-process implementations;
    /// stubs may flip it to short-circuit browser endpoints with
    /// `BrowserUnavailable` in tests.
    fn is_dead(&self) -> bool;

    /// Subscribes to permanent-unavailable broadcasts. Reserved for future
    /// in-process crash handling; in-process dispatchers currently never
    /// publish on this channel.
    fn unavailable_subscribe(&self) -> broadcast::Receiver<BrowserUnavailableReason>;
}
