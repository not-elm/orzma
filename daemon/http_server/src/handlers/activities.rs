use crate::AppState;
use crate::error::HttpError;
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

/// Terminal WebSocket: validates (window, pane, activity) membership, then
/// upgrades and bridges PTY bytes both ways. Internal routing is keyed by
/// ActivityId; the path includes (wid, pid) so URLs are self-describing for
/// the SDK and pre-upgrade authorization is straightforward.
pub async fn terminal_ws(
    State(state): State<AppState>,
    Path((wid, pid, aid)): Path<(WindowId, PaneId, ActivityId)>,
    ws: WebSocketUpgrade,
) -> Result<Response, HttpError> {
    ensure_activity_in_pane_in_window(&state, &wid, &pid, &aid).await?;
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

/// Extension handlers WebSocket: validates (window, pane, activity) membership,
/// then bridges JSON-line frames to the owning extension's UDS. Internal routing
/// is keyed by ActivityId.
pub async fn handlers_ws(
    State(state): State<AppState>,
    Path((wid, pid, aid)): Path<(WindowId, PaneId, ActivityId)>,
    req: axum::extract::Request,
) -> Result<Response, HttpError> {
    ensure_activity_in_pane_in_window(&state, &wid, &pid, &aid).await?;
    let origin = req
        .headers()
        .get(axum::http::header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !is_allowed_origin(origin) {
        return Err(HttpError::Forbidden("origin not allowed".into()));
    }
    let ext_name = state
        .extensions
        .activity_owner(&aid)
        .ok_or_else(|| HttpError::NotFound("unknown activity".into()))?;
    let sock_path = state
        .extensions
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
    Ok(ws.on_upgrade(move |socket| bridge(socket, aid, sock_path)))
}

/// Validates (window, pane, activity) membership and injects
/// `window.__OZMUX__` globals into HTML responses so the iframe SDK can
/// discover its position in the hierarchy without parsing the URL.
pub async fn iframe_serve(
    State(state): State<AppState>,
    Path((wid, pid, aid, path)): Path<(WindowId, PaneId, ActivityId, String)>,
) -> Result<Response, HttpError> {
    ensure_activity_in_pane_in_window(&state, &wid, &pid, &aid).await?;
    let activity = activity_for_iframe(&state, &aid, &path).await?;
    let session_id = crate::handlers::panes::session_owning_window(&state, &wid).await;
    let ids = OzmuxIds {
        session_id: session_id.map(|s| s.to_string()),
        window_id: wid.to_string(),
        pane_id: pid.to_string(),
        activity_id: aid.to_string(),
    };
    serve_iframe_asset(&activity, &path, Some(&ids)).await
}

async fn activity_for_iframe(
    state: &AppState,
    aid: &ActivityId,
    path: &str,
) -> Result<Activity, HttpError> {
    let activity = state.activity_metadata(aid).await.ok_or_else(|| {
        HttpError::Session(ozmux_multiplexer::MultiplexerError::ActivityNotFound(
            aid.clone(),
        ))
    })?;
    if !matches!(activity.kind, ActivityKind::Extension { .. }) {
        return Err(HttpError::IframeFileNotFound(path.to_string()));
    }
    Ok(activity)
}

async fn serve_iframe_asset(
    activity: &Activity,
    path: &str,
    ctx: Option<&OzmuxIds>,
) -> Result<Response, HttpError> {
    let ActivityKind::Extension { html_root } = &activity.kind else {
        return Err(HttpError::IframeFileNotFound(path.to_string()));
    };
    let html_root_canon = html_root
        .canonicalize()
        .map_err(|_| HttpError::IframeFileNotFound(path.to_string()))?;
    let resolved = html_root_canon
        .join(path)
        .canonicalize()
        .map_err(|_| HttpError::IframeFileNotFound(path.to_string()))?;
    if !resolved.starts_with(&html_root_canon) {
        return Err(HttpError::InvalidHtmlPath(path.to_string()));
    }
    let resolved_clone = resolved.clone();
    let path_owned = path.to_string();
    let bytes = tokio::task::spawn_blocking(move || std::fs::read(&resolved_clone))
        .await
        .map_err(|_| HttpError::IframeFileNotFound(path_owned.clone()))?
        .map_err(|_| HttpError::IframeFileNotFound(path_owned))?;
    let mime = mime_guess::from_path(&resolved).first_or_octet_stream();
    // Only HTML responses carry the bootstrap script. Other assets (CSS, JS,
    // fonts, images) are served byte-for-byte so caching and integrity checks
    // stay intact.
    if let Some(ids) = ctx
        && mime.essence_str() == "text/html"
    {
        let body = String::from_utf8_lossy(&bytes);
        let injected = inject_ozmux_globals(&body, ids);
        return Ok((
            axum::http::StatusCode::OK,
            [(CONTENT_TYPE, mime.as_ref().to_string())],
            injected,
        )
            .into_response());
    }
    Ok((
        axum::http::StatusCode::OK,
        [(CONTENT_TYPE, mime.as_ref().to_string())],
        bytes,
    )
        .into_response())
}

async fn ensure_activity_in_pane_in_window(
    state: &AppState,
    wid: &WindowId,
    pid: &PaneId,
    aid: &ActivityId,
) -> Result<(), HttpError> {
    let owner = state
        .pane_owner_window
        .get(pid)
        .map(|e| e.clone())
        .ok_or_else(|| {
            HttpError::Session(ozmux_multiplexer::MultiplexerError::PaneNotFound(
                pid.clone(),
            ))
        })?;
    if &owner != wid {
        return Err(HttpError::Session(
            ozmux_multiplexer::MultiplexerError::PaneNotInWindow {
                window: wid.clone(),
                pane: pid.clone(),
            },
        ));
    }
    let has_activity = state
        .with_window(wid, |w| {
            w.pane(pid).map(|p| p.has_activity(aid)).unwrap_or(false)
        })
        .await
        .ok_or_else(|| {
            HttpError::Session(ozmux_multiplexer::MultiplexerError::WindowNotFound(
                wid.clone(),
            ))
        })?;
    if !has_activity {
        return Err(HttpError::Session(
            ozmux_multiplexer::MultiplexerError::ActivityNotInPane {
                pane: pid.clone(),
                activity: aid.clone(),
            },
        ));
    }
    Ok(())
}

#[derive(Serialize)]
struct OzmuxIds {
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    #[serde(rename = "windowId")]
    window_id: String,
    #[serde(rename = "paneId")]
    pane_id: String,
    #[serde(rename = "activityId")]
    activity_id: String,
}

/// Inject `<script>window.__OZMUX__={...}</script>` into the iframe HTML so the
/// SDK can read its position in the hierarchy without parsing the URL.
///
/// Injection order: after `<head>` (preferred — lands before any user script),
/// else after `<html ...>` (so the script is still in document order), else
/// prepend (degraded fallback for headless/fragmentary HTML).
fn inject_ozmux_globals(html: &str, ctx: &OzmuxIds) -> String {
    let payload = serde_json::to_string(ctx).expect("OzmuxIds is always serializable");
    let script = format!("<script>window.__OZMUX__={payload};</script>");
    if let Some(pos) = html.find("<head>") {
        let cut = pos + "<head>".len();
        return format!("{}{}{}", &html[..cut], script, &html[cut..]);
    }
    if let Some(pos) = html.find("<html")
        && let Some(end) = html[pos..].find('>')
    {
        let cut = pos + end + 1;
        return format!("{}{}{}", &html[..cut], script, &html[cut..]);
    }
    format!("{script}{html}")
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
    Extension {
        html_root: PathBuf,
        /// Owning extension's name. The daemon uses this to populate the
        /// `ExtensionRegistry` so subsequent iframe / handlers-WS requests
        /// can route to the right extension UDS. Required for the Extension
        /// variant; the SDK fills it from the bootstrap-time `EXTENSION_NAME`
        /// env var.
        extension_name: String,
    },
}

impl ActivityInput {
    /// Convert the wire payload into a domain `Activity`, also surfacing the
    /// owning extension's name for Extension-kind activities. The name is
    /// consumed by the handler (to register ownership in `ExtensionRegistry`)
    /// and not stored on `Activity` itself, since the multiplexer model has no
    /// notion of an "owner".
    pub(crate) fn into_activity(self) -> (Activity, Option<String>) {
        let (kind, ext_name) = match self.kind {
            ActivityKindInput::Terminal => (ActivityKind::Terminal, None),
            ActivityKindInput::Extension {
                html_root,
                extension_name,
            } => (ActivityKind::Extension { html_root }, Some(extension_name)),
        };
        let activity = Activity {
            id: self.activity_id,
            name: self.name.unwrap_or_else(|| "Activity".into()),
            kind,
        };
        (activity, ext_name)
    }
}

pub async fn add_to_pane(
    State(state): State<AppState>,
    Path((wid, pid)): Path<(WindowId, PaneId)>,
    axum::Json(body): axum::Json<AddActivityRequest>,
) -> Result<(StatusCode, axum::Json<serde_json::Value>), HttpError> {
    let (activity, ext_name) = body.activity.into_activity();
    let aid = activity.id.clone();
    state
        .with_window_or_404(&wid, |w| w.pane_mut(&pid)?.add_activity(activity))
        .await?;
    if let Some(name) = ext_name.as_deref() {
        state.extensions.record_activity_owner(&aid, name);
    }
    publish_window_layout(&state, &wid).await;
    Ok((
        StatusCode::CREATED,
        axum::Json(serde_json::json!({ "activity_id": aid })),
    ))
}

pub async fn activate(
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
    use std::time::Duration;
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::Message as TtMessage;
    use tower::ServiceExt;

    /// Boot a full daemon router with the bootstrap session and a PTY spawned
    /// for the initial activity. Returns the listen address plus the IDs of the
    /// bootstrap (window, pane, activity).
    async fn boot_server_full() -> (std::net::SocketAddr, AppState, WindowId, PaneId, ActivityId) {
        let state = test_helpers::fresh_state();
        let (_sid, wid, pid, activity_id) = test_helpers::bootstrap_default(&state).await;
        state
            .terminal
            .spawn(
                pid.clone(),
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
        (addr, state, wid, pid, activity_id)
    }

    /// Build a router with the bootstrap session plus an extension Activity
    /// hosted inside the initial Pane so the hierarchical iframe / WS routes
    /// can validate (wid, pid, aid) and serve files from `html_root`.
    async fn setup_hierarchical_extension(
        html_body: &[u8],
    ) -> (
        axum::Router,
        AppState,
        WindowId,
        PaneId,
        ActivityId,
        tempfile::TempDir,
    ) {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("index.html"), html_body).unwrap();
        std::fs::write(tmp.path().join("style.css"), b"body { color: red; }").unwrap();
        let state = test_helpers::fresh_state();
        let (_sid, wid, pid, _initial_aid) = test_helpers::bootstrap_default(&state).await;
        state.extensions.register("memo", tmp.path());
        let activity = Activity::extension(ActivityId::new(), "ext", tmp.path().to_path_buf());
        let aid = activity.id.clone();
        state
            .with_window_or_404(&wid, |w| w.pane_mut(&pid)?.add_activity(activity))
            .await
            .unwrap();
        state.extensions.record_activity_owner(&aid, "memo");
        let (router, _) = test_helpers::router_with(state.clone());
        (router, state, wid, pid, aid, tmp)
    }

    #[tokio::test]
    async fn terminal_ws_round_trip_echoes_input() {
        let (addr, state, wid, pid, aid) = boot_server_full().await;
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
        let (addr, state, wid, pid, aid) = boot_server_full().await;
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
            setup_hierarchical_extension(b"<html></html>").await;
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
    async fn terminal_ws_outbound_stops_after_client_close() {
        let (addr, state, wid, pid, aid) = boot_server_full().await;

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

    #[tokio::test]
    async fn iframe_serve_returns_html_with_correct_content_type() {
        let (router, _state, wid, pid, aid, _tmp) =
            setup_hierarchical_extension(b"<html><body>memo</body></html>").await;
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!(
                        "/windows/{wid}/panes/{pid}/activities/{aid}/iframe/index.html"
                    ))
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
        let (router, _state, wid, pid, aid, _tmp) =
            setup_hierarchical_extension(b"<html><body>memo</body></html>").await;
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!(
                        "/windows/{wid}/panes/{pid}/activities/{aid}/iframe/style.css"
                    ))
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
        let (router, _state, wid, pid, aid, _tmp) =
            setup_hierarchical_extension(b"<html></html>").await;
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!(
                        "/windows/{wid}/panes/{pid}/activities/{aid}/iframe/missing.html"
                    ))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn iframe_serve_blocks_path_traversal() {
        let (router, _state, wid, pid, aid, tmp) =
            setup_hierarchical_extension(b"<html></html>").await;
        let outside = tmp.path().parent().unwrap().join("outside.txt");
        std::fs::write(&outside, b"secret").ok();
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!(
                        "/windows/{wid}/panes/{pid}/activities/{aid}/iframe/../outside.txt"
                    ))
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

        let state = test_helpers::fresh_state();
        let (_sid, wid, pid, _initial_aid) = test_helpers::bootstrap_default(&state).await;
        state.extensions.register("memo", &memo_root);
        let activity = Activity::extension(ActivityId::new(), "ext", memo_root.clone());
        let aid = activity.id.clone();
        state
            .with_window_or_404(&wid, |w| w.pane_mut(&pid)?.add_activity(activity))
            .await
            .unwrap();
        state.extensions.record_activity_owner(&aid, "memo");
        let (router, _) = test_helpers::router_with(state.clone());

        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!(
                        "/windows/{wid}/panes/{pid}/activities/{aid}/iframe/index.html"
                    ))
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
    async fn iframe_html_contains_ozmux_globals_script() {
        let (router, _state, wid, pid, aid, _tmp) =
            setup_hierarchical_extension(b"<html><head></head><body>memo</body></html>").await;
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!(
                        "/windows/{wid}/panes/{pid}/activities/{aid}/iframe/index.html"
                    ))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_str = std::str::from_utf8(&body).unwrap();
        assert!(
            body_str.contains("window.__OZMUX__"),
            "expected globals script, got: {body_str}"
        );
        // The injected payload must use camelCase keys with the IDs that came
        // off the URL — that is the contract the iframe SDK depends on.
        assert!(
            body_str.contains(&format!("\"windowId\":\"{wid}\"")),
            "wid: {body_str}"
        );
        assert!(
            body_str.contains(&format!("\"paneId\":\"{pid}\"")),
            "pid: {body_str}"
        );
        assert!(
            body_str.contains(&format!("\"activityId\":\"{aid}\"")),
            "aid: {body_str}"
        );
        // The injection must land inside <head> so it runs before any user
        // script tag that appears later in the document.
        let head_pos = body_str.find("<head>").unwrap();
        let script_pos = body_str.find("window.__OZMUX__").unwrap();
        let body_tag = body_str.find("<body>").unwrap();
        assert!(head_pos < script_pos && script_pos < body_tag);
    }

    #[tokio::test]
    async fn iframe_injection_falls_back_to_html_when_head_missing() {
        let (router, _state, wid, pid, aid, _tmp) =
            setup_hierarchical_extension(b"<html lang=\"en\"><body>no head</body></html>").await;
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!(
                        "/windows/{wid}/panes/{pid}/activities/{aid}/iframe/index.html"
                    ))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_str = std::str::from_utf8(&body).unwrap();
        assert!(body_str.contains("window.__OZMUX__"));
        let html_open_end = body_str.find('>').unwrap();
        let script_pos = body_str.find("window.__OZMUX__").unwrap();
        assert!(html_open_end < script_pos);
    }

    #[tokio::test]
    async fn iframe_injection_skips_non_html_assets() {
        let (router, _state, wid, pid, aid, _tmp) =
            setup_hierarchical_extension(b"<html><head></head></html>").await;
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!(
                        "/windows/{wid}/panes/{pid}/activities/{aid}/iframe/style.css"
                    ))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(!std::str::from_utf8(&body).unwrap().contains("__OZMUX__"));
    }

    #[tokio::test]
    async fn iframe_rejects_mismatched_window() {
        let (router, _state, _wid, pid, aid, _tmp) =
            setup_hierarchical_extension(b"<html><head></head></html>").await;
        let phantom_wid = WindowId::new();
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!(
                        "/windows/{phantom_wid}/panes/{pid}/activities/{aid}/iframe/index.html"
                    ))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // pane_owner_window says (pid → real wid) which mismatches phantom_wid.
        assert_eq!(resp.status(), axum::http::StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn handlers_ws_returns_404_for_unknown_activity() {
        let state = test_helpers::fresh_state();
        let (_sid, wid, pid, _aid) = test_helpers::bootstrap_default(&state).await;
        let (router, _state) = test_helpers::router_with(state);
        let phantom_aid = ActivityId::new();
        let resp = router
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!(
                        "/windows/{wid}/panes/{pid}/activities/{phantom_aid}/handlers/ws"
                    ))
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
        let (router, _state, wid, pid, aid, _tmp) =
            setup_hierarchical_extension(b"<html></html>").await;
        let resp = router
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!(
                        "/windows/{wid}/panes/{pid}/activities/{aid}/handlers/ws"
                    ))
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
        // setup_hierarchical_extension registers an extension but never sets a
        // handlers sock path, so the route should fail with 503.
        let (router, _state, wid, pid, aid, _tmp) =
            setup_hierarchical_extension(b"<html></html>").await;
        let resp = router
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!(
                        "/windows/{wid}/panes/{pid}/activities/{aid}/handlers/ws"
                    ))
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

        // 2. Build a router with a registry pointing at the mock sock, and
        //    seat an extension Activity inside the bootstrap Pane so the
        //    hierarchical path validation passes.
        let state = test_helpers::fresh_state();
        let (_sid, wid, pid, _initial_aid) = test_helpers::bootstrap_default(&state).await;
        let registry = ozmux_extension::ExtensionRegistry::default();
        registry.register("memo", std::path::Path::new("/tmp/memo"));
        let aid = ActivityId::new();
        state
            .with_window_or_404(&wid, |w| {
                w.pane_mut(&pid)?.add_activity(Activity::extension(
                    aid.clone(),
                    "ext",
                    "/tmp/memo".into(),
                ))
            })
            .await
            .unwrap();
        registry.record_activity_owner(&aid, "memo");
        registry.set_handlers_sock_path("memo", &sock_path);
        let (router, _state) = test_helpers::router_with_registry(state, registry);

        // 3. Bind an axum server on an ephemeral port and connect via tokio_tungstenite.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });

        let url = format!("ws://{addr}/windows/{wid}/panes/{pid}/activities/{aid}/handlers/ws");
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
                "kind": {
                    "type": "extension",
                    "html_root": "/tmp",
                    "extension_name": "memo"
                }
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
    async fn add_to_pane_extension_kind_records_activity_owner_in_registry() {
        let state = test_helpers::fresh_state();
        let (_sid, wid, pid, _aid) = test_helpers::bootstrap_default(&state).await;
        // The handler reads ownership info off `state.extensions`, so register
        // the extension up-front. The wire `extension_name` is what drives
        // `record_activity_owner` — the registration we're verifying.
        state
            .extensions
            .register("memo", std::path::Path::new("/tmp"));
        let registry = state.extensions.clone();
        let (router, _state) = test_helpers::router_with(state);
        let new_aid = ActivityId::new();
        let body = serde_json::json!({
            "activity": {
                "activity_id": new_aid,
                "name": "memo",
                "kind": {
                    "type": "extension",
                    "html_root": "/tmp",
                    "extension_name": "memo"
                }
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
        assert_eq!(registry.activity_owner(&new_aid).as_deref(), Some("memo"));
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
    async fn activate_switches_active_activity_and_publishes() {
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
    async fn activate_already_active_does_not_publish() {
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
    async fn activate_unknown_activity_returns_404() {
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
