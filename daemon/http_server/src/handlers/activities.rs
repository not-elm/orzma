use crate::AppState;
use crate::error::HttpError;
use crate::extractors::ExtensionName;
use crate::handlers::publish_window_layout;
use axum::{
    extract::{
        FromRequest, Path, State, WebSocketUpgrade,
        ws::{CloseFrame, Message, WebSocket},
    },
    http::{StatusCode, header::CONTENT_TYPE},
    response::{IntoResponse, Response},
};
use futures_util::{
    SinkExt, StreamExt,
    stream::{SplitSink, SplitStream},
};
use ozmux_extension::ExtensionRegistry;
use ozmux_multiplexer::{Activity, ActivityId, ActivityKind, PaneId, SetActiveOutcome, WindowId};
use ozmux_terminal::{TerminalEvent, TerminalService};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use std::path::PathBuf;
use tokio::net::UnixStream;
use tokio::sync::broadcast;
use tokio_util::codec::{FramedRead, FramedWrite, LinesCodec};

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

fn is_allowed_origin(origin: &str) -> bool {
    matches!(
        origin,
        "http://127.0.0.1:3200"
            | "http://localhost:3200"
            | "http://127.0.0.1:5173"
            | "http://localhost:5173"
    )
}

#[derive(Deserialize)]
struct UdsEnvelope<'a> {
    #[serde(borrow)]
    frame: &'a RawValue,
}

async fn bridge(ws: WebSocket, aid: ActivityId, sock_path: std::path::PathBuf) {
    let uds = match UnixStream::connect(&sock_path).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, %aid, "handlers ws: uds connect failed");
            return;
        }
    };
    let (uds_r, uds_w) = uds.into_split();
    let (ws_tx, ws_rx) = ws.split();
    let uds_w = FramedWrite::new(uds_w, LinesCodec::new());
    let uds_r = FramedRead::new(uds_r, LinesCodec::new_with_max_length(1 << 20));

    tokio::select! {
        r = forward_ws_to_uds(ws_rx, uds_w, aid.clone()) => {
            if let Err(e) = r {
                tracing::warn!(error = %e, %aid, "handlers ws: ws→uds ended with error");
            }
        }
        r = forward_uds_to_ws(uds_r, ws_tx) => {
            if let Err(e) = r {
                tracing::warn!(error = %e, %aid, "handlers ws: uds→ws ended with error");
            }
        }
    }
}

