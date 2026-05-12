use crate::handlers::publish_window_layout;
use crate::{AppState, error::HttpResult};
use axum::{
    Json,
    extract::{
        Path, State, WebSocketUpgrade,
        ws::{CloseFrame, Message, WebSocket},
    },
    http::StatusCode,
};
use futures_util::{SinkExt, StreamExt, stream::SplitSink};
use ozmux_multiplexer::{MultiplexerResult, SessionId, Window, WindowId};
use serde::Deserialize;

#[derive(Deserialize, Default)]
pub struct CreateRequest {
    #[serde(default)]
    session_id: Option<SessionId>,
    #[serde(default)]
    name: Option<String>,
}

pub async fn create(
    State(state): State<AppState>,
    Json(body): Json<CreateRequest>,
) -> HttpResult<(StatusCode, Json<serde_json::Value>)> {
    let (wid, _pid, _aid) = state
        .create_window(body.session_id.as_ref(), body.name)
        .await?;
    Ok((StatusCode::CREATED, Json(serde_json::json!({ "id": wid }))))
}

#[derive(Deserialize)]
pub struct RenameRequest {
    name: String,
}

pub async fn rename(
    State(state): State<AppState>,
    Path(window_id): Path<WindowId>,
    Json(body): Json<RenameRequest>,
) -> HttpResult<StatusCode> {
    state.rename_window(&window_id, body.name).await?;
    publish_window_layout(&state, &window_id).await;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete(
    State(state): State<AppState>,
    Path(window_id): Path<WindowId>,
) -> HttpResult<StatusCode> {
    state.close_window(&window_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn select(
    State(state): State<AppState>,
    Path(window_id): Path<WindowId>,
) -> HttpResult<StatusCode> {
    state.select_active_window(&window_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn get(
    State(state): State<AppState>,
    Path(window_id): Path<WindowId>,
) -> HttpResult<Json<serde_json::Value>> {
    let view = state
        .with_window_or_404(&window_id, |w| window_view_for(w))
        .await?;
    Ok(Json(view))
}

pub(crate) fn window_view_for(window: &Window) -> MultiplexerResult<serde_json::Value> {
    let pane_ids = window.cells.pane_ids_in_subtree(&window.root_cell)?;
    let panes: Vec<serde_json::Value> = pane_ids
        .iter()
        .filter_map(|pid| window.panes.get(pid).map(|p| pane_view(p, &window.id)))
        .collect();
    let layout = crate::layout_dto::build_layout(&window.root_cell, &window.cells)?;
    Ok(serde_json::json!({
        "id": window.id,
        "name": window.name,
        "root_cell": window.root_cell,
        "active_pane": window.active_pane,
        "panes": panes,
        "layout_schema_version": 1,
        "layout": layout,
    }))
}

fn pane_view(pane: &ozmux_multiplexer::Pane, wid: &WindowId) -> serde_json::Value {
    let activities: Vec<serde_json::Value> = pane
        .activities
        .iter()
        .map(|a| match &a.kind {
            ozmux_multiplexer::ActivityKind::Extension { .. } => {
                // Hierarchical iframe URL — the daemon reads (wid, pid, aid)
                // off the path to inject `window.__OZMUX__` for the SDK.
                serde_json::json!({
                    "id": a.id,
                    "kind": "extension",
                    "iframe_url": format!(
                        "/windows/{}/panes/{}/activities/{}/iframe/index.html",
                        wid, pane.id, a.id
                    ),
                })
            }
            ozmux_multiplexer::ActivityKind::Terminal => {
                serde_json::json!({ "id": a.id, "kind": "terminal" })
            }
        })
        .collect();
    serde_json::json!({
        "id": pane.id,
        "activities": activities,
        "active_activity": pane.active_activity,
    })
}

pub async fn events(
    State(state): State<AppState>,
    Path(window_id): Path<WindowId>,
    ws: WebSocketUpgrade,
) -> impl axum::response::IntoResponse {
    ws.on_upgrade(move |socket| handle_events_socket(socket, state, window_id))
}

async fn handle_events_socket(socket: WebSocket, state: AppState, window_id: WindowId) {
    let (mut tx, _rx) = socket.split();

    // Snapshot under the Window lock, then subscribe to the broadcaster
    // while still holding it. This preserves snapshot/subscribe atomicity:
    // any publish that beats the subscribe will already be in the snapshot,
    // and any publish that lands after will reach the receiver.
    let snapshot_and_rx = state.with_window(&window_id, |w| window_view_for(w)).await;
    let Some(snapshot_result) = snapshot_and_rx else {
        close_with(&mut tx, 1011, "window_not_found").await;
        return;
    };
    let snapshot = match snapshot_result {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, %window_id, "snapshot build failed");
            close_with(&mut tx, 1011, "internal_error").await;
            return;
        }
    };
    let mut receiver = state.layout_broadcast.subscribe_or_create(&window_id);

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
    use crate::test_helpers::{bootstrap_default, fresh_state, router_with};
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use ozmux_multiplexer::{Activity, ActivityId, PaneId, Side, SplitOrientation};
    use std::path::PathBuf;
    use tower::ServiceExt;

    async fn split_via_window(state: &AppState, wid: &WindowId, target: &PaneId) -> PaneId {
        let new_pane_id = PaneId::new();
        let new_activity_id = ActivityId::new();
        state
            .with_window_or_404(wid, |w| {
                w.split_pane(
                    target,
                    new_pane_id.clone(),
                    Activity::terminal(new_activity_id.clone()),
                    Side::After,
                    SplitOrientation::Horizontal,
                )
            })
            .await
            .unwrap();
        state
            .pane_owner_window
            .insert(new_pane_id.clone(), wid.clone());
        new_pane_id
    }

    #[tokio::test]
    async fn create_with_session_id_returns_201_and_attaches() {
        let state = fresh_state();
        let sid = state.create_session(None).await;
        let (router, state) = router_with(state);
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
        let sess = state.sessions.lock().await;
        assert_eq!(sess.get(&sid).unwrap().linked_windows.len(), 1);
    }

    #[tokio::test]
    async fn create_without_session_id_creates_orphan() {
        let (router, state) = router_with(fresh_state());
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
        assert_eq!(state.windows.len(), 1);
        assert_eq!(state.sessions.lock().await.len(), 0);
    }

    #[tokio::test]
    async fn create_with_unknown_session_returns_404() {
        let (router, _) = router_with(fresh_state());
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
        let state = fresh_state();
        let (wid, _, _) = state
            .create_window(None, Some("orig".into()))
            .await
            .unwrap();
        let (router, state) = router_with(state);
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
        let name = state.with_window(&wid, |w| w.name.clone()).await.unwrap();
        assert_eq!(name, "renamed");
    }

    #[tokio::test]
    async fn delete_returns_204_and_removes_window() {
        let state = fresh_state();
        let (wid, _, _) = state.create_window(None, None).await.unwrap();
        let (router, state) = router_with(state);
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
        assert!(!state.windows.contains_key(&wid));
    }

    #[tokio::test]
    async fn delete_unknown_window_returns_404() {
        let (router, _) = router_with(fresh_state());
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
        let state = fresh_state();
        let sid = state.create_session(None).await;
        let (_wid_a, _, _) = state.create_window(Some(&sid), None).await.unwrap();
        let (wid_b, _, _) = state.create_window(Some(&sid), None).await.unwrap();
        let (router, state) = router_with(state);
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
        let sess = state.sessions.lock().await;
        assert_eq!(sess.get(&sid).unwrap().active_window.as_ref(), Some(&wid_b));
    }

    #[tokio::test]
    async fn select_orphan_window_returns_409_window_not_attached() {
        let state = fresh_state();
        let (wid, _, _) = state.create_window(None, None).await.unwrap();
        let (router, _) = router_with(state);
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
        let state = fresh_state();
        let (_sid, wid, pid, aid) = bootstrap_default(&state).await;
        let root_cell = state
            .with_window(&wid, |w| w.root_cell.clone())
            .await
            .unwrap();

        let (router, _) = router_with(state);
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
        let state = fresh_state();
        let (_sid, wid, pid, _aid) = bootstrap_default(&state).await;
        let _ = split_via_window(&state, &wid, &pid).await;

        let (router, _) = router_with(state);
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
        let state = fresh_state();
        let (wid, _, _) = state
            .create_window(None, Some("orphan".into()))
            .await
            .unwrap();
        let (router, _) = router_with(state);
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
        let state = fresh_state();
        let (wid, _, _) = state
            .create_window(None, Some("orig".into()))
            .await
            .unwrap();
        let mut rx = state.layout_broadcast.subscribe_or_create(&wid);
        let (router, _state) = router_with(state);

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
        let (router, _) = router_with(fresh_state());
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
        let state = fresh_state();
        let (_sid, wid, _pid, aid) = bootstrap_default(&state).await;
        let (router, _) = router_with(state);
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
        let state = fresh_state();
        let (_sid, wid, _pid, _aid) = bootstrap_default(&state).await;
        let (router, _) = router_with(state);
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
        let state = fresh_state();
        let (_sid, wid, _pid, _aid) = bootstrap_default(&state).await;
        let (router, _) = router_with(state);
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
        let state = fresh_state();
        let (_sid, wid, pid, _aid) = bootstrap_default(&state).await;
        let (router, _) = router_with(state);
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
        let state = fresh_state();
        let (_sid, wid, pid, _aid) = bootstrap_default(&state).await;
        let _ = split_via_window(&state, &wid, &pid).await;
        let (router, _) = router_with(state);
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
        let state = fresh_state();
        let (wid, _, _) = state.create_window(None, None).await.unwrap();
        let mut rx = state.layout_broadcast.subscribe_or_create(&wid);
        let (router, _state) = router_with(state);

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

    use std::net::SocketAddr;
    use tokio::net::TcpListener as TokioTcpListener;

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

        let state = fresh_state();
        let (_sid, wid, _pid, _aid) = bootstrap_default(&state).await;
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

        let state = fresh_state();
        let (_sid, wid, _pid, _aid) = bootstrap_default(&state).await;
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
        let state = fresh_state();
        let (_sid, wid, _pid, _aid) = bootstrap_default(&state).await;
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
        let state = crate::AppState {
            layout_broadcast: crate::layout_broadcast::LayoutBroadcaster::new(1),
            ..fresh_state()
        };
        let (_sid, wid, _pid, _aid) = bootstrap_default(&state).await;
        let bc = state.layout_broadcast.clone();
        let addr = spawn_server(state).await;
        let url = format!("ws://{}/windows/{}/events", addr, wid);
        let (mut ws, _) = connect_async(&url).await.unwrap();
        let _initial = ws.next().await.unwrap().unwrap();
        bc.publish(&wid, serde_json::json!({ "n": 1 }));
        bc.publish(&wid, serde_json::json!({ "n": 2 }));
        bc.publish(&wid, serde_json::json!({ "n": 3 }));
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
        let state = fresh_state();
        let (_sid, wid, bootstrap_pane, _aid) = bootstrap_default(&state).await;
        let activity = Activity::extension(ActivityId::new(), "ext", PathBuf::from("/tmp"));
        let activity_id = activity.id.clone();
        let new_pane = PaneId::new();
        state
            .with_window_or_404(&wid, |w| {
                w.split_pane(
                    &bootstrap_pane,
                    new_pane.clone(),
                    activity,
                    Side::After,
                    SplitOrientation::Horizontal,
                )
            })
            .await
            .unwrap();
        state
            .pane_owner_window
            .insert(new_pane.clone(), wid.clone());

        let (router, _) = router_with(state);
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
            format!("/windows/{wid}/panes/{new_pane}/activities/{activity_id}/iframe/index.html")
        );
    }

    #[tokio::test]
    async fn snapshot_and_subscribe_is_atomic_under_concurrent_mutations() {
        use futures_util::StreamExt;
        use tokio_tungstenite::{connect_async, tungstenite::Message as TtMessage};

        let state = fresh_state();
        let (_sid, wid, pid, _aid) = bootstrap_default(&state).await;
        let bc = state.layout_broadcast.clone();
        let state_for_server = state.clone();
        let addr = spawn_server(state_for_server).await;

        let mut handles = vec![];
        for _ in 0..5 {
            let s = state.clone();
            let bc = bc.clone();
            let wid_ = wid.clone();
            let pid_ = pid.clone();
            handles.push(tokio::spawn(async move {
                let new_pane_id = PaneId::new();
                let new_activity_id = ActivityId::new();
                let outcome = s
                    .with_window_or_404(&wid_, |w| {
                        w.split_pane(
                            &pid_,
                            new_pane_id.clone(),
                            Activity::terminal(new_activity_id.clone()),
                            Side::After,
                            SplitOrientation::Horizontal,
                        )
                    })
                    .await;
                if outcome.is_ok() {
                    s.pane_owner_window
                        .insert(new_pane_id.clone(), wid_.clone());
                    if let Some(view) = s.with_window(&wid_, |w| super::window_view_for(w)).await
                        && let Ok(view) = view
                    {
                        bc.publish(&wid_, view);
                    }
                }
            }));
        }

        let url = format!("ws://{}/windows/{}/events", addr, wid);
        let (mut ws, _) = connect_async(&url).await.unwrap();
        let _initial = ws.next().await.unwrap().unwrap();

        for h in handles {
            let _ = h.await;
        }

        let mut latest = match _initial {
            TtMessage::Text(t) => serde_json::from_str::<serde_json::Value>(&t).unwrap(),
            _ => panic!("expected text frame"),
        };
        while let Ok(Some(Ok(TtMessage::Text(t)))) =
            tokio::time::timeout(std::time::Duration::from_millis(100), ws.next()).await
        {
            latest = serde_json::from_str::<serde_json::Value>(&t).unwrap();
        }

        let final_pane_count = state
            .with_window(&wid, |w| w.panes.len())
            .await
            .unwrap_or(0);
        let observed_pane_count = latest["panes"].as_array().map(|a| a.len()).unwrap_or(0);
        assert_eq!(
            observed_pane_count, final_pane_count,
            "client's last observed view does not match multiplexer's final state \
             (observed={observed_pane_count}, final={final_pane_count})"
        );
    }
}
