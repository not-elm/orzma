//! `GET /sessions/tree` — read-only snapshot of every session and the
//! windows it owns, used by the cross-session window picker (tmux
//! choose-tree). Mirrors `state::snapshot_session_view`'s lock pattern:
//! clone each Session under the sessions lock, drop the guard, then
//! gather per-window names with `with_window` (so the `sessions ->
//! windows[wid]` lock-order invariant is preserved).

use crate::session_view::SessionView;
use axum::{Json, extract::State};
use ozmux_multiplexer::{MultiplexerService, Session, WindowId};
use std::collections::HashMap;

/// Handler for `GET /sessions/tree`.
pub async fn tree(State(multiplexer): State<MultiplexerService>) -> Json<serde_json::Value> {
    let cloned: Vec<Session> = {
        let sessions = multiplexer.sessions.lock().await;
        let mut all: Vec<Session> = sessions.iter().map(|(_, s)| s.clone()).collect();
        all.sort_by(|a, b| a.id.as_ref().cmp(b.id.as_ref()));
        all
    };

    let mut views: Vec<SessionView> = Vec::with_capacity(cloned.len());
    for session in &cloned {
        let mut names: HashMap<WindowId, String> = HashMap::new();
        for wid in &session.linked_windows {
            if let Some(n) = multiplexer.with_window(wid, |w| w.name.clone()).await {
                names.insert(wid.clone(), n);
            }
        }
        views.push(SessionView::from_session(session, &names));
    }

    Json(serde_json::json!({ "sessions": views }))
}

#[cfg(test)]
mod tests {
    use crate::test_helpers::{fresh_state, router_with};
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn tree_returns_empty_sessions_array_when_none() {
        let (router, _) = router_with(fresh_state());
        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/sessions/tree")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["sessions"].as_array().map(|a| a.len()), Some(0));
    }

    #[tokio::test]
    async fn tree_returns_sessions_sorted_by_id() {
        let state = fresh_state();
        let sid_a = state.multiplexer.create_session(Some("a".into())).await;
        let sid_b = state.multiplexer.create_session(Some("b".into())).await;
        let mut expected = [sid_a.to_string(), sid_b.to_string()];
        expected.sort();

        let (router, _) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/sessions/tree")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let ids: Vec<String> = v["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["id"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(ids, expected.to_vec());
    }

    #[tokio::test]
    async fn tree_includes_window_entries_with_names_and_index() {
        let state = fresh_state();
        let sid = state.multiplexer.create_session(Some("work".into())).await;
        let (w0, _, _) = state
            .multiplexer
            .create_window(Some(&sid), Some("alpha".into()))
            .await
            .unwrap();
        let (w1, _, _) = state
            .multiplexer
            .create_window(Some(&sid), Some("beta".into()))
            .await
            .unwrap();
        let (router, _) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/sessions/tree")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let s = &v["sessions"][0];
        assert_eq!(s["id"].as_str(), Some(sid.as_ref()));
        assert_eq!(s["name"].as_str(), Some("work"));
        assert_eq!(s["active_window"].as_str(), Some(w0.as_ref()));
        let windows = s["windows"].as_array().unwrap();
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0]["id"].as_str(), Some(w0.as_ref()));
        assert_eq!(windows[0]["name"].as_str(), Some("alpha"));
        assert_eq!(windows[0]["index"].as_i64(), Some(0));
        assert_eq!(windows[1]["id"].as_str(), Some(w1.as_ref()));
        assert_eq!(windows[1]["name"].as_str(), Some("beta"));
        assert_eq!(windows[1]["index"].as_i64(), Some(1));
    }

    #[tokio::test]
    async fn tree_does_not_deadlock_with_concurrent_session_ops() {
        let state = fresh_state();
        for i in 0..4 {
            let sid = state
                .multiplexer
                .create_session(Some(format!("s{i}")))
                .await;
            for j in 0..3 {
                let _ = state
                    .multiplexer
                    .create_window(Some(&sid), Some(format!("w{j}")))
                    .await
                    .unwrap();
            }
        }
        let (router, _) = router_with(state.clone());
        let req = || {
            router.clone().oneshot(
                Request::builder()
                    .uri("/sessions/tree")
                    .body(Body::empty())
                    .unwrap(),
            )
        };

        let (a, b, c) = tokio::join!(req(), req(), req());
        assert_eq!(a.unwrap().status(), StatusCode::OK);
        assert_eq!(b.unwrap().status(), StatusCode::OK);
        assert_eq!(c.unwrap().status(), StatusCode::OK);
    }
}