async fn forward_ws_to_uds(
    mut ws_rx: WsStream,
    mut uds_w: FramedWrite<tokio::net::unix::OwnedWriteHalf, LinesCodec>,
    aid: ActivityId,
) -> anyhow::Result<()> {
    while let Some(msg) = ws_rx.next().await {
        match msg? {
            Message::Text(raw) => {
                if raw.contains('\n') {
                    anyhow::bail!("newline in WS text payload");
                }
                let envelope = format!(r#"{{"aid":"{}","frame":{}}}"#, aid, raw);
                uds_w.send(envelope).await?;
            }
            Message::Close(_) => return Ok(()),
            _ => continue,
        }
    }
    Ok(())
}

async fn forward_uds_to_ws(
    mut uds_r: FramedRead<tokio::net::unix::OwnedReadHalf, LinesCodec>,
    mut ws_tx: WsSink,
) -> anyhow::Result<()> {
    while let Some(line) = uds_r.next().await {
        let line = line?;
        let env: UdsEnvelope = serde_json::from_str(&line)?;
        ws_tx.send(Message::Text(env.frame.get().into())).await?;
    }
    Ok(())
}

pub async fn handlers_ws(
    State(registry): State<ExtensionRegistry>,
    Path(activity_id): Path<ActivityId>,
    req: axum::extract::Request,
) -> Result<Response, HttpError> {
    let origin = req
        .headers()
        .get(axum::http::header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !is_allowed_origin(origin) {
        return Err(HttpError::Forbidden("origin not allowed".into()));
    }
    let ext_name = registry
        .activity_owner(&activity_id)
        .ok_or_else(|| HttpError::NotFound("unknown activity".into()))?;
    let sock_path = registry
        .handlers_sock_path(&ext_name)
        .ok_or_else(|| HttpError::ServiceUnavailable("extension not running".into()))?;
    let ws = WebSocketUpgrade::from_request(req, &())
        .await
        .map_err(|e| HttpError::Forbidden(e.to_string()))?;
    let ws = ws
        .max_message_size(1 << 20)
        .max_frame_size(1 << 20)
        .max_write_buffer_size(256 << 10)
        .write_buffer_size(64 << 10);

    Ok(ws.on_upgrade(move |socket| bridge(socket, activity_id, sock_path)))
}

#[derive(Deserialize)]
pub struct CreateActivityRequest {
    html: String,
}

pub async fn create(
    ExtensionName(ext_name): ExtensionName,
    State(state): State<AppState>,
    axum::Json(body): axum::Json<CreateActivityRequest>,
) -> Result<(axum::http::StatusCode, axum::Json<serde_json::Value>), HttpError> {
    let info = state
        .extensions
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

    let activity = Activity::extension(
        ActivityId::new(),
        format!("Extension: {ext_name}"),
        html_root,
    );
    let activity_id = activity.id.clone();
    state.limbo.activities.insert(activity_id.clone(), activity);
    state
        .extensions
        .record_activity_owner(&activity_id, &ext_name);

    Ok((
        axum::http::StatusCode::CREATED,
        axum::Json(serde_json::json!({ "activity_id": activity_id })),
    ))
}

pub async fn iframe_serve(
    State(state): State<AppState>,
    Path((activity_id, path)): Path<(ActivityId, String)>,
) -> Result<Response, HttpError> {
    let activity = state.activity_metadata(&activity_id).await.ok_or_else(|| {
        HttpError::Session(ozmux_multiplexer::MultiplexerError::ActivityNotFound(
            activity_id.clone(),
        ))
    })?;
    let html_root = match &activity.kind {
        ActivityKind::Extension { html_root } => html_root.clone(),
        ActivityKind::Terminal => {
            return Err(HttpError::IframeFileNotFound(path));
        }
    };
    let html_root_canon = html_root
        .canonicalize()
        .map_err(|_| HttpError::IframeFileNotFound(path.clone()))?;
    let resolved = html_root_canon
        .join(&path)
        .canonicalize()
        .map_err(|_| HttpError::IframeFileNotFound(path.clone()))?;
    if !resolved.starts_with(&html_root_canon) {
        return Err(HttpError::InvalidHtmlPath(path));
    }
    let resolved_clone = resolved.clone();
    let bytes = tokio::task::spawn_blocking(move || std::fs::read(&resolved_clone))
        .await
        .map_err(|_| HttpError::IframeFileNotFound(path.clone()))?
        .map_err(|_| HttpError::IframeFileNotFound(path.clone()))?;
    let mime = mime_guess::from_path(&resolved).first_or_octet_stream();
    Ok((
        axum::http::StatusCode::OK,
        [(CONTENT_TYPE, mime.as_ref().to_string())],
        bytes,
    )
        .into_response())
}

#[derive(Deserialize)]
pub struct AddActivityRequest {
    activity: ActivityInput,
}

#[derive(Deserialize)]
pub struct ActivityInput {
    activity_id: ActivityId,
    #[serde(default)]
    name: Option<String>,
    kind: ActivityKindInput,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ActivityKindInput {
    Terminal,
    Extension { html_root: PathBuf },
}

impl ActivityInput {
    pub(crate) fn into_activity(self) -> Activity {
        let kind = match self.kind {
            ActivityKindInput::Terminal => ActivityKind::Terminal,
            ActivityKindInput::Extension { html_root } => ActivityKind::Extension { html_root },
        };
        Activity {
            id: self.activity_id,
            name: self.name.unwrap_or_else(|| "Activity".into()),
            kind,
        }
    }
}

pub async fn add_to_pane(
    State(state): State<AppState>,
    Path((wid, pid)): Path<(WindowId, PaneId)>,
    axum::Json(body): axum::Json<AddActivityRequest>,
) -> Result<(StatusCode, axum::Json<serde_json::Value>), HttpError> {
    let activity = body.activity.into_activity();
    let aid = activity.id.clone();
    state
        .with_window_or_404(&wid, |w| w.pane_mut(&pid)?.add_activity(activity))
        .await?;
    publish_window_layout(&state, &wid).await;
    Ok((
        StatusCode::CREATED,
        axum::Json(serde_json::json!({ "activity_id": aid })),
    ))
}

pub async fn activate_v2(
    State(state): State<AppState>,
    Path((wid, pid, aid)): Path<(WindowId, PaneId, ActivityId)>,
) -> Result<StatusCode, HttpError> {
    let outcome = state
        .with_window_or_404(&wid, |w| w.pane_mut(&pid)?.set_active_activity(&aid))
        .await?;
    if matches!(outcome, SetActiveOutcome::Changed) {
        publish_window_layout(&state, &wid).await;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppState;
    use crate::test_helpers;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use futures_util::{SinkExt, StreamExt};
    use ozmux_terminal::SpawnOptions;
    use std::path::PathBuf;
    use std::time::Duration;
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::Message as TtMessage;
    use tower::ServiceExt;

    async fn boot_server() -> (std::net::SocketAddr, AppState, ActivityId) {
        let state = test_helpers::fresh_state();
        let (_sid, _wid, pid, activity_id) = test_helpers::bootstrap_default(&state).await;
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
                    window_id: None,
                    session_id: None,
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

    async fn router_with_extension(
        ext_name: &str,
        launch_dir: PathBuf,
    ) -> (axum::Router, AppState) {
        let state = test_helpers::fresh_state();
        let _ = test_helpers::bootstrap_default(&state).await;
        state.extensions.register(ext_name, &launch_dir);
        (
            crate::test_helpers::daemon_router_for_test(state.clone()),
            state,
        )
    }

    #[tokio::test]
    async fn create_activity_returns_201_with_activity_id() {
        let tmp = tempfile::tempdir().unwrap();
        let html = tmp.path().join("index.html");
        std::fs::write(&html, "<html></html>").unwrap();
        let (router, _) = router_with_extension("memo", tmp.path().to_path_buf()).await;
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
        let (router, _) = router_with_extension("memo", tmp.path().to_path_buf()).await;
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
        let (router, _) = router_with_extension("memo", tmp.path().to_path_buf()).await;
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
        let (router, _) = router_with_extension("memo", tmp.path().to_path_buf()).await;
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

    async fn setup_extension_with_html(
        ext_name: &str,
    ) -> (axum::Router, AppState, ActivityId, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("index.html"),
            b"<html><body>memo</body></html>",
        )
        .unwrap();
        std::fs::write(tmp.path().join("style.css"), b"body { color: red; }").unwrap();
        let (router, state) = router_with_extension(ext_name, tmp.path().to_path_buf()).await;
        let activity = Activity::extension(ActivityId::new(), "ext", tmp.path().to_path_buf());
        let activity_id = activity.id.clone();
        state.limbo.activities.insert(activity_id.clone(), activity);
        state
            .extensions
            .record_activity_owner(&activity_id, ext_name);
        (router, state, activity_id, tmp)
    }

    #[tokio::test]
    async fn iframe_serve_returns_html_with_correct_content_type() {
        let (router, _state, activity_id, _tmp) = setup_extension_with_html("memo").await;
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!("/activities/{activity_id}/iframe/index.html"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let ct = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(ct.starts_with("text/html"));
    }

    #[tokio::test]
    async fn iframe_serve_returns_css_with_correct_content_type() {
        let (router, _state, activity_id, _tmp) = setup_extension_with_html("memo").await;
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!("/activities/{activity_id}/iframe/style.css"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let ct = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(ct.starts_with("text/css"));
    }

    #[tokio::test]
    async fn iframe_serve_returns_404_for_missing_file() {
        let (router, _state, activity_id, _tmp) = setup_extension_with_html("memo").await;
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!("/activities/{activity_id}/iframe/missing.html"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn iframe_serve_blocks_path_traversal() {
        let (router, _state, activity_id, tmp) = setup_extension_with_html("memo").await;
        let outside = tmp.path().parent().unwrap().join("outside.txt");
        std::fs::write(&outside, b"secret").ok();
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!("/activities/{activity_id}/iframe/../outside.txt"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(matches!(
            resp.status(),
            axum::http::StatusCode::BAD_REQUEST | axum::http::StatusCode::NOT_FOUND
        ));
        let _ = std::fs::remove_file(outside);
    }

    #[tokio::test]
    async fn iframe_for_memo_extension_returns_visible_content() {
        // Resolve the real extensions/memo/ path so this test verifies the
        // file shipped in the repo (not a tempdir copy). CARGO_MANIFEST_DIR
        // is daemon/http_server, so ../.. lands at the workspace root and
        // extensions/memo is just below it.
        let memo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../extensions/memo")
            .canonicalize()
            .expect("extensions/memo must exist relative to daemon/http_server");

        let (router, state) = router_with_extension("memo", memo_root.clone()).await;
        let activity = Activity::extension(ActivityId::new(), "ext", memo_root.clone());
        let activity_id = activity.id.clone();
        state.limbo.activities.insert(activity_id.clone(), activity);

        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!("/activities/{activity_id}/iframe/index.html"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let ct = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(ct.starts_with("text/html"), "expected text/html, got {ct}");

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_str = std::str::from_utf8(&body).unwrap();
        assert!(
            body_str.contains("Memo"),
            "expected memo HTML to contain visible 'Memo' heading, got: {body_str}"
        );
    }

    #[tokio::test]
    async fn terminal_ws_outbound_stops_after_client_close() {
        let (addr, state, activity_id) = boot_server().await;

        // Baseline before any client subscribes. The PTY bridge task holds a
        // Sender (not a Receiver), so receiver_count starts at 0. We capture
        // the actual baseline defensively in case future internals change it.
        let baseline = state.terminal.subscriber_count(&activity_id).await.unwrap();

        // Open a terminal WS via tokio_tungstenite (same pattern as
        // ws_input_is_echoed_back_in_output).
        let url = format!("ws://{addr}/activities/{activity_id}/terminal/ws");
        let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();

        // Drain initial snapshot frame so the server-side outbound task is
        // demonstrably alive holding a Receiver.
        let _ = tokio::time::timeout(std::time::Duration::from_millis(200), ws.next()).await;

        // Confirm the outbound task is subscribed.
        let with_client = state.terminal.subscriber_count(&activity_id).await.unwrap();
        assert!(
            with_client > baseline,
            "expected subscriber count to increase after client connect; baseline={baseline}, with_client={with_client}"
        );

        // Client closes the WS.
        ws.send(TtMessage::Close(None)).await.unwrap();
        drop(ws);

        // Wait up to 500ms for the daemon's abort path to drop the Receiver.
        // Without the abort fix (Task 2), the outbound task stays detached
        // and receiver_count never returns to baseline.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(500);
        loop {
            let n = state.terminal.subscriber_count(&activity_id).await.unwrap();
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

        state.terminal.kill(&activity_id).await.unwrap();
    }

    #[tokio::test]
    async fn handlers_ws_returns_404_for_unknown_activity() {
        let (router, _state) = test_helpers::router_with(test_helpers::fresh_state());
        let resp = router
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/activities/00000000-0000-0000-0000-000000000000/handlers/ws")
                    .header("origin", "http://127.0.0.1:3200")
                    .header("upgrade", "websocket")
                    .header("connection", "upgrade")
                    .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
                    .header("sec-websocket-version", "13")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn handlers_ws_returns_403_for_disallowed_origin() {
        let (router, _state) = test_helpers::router_with(test_helpers::fresh_state());
        let resp = router
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/activities/00000000-0000-0000-0000-000000000000/handlers/ws")
                    .header("origin", "http://evil.example")
                    .header("upgrade", "websocket")
                    .header("connection", "upgrade")
                    .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
                    .header("sec-websocket-version", "13")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn handlers_ws_returns_503_when_extension_not_running() {
        let state = test_helpers::fresh_state();
        let (_sid, _wid, _pid, aid) = test_helpers::bootstrap_default(&state).await;
        let registry = ozmux_extension::ExtensionRegistry::default();
        registry.register("memo", std::path::Path::new("/tmp/memo"));
        registry.record_activity_owner(&aid, "memo");
        let (router, _state) = test_helpers::router_with_registry(state, registry);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/activities/{}/handlers/ws", aid))
                    .header("origin", "http://127.0.0.1:3200")
                    .header("upgrade", "websocket")
                    .header("connection", "upgrade")
                    .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
                    .header("sec-websocket-version", "13")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn handlers_ws_round_trip_through_uds_mock() {
        use futures_util::{SinkExt, StreamExt};
        use std::time::Duration;
        use tokio::io::AsyncWriteExt;
        use tokio::net::UnixListener;

        // 1. Spin up a mock UDS listener that echoes a result frame for every line.
        let tmp = tempfile::tempdir().unwrap();
        let sock_path = tmp.path().join("memo.handlers.sock");
        let listener = UnixListener::bind(&sock_path).unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let (read_half, mut write_half) = stream.split();
            let mut framed =
                tokio_util::codec::FramedRead::new(read_half, tokio_util::codec::LinesCodec::new());
            while let Some(Ok(line)) = framed.next().await {
                #[derive(serde::Deserialize)]
                struct Env<'a> {
                    aid: String,
                    #[serde(borrow)]
                    frame: &'a serde_json::value::RawValue,
                }
                let env: Env = serde_json::from_str(&line).unwrap();
                let raw: serde_json::Value = serde_json::from_str(env.frame.get()).unwrap();
                let kind = raw["kind"].as_str().unwrap_or("");
                let id = raw["id"].as_str().unwrap_or("");
                let frames: Vec<serde_json::Value> = match kind {
                    "call" => vec![serde_json::json!({
                        "kind": "result",
                        "id": id,
                        "payload": raw["payload"],
                    })],
                    "sub.open" => {
                        let mut out = (0..2)
                            .map(|i| {
                                serde_json::json!({
                                    "kind": "sub.data",
                                    "id": id,
                                    "payload": { "i": i },
                                })
                            })
                            .collect::<Vec<_>>();
                        out.push(serde_json::json!({
                            "kind": "sub.complete",
                            "id": id,
                        }));
                        out
                    }
                    _ => continue,
                };
                for frame in frames {
                    let envelope = serde_json::json!({ "aid": env.aid, "frame": frame });
                    let line = envelope.to_string() + "\n";
                    write_half.write_all(line.as_bytes()).await.unwrap();
                }
            }
        });

        // 2. Build a router with a registry pointing at the mock sock.
        let state = test_helpers::fresh_state();
        let (_sid, _wid, _pid, aid) = test_helpers::bootstrap_default(&state).await;
        let registry = ozmux_extension::ExtensionRegistry::default();
        registry.register("memo", std::path::Path::new("/tmp/memo"));
        registry.record_activity_owner(&aid, "memo");
        registry.set_handlers_sock_path("memo", &sock_path);
        let (router, _state) = test_helpers::router_with_registry(state, registry);

        // 3. Bind an axum server on an ephemeral port and connect via tokio_tungstenite.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });

        let url = format!("ws://{}/activities/{}/handlers/ws", addr, aid);
        let req = tokio_tungstenite::tungstenite::http::Request::builder()
            .uri(&url)
            .header("host", addr.to_string())
            .header("origin", "http://127.0.0.1:3200")
            .header("upgrade", "websocket")
            .header("connection", "upgrade")
            .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
            .header("sec-websocket-version", "13")
            .body(())
            .unwrap();
        let (mut ws, _resp) = tokio_tungstenite::connect_async(req).await.unwrap();

        use tokio_tungstenite::tungstenite::Message as TMessage;
        ws.send(TMessage::Text(
            r#"{"kind":"call","id":"1","name":"x","payload":{"v":1}}"#.into(),
        ))
        .await
        .unwrap();
        let msg = tokio::time::timeout(Duration::from_secs(2), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let TMessage::Text(text) = msg else {
            panic!("expected text frame, got {:?}", msg)
        };
        let resp: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(resp["kind"], "result");
        assert_eq!(resp["id"], "1");
        assert_eq!(resp["payload"], serde_json::json!({"v": 1}));

        // sub.open → expect two sub.data then sub.complete, transparently relayed.
        ws.send(TMessage::Text(
            r#"{"kind":"sub.open","id":"s1","name":"counter","params":{}}"#.into(),
        ))
        .await
        .unwrap();
        let mut got = Vec::new();
        for _ in 0..3 {
            let msg = tokio::time::timeout(Duration::from_secs(2), ws.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            let TMessage::Text(text) = msg else {
                panic!("expected text")
            };
            got.push(serde_json::from_str::<serde_json::Value>(&text).unwrap());
        }
        assert_eq!(got[0]["kind"], "sub.data");
        assert_eq!(got[0]["id"], "s1");
        assert_eq!(got[0]["payload"], serde_json::json!({"i": 0}));
        assert_eq!(got[1]["kind"], "sub.data");
        assert_eq!(got[1]["payload"], serde_json::json!({"i": 1}));
        assert_eq!(got[2]["kind"], "sub.complete");
        assert_eq!(got[2]["id"], "s1");

        ws.close(None).await.ok();
        server.abort();
    }

    #[tokio::test]
    async fn add_to_pane_creates_tab_and_publishes() {
        let state = test_helpers::fresh_state();
        let (_sid, wid, pid, _aid) = test_helpers::bootstrap_default(&state).await;
        let mut rx = state.layout_broadcast.subscribe_or_create(&wid);
        let (router, _state) = test_helpers::router_with(state);
        let new_aid = ActivityId::new();
        let body = serde_json::json!({
            "activity": {
                "activity_id": new_aid,
                "kind": { "type": "terminal" }
            }
        });
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/windows/{wid}/panes/{pid}/activities"))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["activity_id"].as_str(), Some(new_aid.as_ref()));
        let _ = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv())
            .await
            .expect("publish timed out")
            .expect("recv error");
    }

    #[tokio::test]
    async fn add_to_pane_with_extension_kind_accepts_html_root() {
        let state = test_helpers::fresh_state();
        let (_sid, wid, pid, _aid) = test_helpers::bootstrap_default(&state).await;
        let (router, _state) = test_helpers::router_with(state);
        let new_aid = ActivityId::new();
        let body = serde_json::json!({
            "activity": {
                "activity_id": new_aid,
                "name": "memo",
                "kind": { "type": "extension", "html_root": "/tmp" }
            }
        });
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/windows/{wid}/panes/{pid}/activities"))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn add_to_pane_unknown_window_returns_404() {
        let state = test_helpers::fresh_state();
        let (_sid, _wid, pid, _aid) = test_helpers::bootstrap_default(&state).await;
        let (router, _state) = test_helpers::router_with(state);
        let bogus_wid = ozmux_multiplexer::WindowId::new();
        let body = serde_json::json!({
            "activity": {
                "activity_id": ActivityId::new(),
                "kind": { "type": "terminal" }
            }
        });
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/windows/{bogus_wid}/panes/{pid}/activities"))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn activate_v2_switches_active_activity_and_publishes() {
        let state = test_helpers::fresh_state();
        let (_sid, wid, pid, _aid_initial) = test_helpers::bootstrap_default(&state).await;
        let new_aid = ActivityId::new();
        state
            .with_window_or_404(&wid, |w| {
                w.pane_mut(&pid)?
                    .add_activity(Activity::terminal(new_aid.clone()))
            })
            .await
            .unwrap();
        let mut rx = state.layout_broadcast.subscribe_or_create(&wid);
        let (router, _state) = test_helpers::router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/windows/{wid}/panes/{pid}/activities/{new_aid}/activate"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let _ = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv())
            .await
            .expect("publish timed out")
            .expect("recv error");
    }

    #[tokio::test]
    async fn activate_v2_already_active_does_not_publish() {
        let state = test_helpers::fresh_state();
        let (_sid, wid, pid, aid) = test_helpers::bootstrap_default(&state).await;
        let mut rx = state.layout_broadcast.subscribe_or_create(&wid);
        let (router, _state) = test_helpers::router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/windows/{wid}/panes/{pid}/activities/{aid}/activate"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let res = tokio::time::timeout(std::time::Duration::from_millis(80), rx.recv()).await;
        assert!(res.is_err(), "Unchanged outcome must not publish");
    }

    #[tokio::test]
    async fn activate_v2_unknown_activity_returns_404() {
        let state = test_helpers::fresh_state();
        let (_sid, wid, pid, _aid) = test_helpers::bootstrap_default(&state).await;
        let (router, _state) = test_helpers::router_with(state);
        let phantom = ActivityId::new();
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/windows/{wid}/panes/{pid}/activities/{phantom}/activate"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
