//! WebSocket endpoint that emits an initial `SessionView` snapshot and
//! every subsequent broadcast value as JSON text frames. Mirrors
//! `handlers::windows::events`.

use crate::AppState;
use axum::extract::{
    Path, State, WebSocketUpgrade,
    ws::{CloseFrame, Message, WebSocket},
};
use futures_util::{SinkExt, StreamExt, stream::SplitSink};
use ozmux_multiplexer::SessionId;

/// HTTP entry point: upgrade to WS and hand off to `handle_events_socket`.
pub async fn events(
    State(state): State<AppState>,
    Path(session_id): Path<SessionId>,
    ws: WebSocketUpgrade,
) -> impl axum::response::IntoResponse {
    ws.on_upgrade(move |socket| handle_events_socket(socket, state, session_id))
}

async fn handle_events_socket(socket: WebSocket, state: AppState, session_id: SessionId) {
    let (mut tx, _rx) = socket.split();
    // NOTE: subscribe BEFORE building the snapshot so no session mutation can
    // slip through the gap between snapshot and subscribe.
    let mut receiver = state.session_broadcast.subscribe_or_create(&session_id);
    let snapshot = match state.snapshot_session_view(&session_id).await {
        Some(v) => v,
        None => {
            close_with(&mut tx, 1011, "session_not_found").await;
            return;
        }
    };
    let snapshot_json = match serde_json::to_string(&snapshot) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, %session_id, "snapshot serialize failed");
            close_with(&mut tx, 1011, "internal_error").await;
            return;
        }
    };
    if tx.send(Message::Text(snapshot_json.into())).await.is_err() {
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
                close_with(&mut tx, 1011, "session_closed").await;
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
    use crate::test_helpers::fresh_state;
    use futures_util::StreamExt;
    use tokio_tungstenite::{connect_async, tungstenite::Message as TtMessage};

    async fn spawn_server(state: crate::AppState) -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
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
        let state = fresh_state();
        let sid = state.multiplexer.create_session(Some("hello".into())).await;
        state
            .multiplexer
            .create_window(Some(&sid), Some("w".into()))
            .await
            .unwrap();
        let addr = spawn_server(state).await;

        let url = format!("ws://{}/sessions/{}/events", addr, sid);
        let (mut ws, _) = connect_async(&url).await.unwrap();
        let msg = ws.next().await.unwrap().unwrap();
        let text = match msg {
            TtMessage::Text(t) => t,
            other => panic!("expected text frame, got {other:?}"),
        };
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["name"].as_str(), Some("hello"));
        assert_eq!(v["windows"][0]["name"].as_str(), Some("w"));
    }

    #[tokio::test]
    async fn events_ws_closes_with_session_not_found_for_unknown_sid() {
        let state = fresh_state();
        let addr = spawn_server(state).await;
        let url = format!("ws://{}/sessions/does-not-exist/events", addr);
        let (mut ws, _) = connect_async(&url).await.unwrap();
        match ws.next().await.unwrap().unwrap() {
            TtMessage::Close(Some(frame)) => {
                assert_eq!(u16::from(frame.code), 1011);
                assert!(frame.reason.contains("session_not_found"));
            }
            other => panic!("expected close frame, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn events_ws_sends_frame_after_external_publish() {
        let state = fresh_state();
        let sid = state.multiplexer.create_session(Some("orig".into())).await;
        let addr = spawn_server(state.clone()).await;
        let url = format!("ws://{}/sessions/{}/events", addr, sid);
        let (mut ws, _) = connect_async(&url).await.unwrap();
        let _snapshot = ws.next().await.unwrap().unwrap();

        state
            .multiplexer
            .rename_session(&sid, "renamed".into())
            .await
            .unwrap();
        state.publish_session_view(&sid).await;

        let msg = ws.next().await.unwrap().unwrap();
        let text = match msg {
            TtMessage::Text(t) => t,
            other => panic!("expected text frame, got {other:?}"),
        };
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["name"].as_str(), Some("renamed"));
    }
}
