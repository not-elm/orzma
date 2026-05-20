//! Per-process registry of cef-backed BrowserActivity rings and nav state.

use crate::frame_ring::FrameRing;
use ozmux_browser_cef_protocol::types::ActivityId as CefActivityId;
use ozmux_browser_cef_protocol::wire::{BrowserUnavailableReason, CursorKind};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::{broadcast, watch};

/// Snapshot of the navigation state for a single browser activity.
///
/// Updated by the event pump whenever `HostEvent::NavStateChanged` or
/// `HostEvent::TitleChanged` arrives; watched by the cef WS handler to push
/// `BrowserServerMsg::Nav` messages to connected clients.
#[derive(Debug, Clone, Default)]
pub struct NavState {
    /// Current page URL.
    pub url: String,
    /// Current page title.
    pub title: String,
    /// `true` if back navigation is available.
    pub can_back: bool,
    /// `true` if forward navigation is available.
    pub can_forward: bool,
}

/// One entry in the registry, owning the frame ring and the nav state channel.
pub struct BrowserCefEntry {
    /// Shared frame ring the event pump pushes `FrameProduced` payloads into.
    pub ring: Arc<FrameRing>,
    /// Per-activity nav state sender. The pump task updates this; each WS
    /// subscriber holds a `Receiver` and pushes `BrowserServerMsg::Nav` on change.
    pub nav_tx: watch::Sender<NavState>,
    /// Per-activity cursor-kind sender. The pump task updates this on
    /// `HostEvent::CursorChanged`; each WS subscriber pushes
    /// `BrowserServerMsg::Cursor` on change.
    pub cursor_tx: watch::Sender<CursorKind>,
}

/// PoC scope: holds a single `session_id` for the process and a
/// `HashMap<CefActivityId, BrowserCefEntry>`. Future work promotes this to
/// a richer supervisor that ties ring lifecycle to BrowserCreate / Close
/// commands.
pub struct BrowserCefRegistry {
    session_id: u64,
    entries: Mutex<HashMap<CefActivityId, BrowserCefEntry>>,
    /// Broadcast channel for signalling that the cef backend has become
    /// permanently unavailable. Seeded in [`new`](Self::new) with capacity 16;
    /// sent by the crash-watcher task in bootstrap; subscribed by each
    /// connected cef WS handler.
    unavailable_tx: broadcast::Sender<BrowserUnavailableReason>,
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
        let (unavailable_tx, _) = broadcast::channel(16);
        Self {
            session_id,
            entries: Mutex::new(HashMap::new()),
            unavailable_tx,
        }
    }

    /// Returns a new `broadcast::Receiver` that fires when the cef backend
    /// signals permanent unavailability. Subscribe once per WS connection
    /// before entering the main select loop.
    pub fn unavailable_subscribe(&self) -> broadcast::Receiver<BrowserUnavailableReason> {
        self.unavailable_tx.subscribe()
    }

    /// Broadcasts a `BrowserUnavailableReason` to all current subscribers.
    ///
    /// A `SendError` (no receivers) is silently ignored — the reason is
    /// informational and no subscriber is a valid steady state.
    pub fn broadcast_unavailable(&self, reason: BrowserUnavailableReason) {
        let _ = self.unavailable_tx.send(reason);
    }

    /// The session id stamped on every `SubscribeReply` / `Screencast` message
    /// emitted by the cef WS handler.
    pub fn session_id(&self) -> u64 {
        self.session_id
    }

    /// Registers a ring for `aid`, creating a fresh `NavState` watch channel.
    ///
    /// Returns the `watch::Receiver<NavState>` for the caller; also accessible
    /// later via [`nav_subscribe`](Self::nav_subscribe).
    pub fn insert(&self, aid: CefActivityId, ring: Arc<FrameRing>) -> watch::Receiver<NavState> {
        let (nav_tx, nav_rx) = watch::channel(NavState::default());
        let (cursor_tx, _) = watch::channel(CursorKind::Default);
        let entry = BrowserCefEntry {
            ring,
            nav_tx,
            cursor_tx,
        };
        self.entries
            .lock()
            .expect("browser_cef entries poisoned")
            .insert(aid, entry);
        nav_rx
    }

    /// Looks up the frame ring registered for `aid`, if any.
    pub fn frame_ring(&self, aid: &CefActivityId) -> Option<Arc<FrameRing>> {
        self.entries
            .lock()
            .expect("browser_cef entries poisoned")
            .get(aid)
            .map(|e| Arc::clone(&e.ring))
    }

    /// Returns a new `watch::Receiver<NavState>` that tracks nav state for `aid`.
    ///
    /// Returns `None` if no entry for `aid` is registered.
    pub fn nav_subscribe(&self, aid: &CefActivityId) -> Option<watch::Receiver<NavState>> {
        self.entries
            .lock()
            .expect("browser_cef entries poisoned")
            .get(aid)
            .map(|e| e.nav_tx.subscribe())
    }

    /// Replaces the nav state for `aid` with `state`. Returns an error string
    /// if no entry is registered for `aid`.
    pub fn nav_publish(&self, aid: &CefActivityId, state: NavState) -> Result<(), String> {
        let guard = self.entries.lock().expect("browser_cef entries poisoned");
        match guard.get(aid) {
            Some(e) => {
                e.nav_tx.send_replace(state);
                Ok(())
            }
            None => Err(format!("no registry entry for aid={}", aid.0)),
        }
    }

    /// Returns a new `watch::Receiver<CursorKind>` tracking the cursor for `aid`.
    ///
    /// Returns `None` if no entry for `aid` is registered.
    pub fn cursor_subscribe(&self, aid: &CefActivityId) -> Option<watch::Receiver<CursorKind>> {
        self.entries
            .lock()
            .expect("browser_cef entries poisoned")
            .get(aid)
            .map(|e| e.cursor_tx.subscribe())
    }

    /// Replaces the cursor kind for `aid`. Returns an error string if no entry
    /// is registered for `aid`.
    pub fn cursor_publish(&self, aid: &CefActivityId, cursor: CursorKind) -> Result<(), String> {
        let guard = self.entries.lock().expect("browser_cef entries poisoned");
        match guard.get(aid) {
            Some(e) => {
                e.cursor_tx.send_replace(cursor);
                Ok(())
            }
            None => Err(format!("no registry entry for aid={}", aid.0)),
        }
    }

    /// Returns a clone of the current nav state for `aid`, if any entry exists.
    pub fn nav_current(&self, aid: &CefActivityId) -> Option<NavState> {
        self.entries
            .lock()
            .expect("browser_cef entries poisoned")
            .get(aid)
            .map(|e| e.nav_tx.borrow().clone())
    }

    /// Removes an entry (called when the underlying activity closes).
    ///
    /// Returns the removed entry so the caller can perform any necessary cleanup.
    pub fn remove(&self, aid: &CefActivityId) -> Option<BrowserCefEntry> {
        self.entries
            .lock()
            .expect("browser_cef entries poisoned")
            .remove(aid)
    }
}

impl Default for BrowserCefRegistry {
    fn default() -> Self {
        Self::new()
    }
}
