use crate::AppState;
use crate::error::HttpError;
use axum::{
    extract::{
        FromRequest, Path, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::Response,
};
use futures_util::{
    SinkExt, StreamExt,
    stream::{SplitSink, SplitStream},
};
use ozmux_multiplexer::{ActivityId, PaneId, WindowId};
use serde::Deserialize;
use serde_json::value::RawValue;
use tokio::net::UnixStream;
use tokio_util::codec::{FramedRead, FramedWrite, LinesCodec};

type WsSink = SplitSink<WebSocket, Message>;
type WsStream = SplitStream<WebSocket>;

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
    state
        .ensure_activity_in_pane_in_window(&wid, &pid, &aid)
        .await?;
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

#[cfg(test)]
mod tests {
    use crate::test_helpers;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use ozmux_multiplexer::{Activity, ActivityId};
    use tower::ServiceExt;

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
            super::super::test_support::setup_hierarchical_extension(b"<html></html>").await;
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
            super::super::test_support::setup_hierarchical_extension(b"<html></html>").await;
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
            .multiplexer
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
}
