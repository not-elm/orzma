//! Production `CefDispatcher` that dispatches directly to in-process CEF via
//! `cef::post_task(ThreadId::UI, ExecuteTask)`. Plan 3 Phase 4: replaces the
//! Plan 1-2 OoP UDS implementation.

use super::CefDispatcher;
use crate::cef_service::{CefHostHandles, DispatchError};
use ozmux_browser_cef_protocol::wire::{BrowserUnavailableReason, HostCommand, HostEvent};
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
    unavailable_tx: broadcast::Sender<BrowserUnavailableReason>,
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

    fn is_dead_handle(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.is_dead)
    }

    fn take_child(&self) -> Option<tokio::process::Child> {
        None
    }

    fn unavailable_subscribe(&self) -> broadcast::Receiver<BrowserUnavailableReason> {
        self.unavailable_tx.subscribe()
    }

    fn handles(&self) -> Option<&Arc<CefHostHandles>> {
        // NOTE: in-process — no CefHostHandles exists. Plan 3 Task 13/14 removes
        // the last consumer of `handles()` via the in-process create_browser path.
        None
    }
}

/// Converts a daemon-facing `HostCommand` into the cef_host-internal
/// `CefCommand`. `BrowserCreate` is **not** routed through here — it must go
/// via a dedicated path (Plan 3 Task 12) so the shm fd and cookies are taken
/// in-process rather than reconstructed from wire bytes.
fn into_cef_command(cmd: HostCommand) -> CefCommand {
    match cmd {
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
        HostCommand::BrowserCreate { .. } => {
            // NOTE: BrowserCreate carries an SCM_RIGHTS shm fd in the OoP path;
            // in-process it must take a real RawFd via a dedicated path (Plan 3
            // Task 12). Reaching this branch via `dispatch()` is a bug.
            tracing::error!(
                "HostCommand::BrowserCreate routed through dispatch(); use the in-process create_browser path instead"
            );
            CefCommand::Noop
        }
        HostCommand::Ready { .. } => {
            // Handshake artifact — meaningless in-process.
            CefCommand::Noop
        }
        // TODO: implement RecreateShm, GetSelection, SetClipboard for the
        // in-process backend. cef_host control.rs currently treats these
        // as unimplemented; we mirror that until Plan 3 wires them up.
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
