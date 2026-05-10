use axum::{
    extract::{
        Path, State, WebSocketUpgrade,
        ws::{CloseFrame, Message, WebSocket},
    },
    response::IntoResponse,
};
use futures_util::{
    SinkExt, StreamExt,
    stream::{SplitSink, SplitStream},
};
use ozmux_multiplexer::activity::{Activity, ActivityId, ActivityKind};
use ozmux_terminal::{TerminalEvent, TerminalService};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::sync::broadcast;
use crate::error::HttpError;
use crate::extractors::ExtensionName;
use crate::MultiplexerState;
use ozmux_extension::ExtensionRegistry;

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
    let (mut ws_tx, ws_rx) = socket.split();
    let Some((snapshot, rx)) = acquire_session(&terminal, &activity_id, &mut ws_tx).await else {
        return;
    };
    if send_snapshot(&mut ws_tx, snapshot).await.is_err() {
        return;
    }
    let outbound = tokio::spawn(forward_pty_to_ws(ws_tx, rx));
    let inbound = tokio::spawn(forward_ws_to_pty(ws_rx, terminal, activity_id));
    tokio::select! {
        _ = outbound => {},
        _ = inbound => {},
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

#[derive(Deserialize)]
pub struct CreateActivityRequest {
    html: String,
}

pub async fn create(
    ExtensionName(ext_name): ExtensionName,
    State(ms): State<MultiplexerState>,
    State(registry): State<ExtensionRegistry>,
    axum::Json(body): axum::Json<CreateActivityRequest>,
) -> Result<(axum::http::StatusCode, axum::Json<serde_json::Value>), HttpError> {
    let info = registry
        .get(&ext_name)
        .ok_or_else(|| HttpError::UnknownExtension(ext_name.clone()))?;
    let html_path = PathBuf::from(&body.html)
        .canonicalize()
        .map_err(|_| HttpError::InvalidHtmlPath(body.html.clone()))?;
    let launch_dir_canon = info
        .launch_dir
        .canonicalize()
        .map_err(|_| HttpError::InvalidHtmlPath(body.html.clone()))?;
    if !html_path.starts_with(&launch_dir_canon) {
        return Err(HttpError::InvalidHtmlPath(body.html));
    }
    let html_root = html_path
        .parent()
        .ok_or_else(|| HttpError::InvalidHtmlPath(body.html.clone()))?
        .to_path_buf();

    let activity_id = {
        let mut ms = ms.lock().await;
        ms.new_activity(Activity {
            name: format!("Extension: {ext_name}"),
            kind: ActivityKind::Extension { html_root },
        })
    };
    registry.record_activity_owner(&activity_id, &ext_name);

    Ok((
        axum::http::StatusCode::CREATED,
        axum::Json(serde_json::json!({ "activity_id": activity_id })),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppState;
    use futures_util::{SinkExt, StreamExt};
    use ozmux_multiplexer::MultiplexerService;
    use ozmux_terminal::SpawnOptions;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::net::TcpListener;
    use tokio::sync::Mutex;
    use tokio_tungstenite::tungstenite::Message as TtMessage;
    use ozmux_extension::ExtensionRegistry;
    use tower::ServiceExt;

    async fn boot_server() -> (std::net::SocketAddr, AppState, ActivityId) {
        let mut ms = MultiplexerService::default();
        let (_sid, _wid, pid, activity_id) = ms.bootstrap_default().unwrap();
        let state = AppState {
            multiplexer: crate::MultiplexerState(Arc::new(Mutex::new(ms))),
            terminal: TerminalService::default(),
            extensions: ozmux_extension::ExtensionRegistry::default(),
        };
        state
            .terminal
            .spawn(
                pid,
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
        let app = crate::test_helpers::daemon_router_for_test(state.clone());
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
        ws.send(TtMessage::Binary(b"echo ws_marker_test\n".to_vec().into()))
            .await
            .unwrap();

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
        let result = tokio::time::timeout(Duration::from_millis(200), ws.next()).await;
        match result {
            Err(_) => {}
            Ok(Some(Ok(TtMessage::Binary(_)))) => {}
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
        let result = tokio::time::timeout(Duration::from_secs(2), ws.next()).await;
        match result {
            Ok(Some(Ok(TtMessage::Close(Some(frame))))) => {
                assert!(frame.reason.contains("activity not found"));
            }
            Ok(Some(Ok(TtMessage::Close(None)))) => {}
            other => panic!("expected Close frame, got: {other:?}"),
        }
    }

    fn router_with_extension(ext_name: &str, launch_dir: PathBuf) -> (axum::Router, AppState) {
        let mut ms = ozmux_multiplexer::MultiplexerService::default();
        let _ = ms.bootstrap_default().unwrap();
        let registry = ExtensionRegistry::default();
        registry.register(ext_name, &launch_dir);
        let state = AppState {
            multiplexer: crate::MultiplexerState(Arc::new(Mutex::new(ms))),
            terminal: ozmux_terminal::TerminalService::default(),
            extensions: registry,
        };
        (crate::test_helpers::daemon_router_for_test(state.clone()), state)
    }

    #[tokio::test]
    async fn create_activity_returns_201_with_activity_id() {
        let tmp = tempfile::tempdir().unwrap();
        let html = tmp.path().join("index.html");
        std::fs::write(&html, "<html></html>").unwrap();
        let (router, _) = router_with_extension("memo", tmp.path().to_path_buf());
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/activities")
                    .header("X-Ozmux-Extension", "memo")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(format!(
                        r#"{{"html":"{}"}}"#,
                        html.display()
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::CREATED);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(v["activity_id"].is_string());
    }

    #[tokio::test]
    async fn create_activity_rejects_path_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        let html = "/etc/passwd";
        let (router, _) = router_with_extension("memo", tmp.path().to_path_buf());
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/activities")
                    .header("X-Ozmux-Extension", "memo")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(format!(r#"{{"html":"{html}"}}"#)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn create_activity_rejects_unknown_extension() {
        let tmp = tempfile::tempdir().unwrap();
        let html = tmp.path().join("index.html");
        std::fs::write(&html, "<html></html>").unwrap();
        let (router, _) = router_with_extension("memo", tmp.path().to_path_buf());
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/activities")
                    .header("X-Ozmux-Extension", "ghost")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(format!(
                        r#"{{"html":"{}"}}"#,
                        html.display()
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn create_activity_requires_extension_header() {
        let tmp = tempfile::tempdir().unwrap();
        let html = tmp.path().join("index.html");
        std::fs::write(&html, "<html></html>").unwrap();
        let (router, _) = router_with_extension("memo", tmp.path().to_path_buf());
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/activities")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(format!(
                        r#"{{"html":"{}"}}"#,
                        html.display()
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::UNAUTHORIZED);
    }
}
