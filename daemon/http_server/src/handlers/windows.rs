use crate::error::HttpResult;
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use ozmux_multiplexer::{MultiplexerService, SessionError, session::SessionId, window::WindowId};
use ozmux_terminal::TerminalService;
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Deserialize, Default)]
pub struct CreateRequest {
    #[serde(default)]
    session_id: Option<SessionId>,
    #[serde(default)]
    name: Option<String>,
}

pub async fn create(
    State(ms): State<Arc<Mutex<MultiplexerService>>>,
    Json(body): Json<CreateRequest>,
) -> HttpResult<(StatusCode, Json<serde_json::Value>)> {
    let id = ms
        .lock()
        .await
        .new_window_in(body.session_id.as_ref(), body.name)?;
    Ok((StatusCode::CREATED, Json(serde_json::json!({ "id": id }))))
}

#[derive(Deserialize)]
pub struct RenameRequest {
    name: String,
}

pub async fn rename(
    State(ms): State<Arc<Mutex<MultiplexerService>>>,
    Path(window_id): Path<WindowId>,
    Json(body): Json<RenameRequest>,
) -> HttpResult<StatusCode> {
    ms.lock().await.rename_window(&window_id, body.name)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete(
    State(ms): State<Arc<Mutex<MultiplexerService>>>,
    State(terminal): State<TerminalService>,
    Path(window_id): Path<WindowId>,
) -> HttpResult<StatusCode> {
    let activities = ms.lock().await.close_window(&window_id)?;
    for aid in activities {
        let _ = terminal.kill(&aid).await;
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn select(
    State(ms): State<Arc<Mutex<MultiplexerService>>>,
    Path(window_id): Path<WindowId>,
) -> HttpResult<StatusCode> {
    ms.lock().await.select_active_window(&window_id)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn get(
    State(ms): State<Arc<Mutex<MultiplexerService>>>,
    Path(window_id): Path<WindowId>,
) -> HttpResult<Json<serde_json::Value>> {
    let ms = ms.lock().await;
    let window = ms
        .windows()
        .get(&window_id)
        .ok_or_else(|| SessionError::WindowNotFound(window_id.clone()))?;
    Ok(Json(window_view(&ms, &window_id, window)?))
}

fn window_view(
    ms: &MultiplexerService,
    id: &WindowId,
    window: &ozmux_multiplexer::window::Window,
) -> ozmux_multiplexer::SessionResult<serde_json::Value> {
    let pane_ids = ms.cells_ref().pane_ids_in_subtree(&window.root_cell)?;
    let panes: Vec<serde_json::Value> = pane_ids
        .iter()
        .filter_map(|pid| ms.panes().get(pid).map(|pane| pane_view(pid, pane)))
        .collect();
    Ok(serde_json::json!({
        "id": id,
        "name": window.name,
        "root_cell": window.root_cell,
        "active_pane": window.active_pane,
        "panes": panes,
    }))
}

fn pane_view(
    id: &ozmux_multiplexer::pane::PaneId,
    pane: &ozmux_multiplexer::pane::Pane,
) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "activities": pane.activities,
        "active_activity": pane.active_activity,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::router_with;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn create_with_session_id_returns_201_and_attaches() {
        let mut ms = MultiplexerService::default();
        let sid = ms.new_session(None);
        let (router, state) = router_with(ms);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/windows")
                    .header("content-type", "application/json")
                    .body(Body::from(format!(r#"{{"session_id":"{}"}}"#, sid)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(v["id"].is_string());
        let ms = state.multiplexer.lock().await;
        assert_eq!(ms.sessions().get(&sid).unwrap().windows.len(), 1);
    }

    #[tokio::test]
    async fn create_without_session_id_creates_orphan() {
        let (router, state) = router_with(MultiplexerService::default());
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/windows")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let ms = state.multiplexer.lock().await;
        assert_eq!(ms.windows().len(), 1);
        // No session should be referencing it (there are no sessions at all).
        assert_eq!(ms.sessions().len(), 0);
    }

    #[tokio::test]
    async fn create_with_unknown_session_returns_404() {
        let (router, _) = router_with(MultiplexerService::default());
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/windows")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"session_id":"bogus"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["code"].as_str(), Some("SESSION_NOT_FOUND"));
    }

    #[tokio::test]
    async fn rename_returns_204_and_updates_name() {
        let mut ms = MultiplexerService::default();
        let wid = ms.new_window_in(None, Some("orig".into())).unwrap();
        let (router, state) = router_with(ms);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/windows/{}", wid))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"renamed"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let ms = state.multiplexer.lock().await;
        assert_eq!(ms.windows().get(&wid).unwrap().name, "renamed");
    }

    #[tokio::test]
    async fn delete_returns_204_and_removes_window() {
        let mut ms = MultiplexerService::default();
        let wid = ms.new_window_in(None, None).unwrap();
        let (router, state) = router_with(ms);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/windows/{}", wid))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let ms = state.multiplexer.lock().await;
        assert!(ms.windows().get(&wid).is_none());
    }

    #[tokio::test]
    async fn delete_unknown_window_returns_404() {
        let (router, _) = router_with(MultiplexerService::default());
        let resp = router
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/windows/missing")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn select_returns_204_and_updates_active_window() {
        let mut ms = MultiplexerService::default();
        let sid = ms.new_session(None);
        let wid_a = ms.new_window_in(Some(&sid), None).unwrap();
        let wid_b = ms.new_window_in(Some(&sid), None).unwrap();
        let (router, state) = router_with(ms);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/windows/{}/select", wid_b))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let ms = state.multiplexer.lock().await;
        assert_eq!(
            ms.sessions().get(&sid).unwrap().active_window.as_ref(),
            Some(&wid_b)
        );
        let _ = wid_a;
    }

    #[tokio::test]
    async fn select_orphan_window_returns_409_window_not_attached() {
        let mut ms = MultiplexerService::default();
        let wid = ms.new_window_in(None, None).unwrap();
        let (router, _) = router_with(ms);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/windows/{}/select", wid))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["code"].as_str(), Some("WINDOW_NOT_ATTACHED"));
    }

    #[tokio::test]
    async fn get_returns_window_view_with_panes() {
        let mut ms = MultiplexerService::default();
        let (_sid, wid, pid, aid) = ms.bootstrap_default().unwrap();
        let root_cell = ms.windows().get(&wid).unwrap().root_cell.clone();

        let (router, _) = router_with(ms);
        let resp = router
            .oneshot(
                Request::builder()
                    .uri(format!("/windows/{}", wid))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["id"].as_str(), Some(wid.as_ref()));
        assert!(v["name"].is_string());
        assert_eq!(v["root_cell"].as_str(), Some(root_cell.as_ref()));
        assert_eq!(v["active_pane"].as_str(), Some(pid.as_ref()));
        let panes = v["panes"].as_array().unwrap();
        assert_eq!(panes.len(), 1);
        assert_eq!(panes[0]["id"].as_str(), Some(pid.as_ref()));
        assert_eq!(panes[0]["activities"][0].as_str(), Some(aid.as_ref()));
        assert_eq!(panes[0]["active_activity"].as_str(), Some(aid.as_ref()));
    }

    #[tokio::test]
    async fn get_after_split_returns_two_panes() {
        use ozmux_multiplexer::cells::{Side, SplitOrientation};
        let mut ms = MultiplexerService::default();
        let (_sid, wid, pid, _aid) = ms.bootstrap_default().unwrap();
        ms.split_pane(pid, Side::After, SplitOrientation::Horizontal)
            .unwrap();

        let (router, _) = router_with(ms);
        let resp = router
            .oneshot(
                Request::builder()
                    .uri(format!("/windows/{}", wid))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["panes"].as_array().map(|a| a.len()), Some(2));
    }

    #[tokio::test]
    async fn get_orphan_window_returns_window_view() {
        let mut ms = MultiplexerService::default();
        let wid = ms.new_window_in(None, Some("orphan".into())).unwrap();
        let (router, _) = router_with(ms);
        let resp = router
            .oneshot(
                Request::builder()
                    .uri(format!("/windows/{}", wid))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["id"].as_str(), Some(wid.as_ref()));
        assert_eq!(v["name"].as_str(), Some("orphan"));
        assert_eq!(v["panes"].as_array().map(|a| a.len()), Some(1));
    }

    #[tokio::test]
    async fn get_unknown_window_returns_404() {
        let (router, _) = router_with(MultiplexerService::default());
        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/windows/missing")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["code"].as_str(), Some("WINDOW_NOT_FOUND"));
    }

    #[tokio::test]
    async fn get_window_active_activity_matches_initial_activity() {
        let mut ms = MultiplexerService::default();
        let (_sid, wid, _pid, aid) = ms.bootstrap_default().unwrap();
        let (router, _) = router_with(ms);
        let resp = router
            .oneshot(
                Request::builder()
                    .uri(format!("/windows/{}", wid))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            v["panes"][0]["active_activity"].as_str(),
            Some(aid.as_ref())
        );
    }
}
