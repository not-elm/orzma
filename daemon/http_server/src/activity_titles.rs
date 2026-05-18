//! Kind-agnostic activity-title map. Producers (terminal/browser) publish
//! `(activity_id → title)`; consumers (the WindowView builder and the
//! `title_republish` debounce task) read snapshots and listen for changes.

use ozmux_multiplexer::{ActivityId, WindowId};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{RwLock, broadcast};

/// Shared, cheaply-cloneable handle to the per-activity title map.
#[derive(Clone)]
pub struct ActivityTitles {
    inner: Arc<RwLock<HashMap<ActivityId, String>>>,
    changes: broadcast::Sender<WindowId>,
}

impl Default for ActivityTitles {
    fn default() -> Self {
        let (changes, _) = broadcast::channel(256);
        Self {
            inner: Arc::default(),
            changes,
        }
    }
}

impl ActivityTitles {
    /// Publish a title change for `aid` in window `wid`. Best-effort
    /// broadcast: receivers that have hung up (no current subscribers,
    /// or `Lagged`) are dropped silently.
    pub async fn set(&self, wid: &WindowId, aid: &ActivityId, title: String) {
        let mut map = self.inner.write().await;
        map.insert(aid.clone(), title);
        drop(map);
        let _ = self.changes.send(wid.clone());
    }

    /// Remove a title for `aid` and notify subscribers of the owning window.
    pub async fn forget(&self, wid: &WindowId, aid: &ActivityId) {
        let mut map = self.inner.write().await;
        map.remove(aid);
        drop(map);
        let _ = self.changes.send(wid.clone());
    }

    /// Look up the current title for `aid`, if any.
    pub async fn get(&self, aid: &ActivityId) -> Option<String> {
        self.inner.read().await.get(aid).cloned()
    }

    /// Returns a full snapshot of all currently-known titles.
    pub async fn snapshot(&self) -> HashMap<ActivityId, String> {
        self.inner.read().await.clone()
    }

    /// Subscribe to per-window change notifications.
    pub fn subscribe(&self) -> broadcast::Receiver<WindowId> {
        self.changes.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn set_then_get_returns_value() {
        let t = ActivityTitles::default();
        let wid = WindowId::new();
        let aid = ActivityId::new();
        t.set(&wid, &aid, "Hello".into()).await;
        assert_eq!(t.get(&aid).await.as_deref(), Some("Hello"));
    }

    #[tokio::test]
    async fn set_broadcasts_window_id() {
        let t = ActivityTitles::default();
        let mut rx = t.subscribe();
        let wid = WindowId::new();
        let aid = ActivityId::new();
        t.set(&wid, &aid, "X".into()).await;
        assert_eq!(rx.recv().await.unwrap(), wid);
    }

    #[tokio::test]
    async fn forget_removes_entry() {
        let t = ActivityTitles::default();
        let wid = WindowId::new();
        let aid = ActivityId::new();
        t.set(&wid, &aid, "X".into()).await;
        t.forget(&wid, &aid).await;
        assert!(t.get(&aid).await.is_none());
    }

    #[tokio::test]
    async fn snapshot_returns_all_entries() {
        let t = ActivityTitles::default();
        let wid = WindowId::new();
        let aid1 = ActivityId::new();
        let aid2 = ActivityId::new();
        t.set(&wid, &aid1, "A".into()).await;
        t.set(&wid, &aid2, "B".into()).await;
        let snap = t.snapshot().await;
        assert_eq!(snap.get(&aid1).map(String::as_str), Some("A"));
        assert_eq!(snap.get(&aid2).map(String::as_str), Some("B"));
    }
}
