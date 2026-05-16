//! Daemon-side glue for the cef BrowserActivity path. Sits between
//! `AppState::add_activity_to_pane` and the `CefHostHandles` +
//! `BrowserCefRegistry` pair.

use crate::cef_registry::BrowserCefRegistry;
use crate::cef_service::CefHostHandles;
use crate::frame_ring::FrameRing;
use crate::shm_alloc::{self, POC_SLOT_PAYLOAD_MAX};
use ozmux_browser_cef_protocol::types::ActivityId as CefActivityId;
use ozmux_browser_cef_protocol::wire::{CefCookieDto, HostCommand};
use std::sync::Arc;

/// Errors returned by the cef provisioning hook.
#[derive(thiserror::Error, Debug)]
pub enum CefBackendError {
    /// `shm_alloc::create_shm_for_activity` failed.
    #[error("shm allocation failed: {0}")]
    ShmAlloc(std::io::Error),
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
    pub async fn provision(
        &self,
        aid: &CefActivityId,
        initial_url: &str,
        cookies: Vec<CefCookieDto>,
    ) -> Result<u32, CefBackendError> {
        let shm_fd = shm_alloc::create_shm_for_activity(&aid.0, POC_SLOT_PAYLOAD_MAX)
            .map_err(CefBackendError::ShmAlloc)?;
        let epoch = 1;
        let ring = Arc::new(FrameRing::new(self.registry.session_id(), epoch));
        // NOTE: the returned nav receiver is discarded here; WS handlers subscribe
        // independently via registry.nav_subscribe() after BrowserCreate completes.
        let _nav_rx = self.registry.insert(aid.clone(), ring);

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
