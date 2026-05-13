use crate::AppState;
use crate::error::HttpError;
use crate::handlers::ensure_activity_in_pane_in_window;
use axum::{
    body::Body,
    extract::{
        Path, Query, State, WebSocketUpgrade,
        ws::{CloseFrame, Message, WebSocket},
    },
    http::{StatusCode, header},
    response::Response,
};
use futures_util::{
    SinkExt, StreamExt,
    stream::{SplitSink, SplitStream},
};
use ozmux_multiplexer::{ActivityId, PaneId, WindowId};
use ozmux_terminal::{TerminalEvent, TerminalService};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

type WsSink = SplitSink<WebSocket, Message>;
type WsStream = SplitStream<WebSocket>;

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

/// Query parameters for the terminal WebSocket endpoint.
#[derive(Deserialize)]
pub struct TerminalWsParams {
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub vt_version: Option<String>,
    #[serde(default)]
    pub last_seq: Option<u32>,
}

/// Terminal WebSocket: validates (window, pane, activity) membership, then
/// upgrades and bridges PTY bytes both ways. Internal routing is keyed by
/// ActivityId; the path includes (wid, pid) so URLs are self-describing for
/// the SDK and pre-upgrade authorization is straightforward.
pub async fn terminal_ws(
    State(state): State<AppState>,
    Path((wid, pid, aid)): Path<(WindowId, PaneId, ActivityId)>,
    Query(params): Query<TerminalWsParams>,
    ws: WebSocketUpgrade,
) -> Result<Response, HttpError> {
    ensure_activity_in_pane_in_window(&state, &wid, &pid, &aid).await?;

    const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;
    let ws = ws.max_message_size(MAX_FRAME_BYTES);

    if params.mode.as_deref() == Some("vt") {
        match params.vt_version.as_deref() {
            Some("vt-1") => {
                let terminal = state.terminal.clone();
                let last_seq = params.last_seq;
                return Ok(ws.on_upgrade(move |socket| {
                    super::vt_ws::vt_ws_loop(socket, terminal, aid, last_seq)
                }));
            }
            other => {
                let body = serde_json::json!({ "supported_versions": ["vt-1"] }).to_string();
                let mut builder = Response::builder()
                    .status(StatusCode::UPGRADE_REQUIRED)
                    .header(header::CONTENT_TYPE, "application/json")
                    .header("X-Ozmux-VT-Version-Server", "vt-1");
                if let Some(client_ver) = other {
                    builder = builder.header("X-Ozmux-VT-Version-Client", client_ver);
                }
                return Ok(builder.body(Body::from(body)).unwrap());
            }
        }
    }

    let terminal = state.terminal.clone();
    Ok(ws.on_upgrade(move |socket| handle_terminal_socket(socket, terminal, aid)))
}

async fn handle_terminal_socket(
    socket: WebSocket,
    terminal: TerminalService,
    activity_id: ActivityId,
) {
    let (mut ws_tx, ws_rx) = socket.split();
    let Some((snapshot, rx)) = acquire_session(&terminal, &activity_id, &mut ws_tx).await else {
        return;
    };
    if send_snapshot(&mut ws_tx, snapshot).await.is_err() {
        return;
    }
    let mut outbound = tokio::spawn(forward_pty_to_ws(ws_tx, rx));
    let mut inbound = tokio::spawn(forward_ws_to_pty(ws_rx, terminal, activity_id));
    tokio::select! {
        res = &mut outbound => {
            log_join_error(res, "outbound");
            inbound.abort();
            if let Err(e) = inbound.await
                && !e.is_cancelled() {
                    tracing::warn!(error = %e, side = "inbound", "task ended with error after abort");
                }
        }
        res = &mut inbound => {
            log_join_error(res, "inbound");
            outbound.abort();
            if let Err(e) = outbound.await
                && !e.is_cancelled() {
                    tracing::warn!(error = %e, side = "outbound", "task ended with error after abort");
                }
        }
    }
}

fn log_join_error<T>(res: Result<T, tokio::task::JoinError>, side: &'static str) {
    if let Err(e) = res {
        if e.is_panic() {
            tracing::error!(side, "task panicked");
        } else if !e.is_cancelled() {
            tracing::warn!(side, error = %e, "task ended with JoinError");
        }
    }
}

