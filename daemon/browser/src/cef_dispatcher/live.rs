//! Production `CefDispatcher` that forwards to the out-of-process `cef_host`
//! via `CefHostHandles`. Plan 3 replaces this with an in-process variant.

use super::CefDispatcher;
use crate::cef_service::{CefHostHandles, DispatchError};
use ozmux_browser_cef_protocol::wire::{BrowserUnavailableReason, HostCommand, HostEvent};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tokio::sync::{broadcast, mpsc};

/// Live dispatcher backed by `CefHostHandles` (OoP cef_host child).
///
/// Holds the handles as `Arc<CefHostHandles>` so multiple dispatcher clones
/// share the same channel. `dispatch` posts via the handles' `commands`
/// `mpsc::Sender` from a blocking context using `try_send` to keep the trait
/// method synchronous (Plan 3 also wants sync, so this is forward-compatible).
pub struct LiveCefDispatcher {
    handles: Arc<CefHostHandles>,
}

impl LiveCefDispatcher {
    /// Wraps `handles` for use as a `CefDispatcher`.
    pub fn new(handles: Arc<CefHostHandles>) -> Self {
        Self { handles }
    }

    /// Exposes the underlying handles for callers that need methods not on the
    /// `CefDispatcher` trait surface (currently `request_browser_create` with
    /// its SCM_RIGHTS fd). Plan 3 removes these callers.
    pub fn handles(&self) -> &Arc<CefHostHandles> {
        &self.handles
    }
}

impl CefDispatcher for LiveCefDispatcher {
    fn dispatch(&self, cmd: HostCommand) -> Result<(), DispatchError> {
        if self.handles.is_dead() {
            return Err(DispatchError::Dead("cef_host marked dead".into()));
        }
        // NOTE: try_send keeps this trait method synchronous; callers do not
        // distinguish channel-full from channel-closed today, so both fold
        // into ChannelClosed.
        self.handles
            .commands
            .try_send(cmd)
            .map_err(|_| DispatchError::ChannelClosed)
    }

    fn events_take(&self) -> Option<mpsc::Receiver<HostEvent>> {
        self.handles.events_take()
    }

    fn is_dead(&self) -> bool {
        self.handles.is_dead()
    }

    fn is_dead_handle(&self) -> Arc<AtomicBool> {
        self.handles.is_dead_handle()
    }

    fn take_child(&self) -> Option<tokio::process::Child> {
        self.handles.take_child()
    }

    fn unavailable_subscribe(&self) -> broadcast::Receiver<BrowserUnavailableReason> {
        // NOTE: CefHostHandles itself does not own the broadcaster — it lives
        // on BrowserCefRegistry. For trait compliance we return a never-firing
        // receiver via a freshly-created broadcast::channel; the real
        // subscription path (BrowserCefRegistry::unavailable_subscribe) is
        // accessed by WS handlers directly. This method only exists because
        // the trait must close over "dead flag observability"; Plan 3 may
        // remove it.
        let (_, rx) = broadcast::channel(1);
        rx
    }
}
