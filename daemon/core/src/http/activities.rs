use crate::{
    pty::{TerminalEvent, TerminalService},
    session::activity::ActivityId,
};
use axum::{
    extract::{
        Path, State, WebSocketUpgrade,
        ws::{CloseFrame, Message, WebSocket},
    },
    response::IntoResponse,
};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum ClientControl {
    Resize { cols: u16, rows: u16 },
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum ServerControl {
    Exit { code: Option<i32> },
}

pub async fn terminal_ws(
    State(terminal): State<TerminalService>,
    Path(activity_id): Path<ActivityId>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_terminal_socket(socket, terminal, activity_id))
}

async fn handle_terminal_socket(
    socket: WebSocket,
    terminal: TerminalService,
    activity_id: ActivityId,
) {
    let (mut ws_tx, mut ws_rx) = socket.split();

    // Refined-A: race-free snapshot + subscribe (single critical section in
    // ScrollbackBuffer, see pty/pty_handle.rs).
    let (snapshot, mut rx) = match terminal.snapshot_and_subscribe(&activity_id).await {
        Ok(pair) => pair,
        Err(_) => {
            let _ = ws_tx
                .send(Message::Close(Some(CloseFrame {
                    code: 1011,
                    reason: "activity not found".into(),
                })))
                .await;
            return;
        }
    };

    // 1) Send snapshot as one binary frame.
    if !snapshot.is_empty() {
        if ws_tx.send(Message::Binary(snapshot.into())).await.is_err() {
            return;
        }
    }

    // 2) Outbound task: broadcast → ws frames.
    let outbound = tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(TerminalEvent::Data { buffer }) => {
                    if ws_tx.send(Message::Binary(buffer.into())).await.is_err() {
                        break;
                    }
                }
                Ok(TerminalEvent::Exit { code }) => {
                    let payload = serde_json::to_string(&ServerControl::Exit { code }).unwrap();
                    let _ = ws_tx.send(Message::Text(payload.into())).await;
                    let _ = ws_tx.send(Message::Close(None)).await;
                    break;
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(lagged = n, "ws receiver lagged, closing");
                    let _ = ws_tx
                        .send(Message::Close(Some(CloseFrame {
                            code: 1011,
                            reason: "lagged".into(),
                        })))
                        .await;
                    break;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    // 3) Inbound task: ws frames → terminal write/resize.
    let inbound_terminal = terminal.clone();
    let inbound_activity = activity_id.clone();
    let inbound = tokio::spawn(async move {
        while let Some(msg) = ws_rx.next().await {
            match msg {
                Ok(Message::Binary(bytes)) => {
                    if inbound_terminal
                        .write(&inbound_activity, &bytes)
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Ok(Message::Text(text)) => {
                    if let Ok(ClientControl::Resize { cols, rows }) =
                        serde_json::from_str::<ClientControl>(&text)
                    {
                        let _ = inbound_terminal
                            .resize(&inbound_activity, cols, rows)
                            .await;
                    }
                }
                Ok(Message::Close(_)) => break,
                Ok(_) => {} // Ping/Pong handled by axum
                Err(_) => break,
            }
        }
    });

    tokio::select! {
        _ = outbound => {},
        _ = inbound => {},
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::AppState;
    use crate::pty::SpawnOptions;
    use futures_util::{SinkExt, StreamExt};
    use std::time::Duration;
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::Message as TtMessage;

    async fn boot_server() -> (std::net::SocketAddr, AppState, ActivityId) {
        let state = AppState::default();
        let activity_id = state.sessions.bootstrap_default().await;
        state
            .terminal
            .spawn(
                activity_id.clone(),
                SpawnOptions {
                    cols: 80,
                    rows: 24,
                    shell: "/bin/sh".to_string(),
                    cwd: None,
                },
            )
            .await
            .unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = crate::http::test_helpers::daemon_router_for_test(state.clone());
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (addr, state, activity_id)
    }

    #[tokio::test]
    async fn ws_input_is_echoed_back_in_output() {
        let (addr, state, activity_id) = boot_server().await;
        let url = format!("ws://{addr}/activities/{activity_id}/terminal/ws");
        let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();

        // Send input that produces a unique marker in output.
        ws.send(TtMessage::Binary(b"echo ws_marker_test\n".to_vec().into()))
            .await
            .unwrap();

        // Drain frames up to ~3s, looking for marker.
        let mut got = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(300), ws.next()).await {
                Ok(Some(Ok(TtMessage::Binary(bytes)))) => {
                    got.extend_from_slice(&bytes);
                    if got
                        .windows(b"ws_marker_test".len())
                        .any(|w| w == b"ws_marker_test")
                    {
                        break;
                    }
                }
                Ok(Some(Ok(_))) => continue,
                Ok(None) | Ok(Some(Err(_))) => break,
                Err(_) => continue,
            }
        }

        state.terminal.kill(&activity_id).await.unwrap();
        let s = String::from_utf8_lossy(&got);
        assert!(s.contains("ws_marker_test"), "expected marker, got: {s}");
    }

    #[tokio::test]
    async fn ws_resize_message_does_not_close_connection() {
        let (addr, state, activity_id) = boot_server().await;
        let url = format!("ws://{addr}/activities/{activity_id}/terminal/ws");
        let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();

        ws.send(TtMessage::Text(
            r#"{"type":"resize","cols":120,"rows":40}"#.into(),
        ))
        .await
        .unwrap();

        // After resize, connection should still be alive (give it 200ms).
        let result = tokio::time::timeout(Duration::from_millis(200), ws.next()).await;
        // Either we got data (still alive) or timeout (no data yet, still alive).
        // We just need to confirm the connection wasn't closed.
        match result {
            Err(_) => { /* timeout, connection still open */ }
            Ok(Some(Ok(TtMessage::Binary(_)))) => { /* alive */ }
            Ok(Some(Ok(TtMessage::Close(_)))) => panic!("connection closed unexpectedly"),
            other => panic!("unexpected: {other:?}"),
        }

        state.terminal.kill(&activity_id).await.unwrap();
    }

    #[tokio::test]
    async fn ws_to_unknown_activity_closes_with_close_frame() {
        let (addr, _state, _) = boot_server().await;
        let bogus = ActivityId::new();
        let url = format!("ws://{addr}/activities/{bogus}/terminal/ws");
        let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();

        // Should receive Close frame quickly.
        let result = tokio::time::timeout(Duration::from_secs(2), ws.next()).await;
        match result {
            Ok(Some(Ok(TtMessage::Close(Some(frame))))) => {
                assert!(frame.reason.contains("activity not found"));
            }
            Ok(Some(Ok(TtMessage::Close(None)))) => { /* also acceptable */ }
            other => panic!("expected Close frame, got: {other:?}"),
        }
    }
}
