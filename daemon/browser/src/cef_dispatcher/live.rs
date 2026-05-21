//! Production `CefDispatcher` that dispatches directly to in-process CEF via
//! `cef::post_task(ThreadId::UI, ExecuteTask)`.

use super::CefDispatcher;
use crate::cef_service::DispatchError;
use ozmux_browser_cef_protocol::types::ActivityId;
use ozmux_browser_cef_protocol::wire::{
    BrowserUnavailableEvent, BrowserUnavailableReason, HostCommand, HostEvent,
};
use ozmux_cef_host::pool::CefCommand;
use ozmux_cef_host::post_command::{PoolHandle, post as cef_post};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use tokio::sync::{broadcast, mpsc};

/// In-process dispatcher. Holds a `PoolHandle` for command dispatch and an
/// event channel receiver fed by CEF callbacks (`RenderHandler::on_paint`
/// etc.) via the `event_tx` planted into the `BrowserPool` at construction.
pub struct LiveCefDispatcher {
    pool_handle: PoolHandle,
    events: Mutex<Option<mpsc::Receiver<HostEvent>>>,
    is_dead: Arc<AtomicBool>,
    unavailable_tx: broadcast::Sender<BrowserUnavailableEvent>,
}

impl LiveCefDispatcher {
    /// Constructs a live dispatcher from a `PoolHandle` and an event receiver.
    ///
    /// CEF UI callbacks push `HostEvent` into the matching sender that fed
    /// `BrowserPool::new`. The receiver passed here is the read end of that
    /// channel, surfaced through `events_take` for the daemon's event pump.
    pub fn new(pool_handle: PoolHandle, events: mpsc::Receiver<HostEvent>) -> Self {
        let (unavailable_tx, _) = broadcast::channel(16);
        Self {
            pool_handle,
            events: Mutex::new(Some(events)),
            is_dead: Arc::new(AtomicBool::new(false)),
            unavailable_tx,
        }
    }

    /// Publishes a `BrowserUnavailable` signal to all WS subscribers.
    ///
    /// `aid = Some(_)` scopes the event to a single activity (e.g. an
    /// extension UDS disconnect); `aid = None` indicates a daemon-wide
    /// outage that every browser subscriber should react to.
    pub fn mark_unavailable(&self, aid: Option<ActivityId>, reason: BrowserUnavailableReason) {
        let ev = BrowserUnavailableEvent { aid, reason };
        if self.unavailable_tx.send(ev).is_err() {
            // No live subscribers — informational only, not a fault.
            tracing::debug!(
                "LiveCefDispatcher::mark_unavailable: no active subscribers, dropping event"
            );
        }
    }
}

impl CefDispatcher for LiveCefDispatcher {
    fn dispatch(&self, cmd: HostCommand) -> Result<(), DispatchError> {
        let cef_cmd = into_cef_command(cmd);
        cef_post(&self.pool_handle, cef_cmd).map_err(|_| DispatchError::ChannelClosed)
    }

    fn events_take(&self) -> Option<mpsc::Receiver<HostEvent>> {
        self.events.lock().expect("events poisoned").take()
    }

    fn is_dead(&self) -> bool {
        self.is_dead.load(std::sync::atomic::Ordering::Acquire)
    }

    fn unavailable_subscribe(&self) -> broadcast::Receiver<BrowserUnavailableEvent> {
        self.unavailable_tx.subscribe()
    }
}

/// Converts a daemon-facing `HostCommand` into the cef_host-internal
/// `CefCommand`. `BrowserCreate` is routed straight through to
/// `CefCommand::BrowserCreate`; the in-process path uses
/// `FrameBufferPool`/`HostEvent::FrameProduced` for screencast frames so no
/// shm fd is required (Plan 3 Phase 5).
fn into_cef_command(cmd: HostCommand) -> CefCommand {
    match cmd {
        HostCommand::BrowserCreate {
            aid,
            initial_url,
            epoch,
            cookies,
            profile,
            context,
        } => CefCommand::BrowserCreate {
            aid,
            initial_url,
            epoch,
            cookies,
            profile,
            context,
        },
        HostCommand::Navigate { aid, url } => CefCommand::Navigate { aid, url },
        HostCommand::NavigateHistory { aid, delta } => CefCommand::NavigateHistory { aid, delta },
        HostCommand::SendInput { aid, input } => CefCommand::SendInput { aid, event: input },
        HostCommand::Resize {
            aid,
            css_w,
            css_h,
            dpr,
        } => CefCommand::Resize {
            aid,
            css_w,
            css_h,
            dpr,
        },
        HostCommand::PauseScreencast { aid } => CefCommand::PauseScreencast { aid },
        HostCommand::ResumeScreencast { aid } => CefCommand::ResumeScreencast { aid },
        HostCommand::Close { aid } => CefCommand::Close { aid },
        HostCommand::Shutdown => CefCommand::Shutdown,
        // TODO: implement RecreateShm, GetSelection, SetClipboard for the
        // in-process backend.
        HostCommand::RecreateShm { .. } => {
            tracing::warn!("unimplemented HostCommand::RecreateShm; ignoring");
            CefCommand::Noop
        }
        HostCommand::GetSelection { .. } => {
            tracing::warn!("unimplemented HostCommand::GetSelection; ignoring");
            CefCommand::Noop
        }
        HostCommand::SetClipboard { .. } => {
            tracing::warn!("unimplemented HostCommand::SetClipboard; ignoring");
            CefCommand::Noop
        }
    }
}