async fn acquire_session(
    terminal: &TerminalService,
    activity_id: &ActivityId,
    ws_tx: &mut WsSink,
) -> Option<(Vec<u8>, broadcast::Receiver<TerminalEvent>)> {
    match terminal.snapshot_and_subscribe(activity_id).await {
        Ok(pair) => Some(pair),
        Err(_) => {
            let _ = ws_tx
                .send(Message::Close(Some(CloseFrame {
                    code: 1011,
                    reason: "activity not found".into(),
                })))
                .await;
            None
        }
    }
}

async fn send_snapshot(ws_tx: &mut WsSink, snapshot: Vec<u8>) -> Result<(), axum::Error> {
    if snapshot.is_empty() {
        return Ok(());
    }
    ws_tx.send(Message::Binary(snapshot.into())).await
}

async fn forward_pty_to_ws(mut ws_tx: WsSink, mut rx: broadcast::Receiver<TerminalEvent>) {
    loop {
        match rx.recv().await {
            Ok(TerminalEvent::Data { buffer }) => {
                if ws_tx.send(Message::Binary(buffer.into())).await.is_err() {
                    break;
                }
            }
            Ok(TerminalEvent::Exit { code }) => {
                send_exit_and_close(&mut ws_tx, code).await;
                break;
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                close_with_reason(&mut ws_tx, "lagged", n).await;
                break;
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

async fn forward_ws_to_pty(
    mut ws_rx: WsStream,
    terminal: TerminalService,
    activity_id: ActivityId,
) {
    while let Some(msg) = ws_rx.next().await {
        match msg {
            Ok(Message::Binary(bytes)) => {
                if terminal.write(&activity_id, &bytes).await.is_err() {
                    break;
                }
            }
            Ok(Message::Text(text)) => apply_client_control(&terminal, &activity_id, &text).await,
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(_) => break,
        }
    }
}

async fn apply_client_control(terminal: &TerminalService, activity_id: &ActivityId, text: &str) {
    let Ok(ClientControl::Resize { cols, rows }) = serde_json::from_str::<ClientControl>(text)
    else {
        return;
    };
    let _ = terminal.resize(activity_id, cols, rows).await;
}

async fn send_exit_and_close(ws_tx: &mut WsSink, code: Option<i32>) {
    let payload = serde_json::to_string(&ServerControl::Exit { code })
        .expect("ServerControl::Exit is always serializable");
    let _ = ws_tx.send(Message::Text(payload.into())).await;
    let _ = ws_tx.send(Message::Close(None)).await;
}

async fn close_with_reason(ws_tx: &mut WsSink, reason: &'static str, lagged: u64) {
    tracing::warn!(lagged, reason, "closing ws");
    let _ = ws_tx
        .send(Message::Close(Some(CloseFrame {
            code: 1011,
            reason: reason.into(),
        })))
        .await;
}

#[cfg(test)]
mod tests {
    use ozmux_multiplexer::ActivityId;
    use std::time::Duration;
    use tokio_tungstenite::tungstenite::Message as TtMessage;

    use futures_util::{SinkExt, StreamExt};
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn terminal_ws_round_trip_echoes_input() {
        let (addr, state, wid, pid, aid) = super::super::test_support::boot_server_full().await;
        let url = format!("ws://{addr}/windows/{wid}/panes/{pid}/activities/{aid}/terminal/ws");
        let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
        ws.send(TtMessage::Binary(b"echo ws_hier_marker\n".to_vec().into()))
            .await
            .unwrap();
        let mut got = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(300), ws.next()).await {
                Ok(Some(Ok(TtMessage::Binary(bytes)))) => {
                    got.extend_from_slice(&bytes);
                    if got
                        .windows(b"ws_hier_marker".len())
                        .any(|w| w == b"ws_hier_marker")
                    {
                        break;
                    }
                }
                Ok(Some(Ok(_))) => continue,
                Ok(None) | Ok(Some(Err(_))) => break,
                Err(_) => continue,
            }
        }
        state.terminal.kill(&aid).await.unwrap();
        let s = String::from_utf8_lossy(&got);
        assert!(s.contains("ws_hier_marker"), "expected marker, got: {s}");
    }

    #[tokio::test]
    async fn terminal_ws_resize_message_does_not_close_connection() {
        let (addr, state, wid, pid, aid) = super::super::test_support::boot_server_full().await;
        let url = format!("ws://{addr}/windows/{wid}/panes/{pid}/activities/{aid}/terminal/ws");
        let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
        ws.send(TtMessage::Text(
            r#"{"type":"resize","cols":120,"rows":40}"#.into(),
        ))
        .await
        .unwrap();
        let result = tokio::time::timeout(Duration::from_millis(200), ws.next()).await;
        match result {
            Err(_) => {}
            Ok(Some(Ok(TtMessage::Binary(_)))) => {}
            Ok(Some(Ok(TtMessage::Close(_)))) => panic!("connection closed unexpectedly"),
            other => panic!("unexpected: {other:?}"),
        }
        state.terminal.kill(&aid).await.unwrap();
    }

    #[tokio::test]
    async fn terminal_ws_rejects_unknown_activity_in_pane() {
        let (router, _state, wid, pid, _aid, _tmp) =
            super::super::test_support::setup_hierarchical_extension(b"<html></html>").await;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let phantom_aid = ActivityId::new();
        let url =
            format!("ws://{addr}/windows/{wid}/panes/{pid}/activities/{phantom_aid}/terminal/ws");
        let res = tokio_tungstenite::connect_async(url).await;
        // The upgrade should fail because the activity is not in the pane.
        assert!(res.is_err(), "expected upgrade failure, got Ok");
    }

    #[tokio::test]
    async fn vt_mode_rejects_unknown_version_with_426() {
        let (addr, _state, wid, pid, aid) = super::super::test_support::boot_server_full().await;
        let url = format!(
            "ws://{addr}/windows/{wid}/panes/{pid}/activities/{aid}/terminal/ws?mode=vt&vt_version=vt-99"
        );
        let err = tokio_tungstenite::connect_async(&url)
            .await
            .expect_err("expected upgrade failure with 426");
        let tokio_tungstenite::tungstenite::Error::Http(resp) = err else {
            panic!("expected tungstenite::Error::Http, got: {err:?}");
        };
        assert_eq!(resp.status().as_u16(), 426);
        assert_eq!(
            resp.headers()
                .get("X-Ozmux-VT-Version-Server")
                .and_then(|h| h.to_str().ok()),
            Some("vt-1")
        );
        let body_bytes = resp.into_body().unwrap_or_default();
        let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(body["supported_versions"], serde_json::json!(["vt-1"]));
    }

    #[tokio::test]
    async fn vt_mode_upgrades_with_correct_version() {
        let (addr, _state, wid, pid, aid) = super::super::test_support::boot_server_full().await;
        let url = format!(
            "ws://{addr}/windows/{wid}/panes/{pid}/activities/{aid}/terminal/ws?mode=vt&vt_version=vt-1"
        );
        let (ws, resp) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("ws upgrade should succeed");
        assert_eq!(resp.status().as_u16(), 101);
        drop(ws);
    }

    #[tokio::test]
    async fn terminal_ws_outbound_stops_after_client_close() {
        let (addr, state, wid, pid, aid) = super::super::test_support::boot_server_full().await;

        // Baseline before any client subscribes. The PTY bridge task holds a
        // Sender (not a Receiver), so receiver_count starts at 0.
        let baseline = state.terminal.subscriber_count(&aid).await.unwrap();

        let url = format!("ws://{addr}/windows/{wid}/panes/{pid}/activities/{aid}/terminal/ws");
        let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();

        // Drain initial snapshot so the server-side outbound task is
        // demonstrably alive holding a Receiver.
        let _ = tokio::time::timeout(std::time::Duration::from_millis(200), ws.next()).await;

        let with_client = state.terminal.subscriber_count(&aid).await.unwrap();
        assert!(
            with_client > baseline,
            "expected subscriber count to increase after client connect; baseline={baseline}, with_client={with_client}"
        );

        ws.send(TtMessage::Close(None)).await.unwrap();
        drop(ws);

        // Wait up to 500ms for the daemon's abort path to drop the Receiver.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(500);
        loop {
            let n = state.terminal.subscriber_count(&aid).await.unwrap();
            if n <= baseline {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                panic!(
                    "outbound task still subscribed 500ms after close; baseline={baseline}, current={n}"
                );
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }

        state.terminal.kill(&aid).await.unwrap();
    }
}
