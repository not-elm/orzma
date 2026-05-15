use crate::AppState;
use crate::error::HttpError;
use crate::state::ActivityKindDiscriminant;
use axum::{
    extract::{FromRequest, Path, Query, State, WebSocketUpgrade},
    response::Response,
};
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
pub async fn terminal_ws(
    State(state): State<AppState>,
    Path((wid, pid, aid)): Path<(WindowId, PaneId, ActivityId)>,
    Query(params): Query<TerminalWsParams>,
    req: axum::extract::Request,
) -> Result<Response, HttpError> {
    if let Some(origin) = req.headers().get(axum::http::header::ORIGIN) {
        let s = origin.to_str().map_err(|_| HttpError::ForbiddenOrigin)?;
        if !crate::origin_guard::is_allowed_origin(s) {
            return Err(HttpError::ForbiddenOrigin);
        }
    }
    let _activity = state
        .ensure_activity_kind(&wid, &pid, &aid, ActivityKindDiscriminant::Terminal)
        .await?;

    const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;
    let ws = WebSocketUpgrade::from_request(req, &())
        .await
        .map_err(|e| HttpError::Forbidden(e.to_string()))?
        .max_message_size(MAX_FRAME_BYTES);

    let terminal = state.terminal.clone();
    let last_seq = params.last_seq;
    Ok(ws.on_upgrade(move |socket| super::vt_ws::vt_ws_loop(socket, terminal, aid, last_seq)))
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
        assert!(res.is_err(), "expected upgrade failure, got Ok");
    }
}
