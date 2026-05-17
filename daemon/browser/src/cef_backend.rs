//! Daemon-side glue for the cef BrowserActivity path. Sits between
//! `AppState::add_activity_to_pane` and the `CefHostHandles` +
//! `BrowserCefRegistry` pair.

use crate::cef_registry::BrowserCefRegistry;
use crate::cef_service::CefHostHandles;
use crate::frame_ring::FrameRing;
use crate::shm_alloc::{self, SLOT_PAYLOAD_MAX};
use crate::shm_reader::OwnedShmReader;
use ozmux_browser_cef_protocol::types::ActivityId as CefActivityId;
use ozmux_browser_cef_protocol::wire::{CefCookieDto, HostCommand};
use std::sync::Arc;

/// Errors returned by the cef provisioning hook.
#[derive(thiserror::Error, Debug)]
pub enum CefBackendError {
    /// `shm_alloc::create_shm_for_activity` failed.
    #[error("shm allocation failed: {0}")]
    ShmAlloc(std::io::Error),
    /// Mapping the shm region for daemon-side reading failed.
    #[error("shm mmap failed: {0}")]
    ShmMap(std::io::Error),
    /// `CefHostHandles::request_browser_create` reported a closed control channel.
    #[error("cef_host control channel closed: {0}")]
    ControlSendFailed(std::io::Error),
}

/// Pair of handles to the cef_host + ring registry used by the daemon side
/// to drive `BrowserCreate` lifecycle.
pub struct CefBackend {
    /// Handle to the running cef_host supervisor channels.
    pub handles: Arc<CefHostHandles>,
    /// Registry of per-activity `FrameRing`s.
    pub registry: Arc<BrowserCefRegistry>,
}

impl CefBackend {
    /// Allocates a per-activity shm region, registers a `FrameRing` in the
    /// registry, then dispatches `HostCommand::BrowserCreate` to cef_host with
    /// the shm fd as ancillary data via SCM_RIGHTS. Returns the epoch chosen
    /// for the ring (always 1 in Plan 2; respawn changes this in Plan 3).
    ///
    /// Cookies for `initial_url` are extracted from the host Chrome profile
    /// (macOS only) and forwarded inline in `BrowserCreate`. On failure the
    /// cookie list degrades to empty and a warning is logged so the browser
    /// still opens in an unauthenticated state (spec §4.6).
    pub async fn provision(
        &self,
        aid: &CefActivityId,
        initial_url: &str,
        _cookies: Vec<CefCookieDto>,
    ) -> Result<u32, CefBackendError> {
        let cookies = crate::cookie_extractor::extract_for(initial_url)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(error = %e, "cookie extraction failed; provisioning with no cookies");
                // TODO: Phase B follow-up — emit BrowserServerMsg::PageError after
                // BrowserReady so the user sees the degraded login state.
                Vec::new()
            });

        let shm_fd = shm_alloc::create_shm_for_activity(&aid.0, SLOT_PAYLOAD_MAX)
            .map_err(CefBackendError::ShmAlloc)?;
        // Map a read-only view before handing the fd to cef_host; the mapping
        // outlives the descriptor, so `shm_fd` can still be moved into the
        // SCM_RIGHTS send below. The event pump reads frames through this.
        let reader = Arc::new(
            OwnedShmReader::map(&shm_fd, SLOT_PAYLOAD_MAX).map_err(CefBackendError::ShmMap)?,
        );
        let epoch = 1;
        let ring = Arc::new(FrameRing::new(self.registry.session_id(), epoch));
        // NOTE: the returned nav receiver is discarded here; WS handlers subscribe
        // independently via registry.nav_subscribe() after BrowserCreate completes.
        let _nav_rx = self.registry.insert(aid.clone(), ring, reader);

        self.handles
            .request_browser_create(aid.clone(), initial_url.to_string(), epoch, cookies, shm_fd)
            .await
            .map_err(CefBackendError::ControlSendFailed)?;

        Ok(epoch)
    }

    /// Removes the FrameRing from the registry and tells cef_host to close
    /// the activity. Errors on the control channel are logged but not
    /// propagated — close is best-effort.
    pub async fn close(&self, aid: &CefActivityId) {
        self.registry.remove(aid);
        if let Err(e) = self
            .handles
            .send_command(HostCommand::Close { aid: aid.clone() })
            .await
        {
            tracing::warn!(?aid, error = %e, "cef_host Close failed");
        }
    }
}
