//! Abstraction over the cef_host transport. Production uses
//! `LiveCefDispatcher` which delegates to `CefHostHandles` (currently an
//! out-of-process child; Plan 3 swaps this for in-process CEF). Tests use
//! `StubCefDispatcher` to construct an `AppState` without spinning up CEF.

use crate::cef_service::{CefHostHandles, DispatchError};
use ozmux_browser_cef_protocol::wire::{BrowserUnavailableReason, HostCommand, HostEvent};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tokio::sync::{broadcast, mpsc};

pub mod live;
pub mod stub;

/// Transport-agnostic interface used by the daemon's HTTP handlers and event
/// pump in place of touching `CefHostHandles` directly. Live implementations
/// forward to the real cef_host; stub implementations short-circuit every
/// method so tests can construct an `AppState` without spawning anything.
pub trait CefDispatcher: Send + Sync {
    /// Dispatches a `HostCommand` to the CEF host. Implementations may
    /// translate to in-process `CefCommand` (Plan 3) or forward over UDS
    /// (Phase 1-2 holdover).
    fn dispatch(&self, cmd: HostCommand) -> Result<(), DispatchError>;

    /// Takes ownership of the event receiver. Returns `Some` on the first
    /// call; subsequent calls return `None`. Called exactly once by
    /// `spawn_event_pump`.
    fn events_take(&self) -> Option<mpsc::Receiver<HostEvent>>;

    /// Reads the dead flag (set by the crash watcher; always `false` for
    /// in-process implementations). Used by HTTP handlers to short-circuit
    /// browser endpoints with `BrowserUnavailable` after a permanent failure.
    fn is_dead(&self) -> bool;

    /// Returns a clone of the dead-flag handle. Used by
    /// `spawn_cef_crash_watcher` in Phase 1-2 only; Plan 3 removes the watcher
    /// and this method becomes dead weight (still implemented for trait
    /// completeness).
    fn is_dead_handle(&self) -> Arc<AtomicBool>;

    /// Takes ownership of the child process handle for the crash watcher.
    /// Returns `Some` on the first call, `None` thereafter. In-process
    /// implementations (Plan 3) always return `None`.
    fn take_child(&self) -> Option<tokio::process::Child>;

    /// Subscribes to permanent-unavailable broadcasts (currently driven by
    /// the crash watcher; preserved for endpoint compatibility).
    fn unavailable_subscribe(&self) -> broadcast::Receiver<BrowserUnavailableReason>;

    /// Returns the underlying OoP handles, for callers that still need
    /// methods not on this trait (currently `request_browser_create` with
    /// its SCM_RIGHTS fd, and `Close` via `send_command`). Returns `None`
    /// for in-process or stub implementations; callers must treat that as
    /// "browser unavailable". Plan 3 removes the last caller and this
    /// method.
    fn handles(&self) -> Option<&Arc<CefHostHandles>>;
}
