use crate::AppState;
use crate::error::HttpError;
use crate::state::ActivityKindDiscriminant;
use axum::{
    extract::{FromRequest, Path, Query, State, WebSocketUpgrade},
    response::Response,
};
#[cfg(not(debug_assertions))]
use axum::response::IntoResponse;
use ozmux_multiplexer::{ActivityId, PaneId, WindowId};
use serde::Deserialize;

/// Query parameters for the terminal WebSocket endpoint.
#[derive(Deserialize)]
pub struct TerminalWsParams {
    #[serde(default)]
    pub last_seq: Option<u32>,
}

/// Terminal WebSocket: validates (window, pane, activity) membership, then
/// upgrades and runs the VT frame bridge. Internal routing is keyed by
/// ActivityId; the path includes (wid, pid) so URLs are self-describing for
/// the SDK and pre-upgrade authorization is straightforward.
///
/// In debug builds, a `?replay=<fixture>` query parameter diverts the
/// connection to the deterministic PTY-tape replay harness (see
/// `ozmux_terminal::testing`) instead of the live PTY bridge. In release
/// builds the same query parameter returns HTTP 404 so the debug-only path
/// cannot be invoked in production.
pub async fn terminal_ws(
    State(state): State<AppState>,
    Path((wid, pid, aid)): Path<(WindowId, PaneId, ActivityId)>,
    Query(params): Query<TerminalWsParams>,
    req: axum::extract::Request,
) -> Result<Response, HttpError> {
    let origin = req
        .headers()
        .get(axum::http::header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !crate::origin_guard::is_allowed_origin(origin) {
        return Err(HttpError::ForbiddenOrigin);
    }
    let _activity = state
        .ensure_activity_kind(&wid, &pid, &aid, ActivityKindDiscriminant::Terminal)
        .await?;

    let replay_fixture = req
        .uri()
        .query()
        .and_then(|q| {
            url::form_urlencoded::parse(q.as_bytes())
                .find(|(k, _)| k == "replay")
                .map(|(_, v)| v.into_owned())
        });

    if let Some(fixture) = replay_fixture {
        #[cfg(debug_assertions)]
        {
            const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;
            let ws = WebSocketUpgrade::from_request(req, &())
                .await
                .map_err(|e| HttpError::Forbidden(e.to_string()))?
                .max_message_size(MAX_FRAME_BYTES);
            return Ok(ws.on_upgrade(move |socket| run_replay_session(fixture, socket)));
        }
        #[cfg(not(debug_assertions))]
        {
            let _ = fixture;
            return Ok(axum::http::StatusCode::NOT_FOUND.into_response());
        }
    }

    const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;
    let ws = WebSocketUpgrade::from_request(req, &())
        .await
        .map_err(|e| HttpError::Forbidden(e.to_string()))?
        .max_message_size(MAX_FRAME_BYTES);

    let terminal = state.terminal.clone();
    let last_seq = params.last_seq;
    Ok(ws.on_upgrade(move |socket| super::vt_ws::vt_ws_loop(socket, terminal, aid, last_seq)))
}

/// Debug-only background replay session: drives the named PTY tape through
/// the test replay harness and forwards each resulting WireMessage to the
/// connected client. Incoming client frames are ignored while replay is in
/// progress; the replay task is cancelled if the connection drops first.
#[cfg(debug_assertions)]
async fn run_replay_session(fixture: String, ws: axum::extract::ws::WebSocket) {
    use futures_util::{SinkExt, StreamExt};
    use ozmux_terminal::testing::replay::{ReplayMode, feed_pty_tape};
    use ozmux_terminal::testing::tape::Tape;
    use ozmux_terminal::vt::WireMessage;
    use tokio_util::task::AbortOnDropHandle;

    let (mut ws_tx, mut ws_rx) = ws.split();

    let task = tokio::spawn(async move {
        let tape_path = std::path::PathBuf::from(format!(
            "daemon/terminal/tests/fixtures/pty_tapes/{fixture}.tape"
        ));
        let tape = match Tape::load(&tape_path) {
            Ok(t) => t,
            Err(e) => {
                tracing::error!(?fixture, error = %e, "?replay=: tape load failed");
                return;
            }
        };
        match feed_pty_tape(&tape, ReplayMode::Immediate).await {
            Ok(msgs) => {
                tracing::info!(?fixture, frames = msgs.len(), "?replay=: completed");
                for msg in msgs {
                    let frame = match msg {
                        WireMessage::Binary { encoded, .. } => {
                            axum::extract::ws::Message::Binary(encoded)
                        }
                        WireMessage::Text(s) => axum::extract::ws::Message::Text(s.into()),
                    };
                    if ws_tx.send(frame).await.is_err() {
                        break;
                    }
                }
            }
            Err(e) => tracing::error!(?fixture, error = %e, "?replay=: feed_pty_tape failed"),
        }
    });
    let _guard = AbortOnDropHandle::new(task);

    while let Some(_msg) = ws_rx.next().await {
        // Ignore client frames; replay runs in the background.
    }
}

#[cfg(test)]
mod tests {
    use ozmux_multiplexer::ActivityId;
    use tokio::net::TcpListener;
    use tower::ServiceExt;

    #[tokio::test]
    async fn terminal_ws_rejects_disallowed_origin() {
        let (router, _state, wid, pid, aid, _tmp) =
            super::super::test_support::setup_hierarchical_extension(b"<html></html>").await;
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri(format!(
                        "/windows/{wid}/panes/{pid}/activities/{aid}/terminal/ws"
                    ))
                    .header("origin", "http://evil.example.com")
                    .header("upgrade", "websocket")
                    .header("connection", "upgrade")
                    .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
                    .header("sec-websocket-version", "13")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn terminal_ws_rejects_missing_origin() {
        let (router, _state, wid, pid, aid, _tmp) =
            super::super::test_support::setup_hierarchical_extension(b"<html></html>").await;
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri(format!(
                        "/windows/{wid}/panes/{pid}/activities/{aid}/terminal/ws"
                    ))
                    // No Origin header — must be denied just as an explicit disallowed origin.
                    .header("upgrade", "websocket")
                    .header("connection", "upgrade")
                    .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
                    .header("sec-websocket-version", "13")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::FORBIDDEN);
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
        let res = tokio_tungstenite::connect_async(req).await;
        assert!(res.is_err(), "expected upgrade failure, got Ok");
    }
}
