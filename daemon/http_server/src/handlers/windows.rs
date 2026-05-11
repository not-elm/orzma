use crate::{MultiplexerState, error::HttpResult};
use axum::{
    Json,
    extract::{
        Path, State, WebSocketUpgrade,
        ws::{CloseFrame, Message, WebSocket},
    },
    http::StatusCode,
};
use futures_util::{SinkExt, StreamExt, stream::SplitSink};
use ozmux_multiplexer::{
    MultiplexerError, MultiplexerService, session::SessionId, window::WindowId,
};
use ozmux_terminal::TerminalService;
use serde::Deserialize;

#[derive(Deserialize, Default)]
pub struct CreateRequest {
    #[serde(default)]
    session_id: Option<SessionId>,
    #[serde(default)]
    name: Option<String>,
}

pub async fn create(
    State(ms): State<MultiplexerState>,
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
    State(ms): State<MultiplexerState>,
    State(broadcaster): State<crate::layout_broadcast::LayoutBroadcaster>,
    Path(window_id): Path<WindowId>,
    Json(body): Json<RenameRequest>,
) -> HttpResult<StatusCode> {
    let mut ms = ms.lock().await;
    ms.rename_window(&window_id, body.name)?;
    if let Some(window) = ms.windows().get(&window_id) {
        match window_view_for(&ms, &window_id, window) {
            Ok(view) => broadcaster.publish(&window_id, view),
            Err(e) => tracing::warn!(error = %e, %window_id, "skipped layout publish on rename"),
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete(
    State(ms): State<MultiplexerState>,
    State(terminal): State<TerminalService>,
    State(broadcaster): State<crate::layout_broadcast::LayoutBroadcaster>,
    Path(window_id): Path<WindowId>,
) -> HttpResult<StatusCode> {
    let activities = ms.lock().await.close_window(&window_id)?;
    for aid in activities {
        let _ = terminal.kill(&aid).await;
    }
    broadcaster.close(&window_id);
    Ok(StatusCode::NO_CONTENT)
}

pub async fn select(
    State(ms): State<MultiplexerState>,
    Path(window_id): Path<WindowId>,
) -> HttpResult<StatusCode> {
    ms.lock().await.select_active_window(&window_id)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn get(
    State(ms): State<MultiplexerState>,
    Path(window_id): Path<WindowId>,
) -> HttpResult<Json<serde_json::Value>> {
    let ms = ms.lock().await;
    let window = ms
        .windows()
        .get(&window_id)
        .ok_or_else(|| MultiplexerError::WindowNotFound(window_id.clone()))?;
    Ok(Json(window_view_for(&ms, &window_id, window)?))
}

pub(crate) fn window_view_for(
    ms: &MultiplexerService,
    id: &WindowId,
    window: &ozmux_multiplexer::window::Window,
) -> ozmux_multiplexer::MultiplexerResult<serde_json::Value> {
    let pane_ids = ms.cells_ref().pane_ids_in_subtree(&window.root_cell)?;
    let panes: Vec<serde_json::Value> = pane_ids
        .iter()
        .filter_map(|pid| ms.panes().get(pid).map(|pane| pane_view(ms, pid, pane)))
        .collect();
    let layout = crate::layout_dto::build_layout(&window.root_cell, ms.cells_ref())?;
    Ok(serde_json::json!({
        "id": id,
        "name": window.name,
        "root_cell": window.root_cell,
        "active_pane": window.active_pane,
        "panes": panes,
        "layout_schema_version": 1,
        "layout": layout,
    }))
}

fn pane_view(
    ms: &MultiplexerService,
    id: &ozmux_multiplexer::pane::PaneId,
    pane: &ozmux_multiplexer::pane::Pane,
) -> serde_json::Value {
    let activities: Vec<serde_json::Value> = pane
        .activities
        .iter()
        .map(|aid| match ms.activities().get(aid).map(|a| &a.kind) {
            Some(ozmux_multiplexer::activity::ActivityKind::Extension { .. }) => {
                serde_json::json!({
                    "id": aid,
                    "kind": "extension",
                    "iframe_url": format!("/activities/{aid}/iframe/index.html"),
                })
            }
            Some(ozmux_multiplexer::activity::ActivityKind::Terminal) | None => {
                serde_json::json!({ "id": aid, "kind": "terminal" })
            }
        })
        .collect();
    serde_json::json!({
        "id": id,
        "activities": activities,
        "active_activity": pane.active_activity,
    })
}

pub async fn events(
    State(ms): State<MultiplexerState>,
    State(broadcaster): State<crate::layout_broadcast::LayoutBroadcaster>,
    Path(window_id): Path<WindowId>,
    ws: WebSocketUpgrade,
) -> impl axum::response::IntoResponse {
    ws.on_upgrade(move |socket| handle_events_socket(socket, ms, broadcaster, window_id))
}

async fn handle_events_socket(
    socket: WebSocket,
    ms: MultiplexerState,
    broadcaster: crate::layout_broadcast::LayoutBroadcaster,
    window_id: WindowId,
) {
    let (mut tx, _rx) = socket.split();

    // Atomic snapshot + subscribe under ms.lock.
    let (snapshot, mut receiver) = {
        let ms_guard = ms.lock().await;
        let Some(window) = ms_guard.windows().get(&window_id) else {
            close_with(&mut tx, 1011, "window_not_found").await;
            return;
        };
        let snapshot = match window_view_for(&ms_guard, &window_id, window) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, %window_id, "snapshot build failed");
                close_with(&mut tx, 1011, "internal_error").await;
                return;
            }
        };
        let receiver = broadcaster.subscribe_or_create(&window_id);
        (snapshot, receiver)
    };

    if tx
        .send(Message::Text(snapshot.to_string().into()))
        .await
        .is_err()
    {
        return;
    }

    use tokio::sync::broadcast::error::RecvError;
    loop {
        match receiver.recv().await {
            Ok(view) => {
                if tx
                    .send(Message::Text(view.to_string().into()))
                    .await
                    .is_err()
                {
                    return;
                }
            }
            Err(RecvError::Lagged(_)) => {
                close_with(&mut tx, 1011, "lagged").await;
                return;
            }
            Err(RecvError::Closed) => {
                close_with(&mut tx, 1011, "window_closed").await;
                return;
            }
        }
    }
}

async fn close_with(tx: &mut SplitSink<WebSocket, Message>, code: u16, reason: &'static str) {
    let _ = tx
        .send(Message::Close(Some(CloseFrame {
            code,
            reason: reason.into(),
        })))
        .await;
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
        assert_eq!(panes[0]["activities"][0]["id"].as_str(), Some(aid.as_ref()));
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
    async fn rename_publishes_layout_with_new_name() {
        let mut ms = MultiplexerService::default();
        let wid = ms.new_window_in(None, Some("orig".into())).unwrap();
        let (router, state) = router_with(ms);
        let mut rx = state.layout_broadcast.subscribe_or_create(&wid);

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

        let view = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv())
            .await
            .expect("publish timed out")
            .expect("recv error");
        assert_eq!(view["name"].as_str(), Some("renamed"));
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

    #[tokio::test]
    async fn get_window_returns_activities_with_kind_for_terminal() {
        let mut ms = MultiplexerService::default();
        let (_sid, wid, _pid, _aid) = ms.bootstrap_default().unwrap();
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
        let panes = v["panes"].as_array().unwrap();
        let activities = panes[0]["activities"].as_array().unwrap();
        assert!(activities[0]["id"].is_string());
        assert_eq!(activities[0]["kind"].as_str(), Some("terminal"));
    }

    #[tokio::test]
    async fn get_window_includes_layout_schema_version_1() {
        let mut ms = MultiplexerService::default();
        let (_sid, wid, _pid, _aid) = ms.bootstrap_default().unwrap();
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
        assert_eq!(v["layout_schema_version"].as_u64(), Some(1));
    }

    #[tokio::test]
    async fn get_window_includes_layout_root_with_pane_for_single_pane_window() {
        let mut ms = MultiplexerService::default();
        let (_sid, wid, pid, _aid) = ms.bootstrap_default().unwrap();
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
        let layout = &v["layout"];
        assert_eq!(layout["type"].as_str(), Some("root"));
        let child = &layout["child"];
        assert_eq!(child["type"].as_str(), Some("pane"));
        assert_eq!(child["pane_id"].as_str(), Some(pid.as_ref()));
    }

    #[tokio::test]
    async fn get_window_layout_after_split_has_split_node() {
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
        let split = &v["layout"]["child"];
        assert_eq!(split["type"].as_str(), Some("split"));
        assert_eq!(split["orientation"].as_str(), Some("horizontal"));
        assert!(split["lhs"].is_object());
        assert!(split["rhs"].is_object());
    }

    #[tokio::test]
    async fn delete_window_kicks_subscribers_with_recv_closed() {
        use tokio::sync::broadcast::error::RecvError;
        let mut ms = MultiplexerService::default();
        let wid = ms.new_window_in(None, None).unwrap();
        let (router, state) = router_with(ms);
        let mut rx = state.layout_broadcast.subscribe_or_create(&wid);

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

        let err = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv())
            .await
            .expect("recv timed out")
            .expect_err("expected RecvError::Closed");
        assert!(matches!(err, RecvError::Closed));
    }

    use crate::layout_broadcast::LayoutBroadcaster;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use tokio::net::TcpListener as TokioTcpListener;
    use tokio::sync::Mutex;

    async fn spawn_server(state: crate::AppState) -> SocketAddr {
        let listener = TokioTcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, crate::daemon_router(state))
                .await
                .unwrap();
        });
        addr
    }

    #[tokio::test]
    async fn events_ws_sends_initial_snapshot() {
        use futures_util::StreamExt;
        use tokio_tungstenite::{connect_async, tungstenite::Message as TtMessage};

        let mut ms = MultiplexerService::default();
        let (_sid, wid, _pid, _aid) = ms.bootstrap_default().unwrap();
        let state = crate::AppState {
            multiplexer: crate::MultiplexerState(Arc::new(Mutex::new(ms))),
            terminal: ozmux_terminal::TerminalService::default(),
            extensions: ozmux_extension::ExtensionRegistry::default(),
            layout_broadcast: LayoutBroadcaster::default(),
        };
        let addr = spawn_server(state).await;
        let url = format!("ws://{}/windows/{}/events", addr, wid);
        let (mut ws, _) = connect_async(&url).await.unwrap();
        let msg = ws.next().await.unwrap().unwrap();
        let text = match msg {
            TtMessage::Text(t) => t,
            other => panic!("expected text frame, got {other:?}"),
        };
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["id"].as_str(), Some(wid.as_ref()));
        assert!(v["layout"].is_object());
    }

    #[tokio::test]
    async fn events_ws_closes_with_window_not_found_for_unknown_wid() {
        use futures_util::StreamExt;
        use tokio_tungstenite::{connect_async, tungstenite::Message as TtMessage};
        let state = crate::AppState::default();
        let addr = spawn_server(state).await;
        let url = format!("ws://{}/windows/does-not-exist/events", addr);
        let (mut ws, _) = connect_async(&url).await.unwrap();
        match ws.next().await.unwrap().unwrap() {
            TtMessage::Close(Some(frame)) => {
                assert_eq!(u16::from(frame.code), 1011);
                assert!(frame.reason.contains("window_not_found"));
            }
            other => panic!("expected close frame, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn events_ws_sends_frame_after_external_publish() {
        use futures_util::StreamExt;
        use tokio_tungstenite::{connect_async, tungstenite::Message as TtMessage};

        let mut ms = MultiplexerService::default();
        let (_sid, wid, _pid, _aid) = ms.bootstrap_default().unwrap();
        let state = crate::AppState {
            multiplexer: crate::MultiplexerState(Arc::new(Mutex::new(ms))),
            terminal: ozmux_terminal::TerminalService::default(),
            extensions: ozmux_extension::ExtensionRegistry::default(),
            layout_broadcast: LayoutBroadcaster::default(),
        };
        let bc = state.layout_broadcast.clone();
        let addr = spawn_server(state).await;
        let url = format!("ws://{}/windows/{}/events", addr, wid);
        let (mut ws, _) = connect_async(&url).await.unwrap();
        let _initial = ws.next().await.unwrap().unwrap();

        bc.publish(
            &wid,
            serde_json::json!({ "id": wid.as_ref(), "marker": "second" }),
        );

        match ws.next().await.unwrap().unwrap() {
            TtMessage::Text(t) => {
                let v: serde_json::Value = serde_json::from_str(&t).unwrap();
                assert_eq!(v["marker"].as_str(), Some("second"));
            }
            other => panic!("expected text frame, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn events_ws_closes_with_window_closed_when_broadcaster_drops() {
        use futures_util::StreamExt;
        use tokio_tungstenite::{connect_async, tungstenite::Message as TtMessage};
        let mut ms = MultiplexerService::default();
        let (_sid, wid, _pid, _aid) = ms.bootstrap_default().unwrap();
        let state = crate::AppState {
            multiplexer: crate::MultiplexerState(Arc::new(Mutex::new(ms))),
            terminal: ozmux_terminal::TerminalService::default(),
            extensions: ozmux_extension::ExtensionRegistry::default(),
            layout_broadcast: LayoutBroadcaster::default(),
        };
        let bc = state.layout_broadcast.clone();
        let addr = spawn_server(state).await;
        let url = format!("ws://{}/windows/{}/events", addr, wid);
        let (mut ws, _) = connect_async(&url).await.unwrap();
        let _initial = ws.next().await.unwrap().unwrap();
        bc.close(&wid);
        match ws.next().await.unwrap().unwrap() {
            TtMessage::Close(Some(frame)) => {
                assert_eq!(u16::from(frame.code), 1011);
                assert!(frame.reason.contains("window_closed"));
            }
            other => panic!("expected close frame, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn events_ws_closes_with_lagged_when_subscriber_falls_behind() {
        use futures_util::StreamExt;
        use tokio_tungstenite::{connect_async, tungstenite::Message as TtMessage};
        let mut ms = MultiplexerService::default();
        let (_sid, wid, _pid, _aid) = ms.bootstrap_default().unwrap();
        let state = crate::AppState {
            multiplexer: crate::MultiplexerState(Arc::new(Mutex::new(ms))),
            terminal: ozmux_terminal::TerminalService::default(),
            extensions: ozmux_extension::ExtensionRegistry::default(),
            layout_broadcast: LayoutBroadcaster::new(1),
        };
        let bc = state.layout_broadcast.clone();
        let addr = spawn_server(state).await;
        let url = format!("ws://{}/windows/{}/events", addr, wid);
        let (mut ws, _) = connect_async(&url).await.unwrap();
        let _initial = ws.next().await.unwrap().unwrap();
        // Don't read further; force the channel to lag.
        bc.publish(&wid, serde_json::json!({ "n": 1 }));
        bc.publish(&wid, serde_json::json!({ "n": 2 }));
        bc.publish(&wid, serde_json::json!({ "n": 3 }));
        // Read until we get a Close frame.
        loop {
            match ws.next().await.unwrap().unwrap() {
                TtMessage::Close(Some(frame)) => {
                    assert_eq!(u16::from(frame.code), 1011);
                    assert!(frame.reason.contains("lagged"));
                    break;
                }
                TtMessage::Text(_) => continue,
                other => panic!("unexpected frame: {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn get_window_includes_iframe_url_for_extension_activity() {
        use ozmux_multiplexer::activity::{Activity, ActivityKind};
        use ozmux_multiplexer::cells::{Side, SplitOrientation};
        use std::path::PathBuf;
        let mut ms = MultiplexerService::default();
        let (_sid, wid, bootstrap_pane, _aid) = ms.bootstrap_default().unwrap();
        let activity_id = ms.new_activity(Activity {
            name: "ext".into(),
            kind: ActivityKind::Extension {
                html_root: PathBuf::from("/tmp"),
            },
        });
        let pane_id = ms.new_pane_with_activity(activity_id.clone()).unwrap();
        ms.split_with_pane(
            bootstrap_pane,
            pane_id,
            Side::After,
            SplitOrientation::Horizontal,
        )
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
        let panes = v["panes"].as_array().unwrap();
        let ext_pane = panes
            .iter()
            .find(|p| p["activities"][0]["kind"].as_str() == Some("extension"))
            .expect("extension pane not found");
        let iframe_url = ext_pane["activities"][0]["iframe_url"].as_str().unwrap();
        assert_eq!(
            iframe_url,
            format!("/activities/{activity_id}/iframe/index.html")
        );
    }

    #[tokio::test]
    async fn snapshot_and_subscribe_is_atomic_under_concurrent_mutations() {
        use futures_util::StreamExt;
        use ozmux_multiplexer::cells::{Side, SplitOrientation};
        use tokio_tungstenite::{connect_async, tungstenite::Message as TtMessage};

        let mut ms = MultiplexerService::default();
        let (_sid, wid, pid, _aid) = ms.bootstrap_default().unwrap();
        let state = crate::AppState {
            multiplexer: crate::MultiplexerState(Arc::new(Mutex::new(ms))),
            terminal: ozmux_terminal::TerminalService::default(),
            extensions: ozmux_extension::ExtensionRegistry::default(),
            layout_broadcast: LayoutBroadcaster::default(),
        };
        let ms_handle = state.multiplexer.clone();
        let bc = state.layout_broadcast.clone();
        let addr = spawn_server(state).await;

        // Spawn N concurrent splits of the bootstrap pane.
        let mut handles = vec![];
        for _ in 0..5 {
            let h = ms_handle.clone();
            let bc = bc.clone();
            let wid_ = wid.clone();
            let pid_ = pid.clone();
            handles.push(tokio::spawn(async move {
                let mut ms = h.lock().await;
                if ms
                    .split_pane(pid_.clone(), Side::After, SplitOrientation::Horizontal)
                    .is_ok()
                    && let Some(window) = ms.windows().get(&wid_)
                    && let Ok(view) = super::window_view_for(&ms, &wid_, window)
                {
                    bc.publish(&wid_, view);
                }
            }));
        }

        // Connect AFTER spawning the mutation tasks. Some mutations may have
        // landed before our connect; the rest may land during/after. The atomicity
        // contract: regardless of timing, the *final* observed view must equal
        // the *final* multiplexer state.
        let url = format!("ws://{}/windows/{}/events", addr, wid);
        let (mut ws, _) = connect_async(&url).await.unwrap();
        let _initial = ws.next().await.unwrap().unwrap();

        for h in handles {
            let _ = h.await;
        }

        // Drain any further frames with a short idle window.
        let mut latest = match _initial {
            TtMessage::Text(t) => serde_json::from_str::<serde_json::Value>(&t).unwrap(),
            _ => panic!("expected text frame"),
        };
        while let Ok(Some(Ok(TtMessage::Text(t)))) =
            tokio::time::timeout(std::time::Duration::from_millis(100), ws.next()).await
        {
            latest = serde_json::from_str::<serde_json::Value>(&t).unwrap();
        }

        let final_pane_count = ms_handle.lock().await.panes().len();
        let observed_pane_count = latest["panes"].as_array().map(|a| a.len()).unwrap_or(0);
        assert_eq!(
            observed_pane_count, final_pane_count,
            "client's last observed view does not match multiplexer's final state \
             (observed={observed_pane_count}, final={final_pane_count})"
        );
    }
}
