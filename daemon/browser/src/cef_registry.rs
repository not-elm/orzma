//! Per-process registry of cef-backed BrowserActivity rings.

use crate::frame_ring::FrameRing;
use ozmux_browser_cef_protocol::types::ActivityId as CefActivityId;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// PoC scope: holds a single `session_id` for the process and a
/// `HashMap<CefActivityId, Arc<FrameRing>>`. Plan 2 promotes this to a richer
/// supervisor that owns the `CefHostSupervisor` and ties ring lifecycle to
/// BrowserCreate / Close commands.
pub struct BrowserCefRegistry {
    session_id: u64,
    rings: Mutex<HashMap<CefActivityId, Arc<FrameRing>>>,
}

impl BrowserCefRegistry {
    /// Creates an empty registry. `session_id` is seeded from the wall-clock
    /// microsecond at startup so reconnecting frontends can detect a daemon
    /// restart.
    pub fn new() -> Self {
        let session_id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_micros() as u64)
            .unwrap_or(1);
        Self {
            session_id,
            rings: Mutex::new(HashMap::new()),
        }
    }

    /// The session id stamped on every `SubscribeReply` / `Screencast` message
    /// emitted by the cef WS handler.
    pub fn session_id(&self) -> u64 {
        self.session_id
    }

    /// Looks up the ring registered for `aid`, if any.
    pub fn frame_ring(&self, aid: &CefActivityId) -> Option<Arc<FrameRing>> {
        self.rings
            .lock()
            .expect("browser_cef rings poisoned")
            .get(aid)
            .cloned()
    }

    /// Registers a ring for `aid`. Replaces any prior ring.
    pub fn insert(&self, aid: CefActivityId, ring: Arc<FrameRing>) {
        self.rings
            .lock()
            .expect("browser_cef rings poisoned")
            .insert(aid, ring);
    }

    /// Removes a ring (called when the underlying activity closes).
    pub fn remove(&self, aid: &CefActivityId) -> Option<Arc<FrameRing>> {
        self.rings
            .lock()
            .expect("browser_cef rings poisoned")
            .remove(aid)
    }
}

impl Default for BrowserCefRegistry {
    fn default() -> Self {
        Self::new()
    }
}
