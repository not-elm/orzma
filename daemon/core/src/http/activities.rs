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
