//! Serialisable snapshot of a session and the windows it owns. Built
//! from `ozmux_multiplexer::Session` plus a caller-supplied window-name
//! lookup. JSON contract: `docs/superpowers/specs/2026-05-16-status-bar-design.md` §4.1.

use std::collections::HashMap;

use ozmux_multiplexer::{Session, SessionId, WindowId};
use serde::Serialize;

/// One entry in `SessionView.windows`.
#[derive(Serialize, Debug, PartialEq, Eq)]
pub struct SessionWindowEntry {
    /// Window id.
    pub id: WindowId,
    /// Window name (resolved by the caller; missing windows degrade to "").
    pub name: String,
    /// Position inside `Session.linked_windows`.
    pub index: u32,
}

/// JSON snapshot of a session and its windows.
#[derive(Serialize, Debug, PartialEq, Eq)]
pub struct SessionView {
    /// Session id.
    pub id: SessionId,
    /// Session name.
    pub name: String,
    /// Currently-active window in this session, if any.
    pub active_window: Option<WindowId>,
    /// All windows linked to this session, in `linked_windows` order.
    pub windows: Vec<SessionWindowEntry>,
}

impl SessionView {
    /// Build a `SessionView` from a `Session` and a pre-built window-name
    /// lookup. Windows whose names are missing from `window_names` are
    /// emitted with an empty `name` and a `tracing::warn` is logged.
    pub fn from_session(session: &Session, window_names: &HashMap<WindowId, String>) -> Self {
        let windows = session
            .linked_windows
            .iter()
            .enumerate()
            .map(|(idx, wid)| {
                let name = match window_names.get(wid) {
                    Some(n) => n.clone(),
                    None => {
                        tracing::warn!(%wid, "session window missing in name lookup");
                        String::new()
                    }
                };
                SessionWindowEntry {
                    id: wid.clone(),
                    name,
                    index: idx as u32,
                }
            })
            .collect();
        Self {
            id: session.id.clone(),
            name: session.name.clone(),
            active_window: session.active_window.clone(),
            windows,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ozmux_multiplexer::MultiplexerService;

    /// Walk the session's linked windows, locking each to read its
    /// current name. Returns a `HashMap` suitable for `from_session`.
    async fn collect_window_names(
        mp: &MultiplexerService,
        session: &Session,
    ) -> HashMap<WindowId, String> {
        let mut out = HashMap::new();
        for wid in &session.linked_windows {
            if let Some(name) = mp.with_window(wid, |w| w.name.clone()).await {
                out.insert(wid.clone(), name);
            }
        }
        out
    }

    #[tokio::test]
    async fn from_session_indexes_linked_windows_in_order() {
        let mp = MultiplexerService::default();
        let sid = mp.create_session(Some("s".into())).await;
        let (w0, _, _) = mp.create_window(Some(&sid), Some("alpha".into())).await.unwrap();
        let (w1, _, _) = mp.create_window(Some(&sid), Some("beta".into())).await.unwrap();
        let (w2, _, _) = mp.create_window(Some(&sid), Some("gamma".into())).await.unwrap();

        let sessions = mp.sessions.lock().await;
        let session = sessions.get(&sid).unwrap();
        let names = collect_window_names(&mp, session).await;
        let view = SessionView::from_session(session, &names);

        assert_eq!(view.id, sid);
        assert_eq!(view.name, "s");
        assert_eq!(view.active_window.as_ref(), Some(&w0));
        assert_eq!(view.windows.len(), 3);
        assert_eq!(view.windows[0], SessionWindowEntry { id: w0, name: "alpha".into(), index: 0 });
        assert_eq!(view.windows[1], SessionWindowEntry { id: w1, name: "beta".into(),  index: 1 });
        assert_eq!(view.windows[2], SessionWindowEntry { id: w2, name: "gamma".into(), index: 2 });
    }

    #[tokio::test]
    async fn serializes_to_expected_json_shape() {
        let mp = MultiplexerService::default();
        let sid = mp.create_session(Some("ozmux".into())).await;
        let (wid, _, _) = mp.create_window(Some(&sid), Some("main".into())).await.unwrap();
        let sessions = mp.sessions.lock().await;
        let session = sessions.get(&sid).unwrap();
        let names = collect_window_names(&mp, session).await;
        let view = SessionView::from_session(session, &names);
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["name"].as_str(), Some("ozmux"));
        assert_eq!(json["active_window"].as_str(), Some(wid.as_ref()));
        assert_eq!(json["windows"][0]["name"].as_str(), Some("main"));
        assert_eq!(json["windows"][0]["index"].as_u64(), Some(0));
    }

    #[tokio::test]
    async fn missing_window_name_degrades_to_empty_string() {
        let mp = MultiplexerService::default();
        let sid = mp.create_session(None).await;
        let (_wid, _, _) = mp.create_window(Some(&sid), Some("present".into())).await.unwrap();
        let sessions = mp.sessions.lock().await;
        let session = sessions.get(&sid).unwrap();
        let empty_names: HashMap<WindowId, String> = HashMap::new();
        let view = SessionView::from_session(session, &empty_names);
        assert_eq!(view.windows.len(), 1);
        assert_eq!(view.windows[0].name, "");
    }
}
