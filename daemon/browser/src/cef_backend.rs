//! Daemon-side glue for the cef BrowserActivity path. Sits between
//! `AppState::add_activity_to_pane` and the in-process `CefDispatcher` +
//! `BrowserCefRegistry` pair.

use crate::cef_dispatcher::CefDispatcher;
use crate::cef_registry::BrowserCefRegistry;
use crate::cef_service::DispatchError;
use crate::frame_ring::FrameRing;
use ozmux_browser_cef_protocol::types::ActivityId as CefActivityId;
use ozmux_browser_cef_protocol::wire::{BrowserExtraContext, BrowserProfileWire, HostCommand};
use std::sync::Arc;

/// Errors returned by the cef provisioning hook.
#[derive(thiserror::Error, Debug)]
pub enum CefBackendError {
    /// `CefDispatcher::dispatch` reported a closed control channel.
    #[error("cef_host control channel closed: {0}")]
    ControlSendFailed(DispatchError),
}

/// Pair of handles to the cef dispatcher + ring registry used by the daemon
/// side to drive `BrowserCreate` lifecycle.
pub struct CefBackend {
    /// In-process `CefDispatcher` that posts commands to the CEF UI thread.
    pub dispatcher: Arc<dyn CefDispatcher>,
    /// Registry of per-activity `FrameRing`s.
    pub registry: Arc<BrowserCefRegistry>,
}

impl CefBackend {
    /// Registers a `FrameRing` in the registry, then dispatches
    /// `HostCommand::BrowserCreate` to the CEF UI thread. Returns the epoch
    /// chosen for the ring (always 1 in Plan 2; respawn changes this in
    /// Plan 3).
    ///
    /// Cookies for `initial_url` are extracted from the host Chrome profile
    /// (macOS only) and forwarded inline in `BrowserCreate`. On failure the
    /// cookie list degrades to empty and a warning is logged so the browser
    /// still opens in an unauthenticated state (spec §4.6).
    ///
    /// `profile` is forwarded verbatim to `HostCommand::BrowserCreate` and
    /// selects the embedded browser's storage profile.
    ///
    /// `context` is forwarded into CEF's `extra_info` so the render process
    /// can build `window.ozmux.context` synchronously in
    /// `on_context_created`.
    pub async fn provision(
        &self,
        aid: &CefActivityId,
        initial_url: &str,
        profile: BrowserProfileWire,
        context: BrowserExtraContext,
    ) -> Result<u32, CefBackendError> {
        let cookies = crate::cookie_extractor::extract_for(initial_url)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(error = %e, "cookie extraction failed; provisioning with no cookies");
                // TODO: Phase B follow-up — emit BrowserServerMsg::PageError after
                // BrowserReady so the user sees the degraded login state.
                Vec::new()
            });

        let epoch = 1;
        let ring = Arc::new(FrameRing::new(self.registry.session_id(), epoch));
        // NOTE: the returned nav receiver is discarded here; WS handlers subscribe
        // independently via registry.nav_subscribe() after BrowserCreate completes.
        let _nav_rx = self.registry.insert(aid.clone(), ring);

        self.dispatcher
            .dispatch(HostCommand::BrowserCreate {
                aid: aid.clone(),
                initial_url: initial_url.to_string(),
                epoch,
                cookies,
                profile,
                context,
            })
            .map_err(CefBackendError::ControlSendFailed)?;

        Ok(epoch)
    }

    /// Removes the FrameRing from the registry and tells cef_host to close
    /// the activity. Errors on the control channel are logged but not
    /// propagated — close is best-effort.
    pub async fn close(&self, aid: &CefActivityId) {
        self.registry.remove(aid);
        if let Err(e) = self
            .dispatcher
            .dispatch(HostCommand::Close { aid: aid.clone() })
        {
            tracing::warn!(?aid, error = %e, "cef_host Close dispatch failed");
        }
    }
}
