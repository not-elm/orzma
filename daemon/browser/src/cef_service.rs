//! Daemon-side cef host event pump.
//!
//! `CefHostHandles` / `CefHostSupervisor` / the UDS pump / SHM transport were
//! retired together with the out-of-process `cef_host` binary in Plan 3
//! Phase 5. The remaining surface is the `DispatchError` enum (shared by
//! every `CefDispatcher` implementation) and the `spawn_event_pump` helper
//! that drains `HostEvent`s from the in-process dispatcher into the
//! per-activity `BrowserCefRegistry`.

use crate::cef_registry::{BrowserCefRegistry, NavState};
use crate::frame_ring::FrameEnvelope;
use ozmux_browser_cef_protocol::wire::HostEvent;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Error returned when a `HostCommand` cannot be delivered to the CEF UI
/// thread.
#[derive(thiserror::Error, Debug)]
pub enum DispatchError {
    /// The CEF command channel has been closed (CEF shut down or the
    /// dispatcher was dropped).
    #[error("cef command channel closed")]
    ChannelClosed,
    /// The CEF host is marked permanently unavailable.
    #[error("cef_host marked dead: {0}")]
    Dead(String),
}

/// Spawns a task that drains `HostEvent`s from the dispatcher's event channel
/// and routes them to per-activity sinks on `BrowserCefRegistry`.
///
/// Takes the event receiver out of `dispatcher` via
/// [`CefDispatcher::events_take`](crate::cef_dispatcher::CefDispatcher::events_take)
/// exactly once. Returns the `JoinHandle` so the caller (daemon_bootstrap) can
/// hold it for the process lifetime; dropping the handle cancels the task.
///
/// # Panics
///
/// Panics if `dispatcher.events_take()` returns `None` (i.e. the receiver was
/// already consumed by a previous call or by a test).
pub fn spawn_event_pump(
    dispatcher: Arc<dyn crate::cef_dispatcher::CefDispatcher>,
    registry: Arc<BrowserCefRegistry>,
) -> tokio::task::JoinHandle<()> {
    let events = dispatcher.events_take().expect(
        "spawn_event_pump: events receiver already consumed; call this function exactly once",
    );
    tokio::spawn(event_pump_loop(events, registry))
}

async fn event_pump_loop(mut events: mpsc::Receiver<HostEvent>, registry: Arc<BrowserCefRegistry>) {
    loop {
        let Some(ev) = events.recv().await else {
            tracing::debug!("cef_host event channel closed; pump exiting");
            break;
        };
        match ev {
            HostEvent::NavStateChanged {
                aid,
                url,
                title,
                can_back,
                can_forward,
            } => {
                let next = NavState {
                    url,
                    title,
                    can_back,
                    can_forward,
                };
                if let Err(e) = registry.nav_publish(&aid, next) {
                    tracing::debug!(error = %e, "nav_publish: aid not in registry");
                }
            }
            HostEvent::TitleChanged { aid, title } => {
                if let Some(mut current) = registry.nav_current(&aid) {
                    current.title = title;
                    if let Err(e) = registry.nav_publish(&aid, current) {
                        tracing::debug!(error = %e, "nav_publish (TitleChanged): aid not in registry");
                    }
                }
            }
            HostEvent::CursorChanged { aid, cursor } => {
                if let Err(e) = registry.cursor_publish(&aid, cursor) {
                    tracing::debug!(error = %e, "cursor_publish: aid not in registry");
                }
            }
            // In-process screencast frame (Plan 3 Task 11+12): the cef_host
            // render handler already copied the BGRA payload through
            // `FrameBufferPool`, so we just wrap the fields in a `FrameEnvelope`
            // and push into the per-activity ring.
            HostEvent::FrameProduced {
                aid,
                session_id,
                epoch,
                frame_seq,
                captured_at_us,
                width,
                height,
                is_keyframe,
                damage_rects,
                is_popup,
                bgra,
            } => {
                let Some(ring) = registry.frame_ring(&aid) else {
                    tracing::debug!(aid = %aid.0, "FrameProduced: aid not in registry");
                    continue;
                };
                ring.push(Arc::new(FrameEnvelope {
                    session_id,
                    epoch,
                    frame_seq,
                    captured_at_us,
                    width,
                    height,
                    is_keyframe,
                    damage_rects,
                    is_popup,
                    bgra,
                }));
            }
            // NOTE: BrowserReady is consumed by integration tests via direct
            // receiver access — no-op here in the production pump.
            HostEvent::BrowserReady { .. } => {}
            // NOTE: remaining variants are informational or deferred to later tasks.
            HostEvent::SelectionChanged { .. }
            | HostEvent::PageError { .. }
            | HostEvent::RenderProcessTerminated { .. }
            | HostEvent::LogLine { .. }
            | HostEvent::Crashed { .. } => {
                tracing::debug!(?ev, "event pump: unhandled HostEvent variant");
            }
        }
    }
}
