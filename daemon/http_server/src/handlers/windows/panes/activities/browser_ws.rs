//! WebSocket handler for browser activities. Streams `BrowserServerMsg`s to
//! the frontend and receives `BrowserClientMsg`s. Uses a `watch::Receiver`
//! (latest-frame semantics) for the server-push side instead of the replay
//! ring used by the terminal VT handler.

use crate::error::{HttpError, HttpResult};
use crate::state::{ActivityKindDiscriminant, AppState};
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{FromRequest, Path, State, WebSocketUpgrade};
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use ozmux_browser::{BrowserClientMsg, BrowserServerMsg};
use ozmux_multiplexer::{ActivityId, PaneId, WindowId};

/// `GET /windows/{wid}/panes/{pid}/activities/{aid}/browser/ws`
///
/// Validates origin and activity kind, then upgrades to WebSocket and starts
/// the browser frame bridge. Sends the current snapshot immediately on
/// connect, then streams subsequent snapshots as they arrive.
pub async fn browser_ws(
    State(state): State<AppState>,
    Path((wid, pid, aid)): Path<(WindowId, PaneId, ActivityId)>,
    req: axum::extract::Request,
) -> HttpResult<Response> {
    let origin = req
        .headers()
        .get(axum::http::header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !crate::origin_guard::is_allowed_origin(origin) {
        return Err(HttpError::ForbiddenOrigin);
    }
    let _activity = state
        .ensure_activity_kind(&wid, &pid, &aid, ActivityKindDiscriminant::Browser)
        .await?;
    let ws = WebSocketUpgrade::from_request(req, &())
        .await
        .map_err(|e| HttpError::Forbidden(e.to_string()))?;
    Ok(ws.on_upgrade(move |socket| run(socket, state, aid)))
}

async fn run(socket: WebSocket, state: AppState, aid: ActivityId) {
    let Some(mut snapshot_rx) = state.browser.watch(&aid).await else {
        return;
    };
    let (mut tx, mut rx) = socket.split();

    let initial = snapshot_rx.borrow().clone();
    if let Some(frame) = &initial.frame {
        let msg = BrowserServerMsg::Screencast {
            jpeg: frame.jpeg.clone(),
            width: frame.width,
            height: frame.height,
        };
        if let Ok(bin) = rmp_serde::to_vec_named(&msg)
            && tx.send(Message::Binary(bin.into())).await.is_err()
        {
            return;
        }
    }
    let nav_msg = BrowserServerMsg::Nav {
        url: initial.nav.url.clone(),
        title: initial.nav.title.clone(),
    };
    if let Ok(bin) = rmp_serde::to_vec_named(&nav_msg)
        && tx.send(Message::Binary(bin.into())).await.is_err()
    {
        return;
    }

    loop {
        tokio::select! {
            res = snapshot_rx.changed() => {
                if res.is_err() { break; }
                let s = snapshot_rx.borrow_and_update().clone();
                if let Some(f) = &s.frame {
                    let msg = BrowserServerMsg::Screencast {
                        jpeg: f.jpeg.clone(),
                        width: f.width,
                        height: f.height,
                    };
                    if let Ok(bin) = rmp_serde::to_vec_named(&msg)
                        && tx.send(Message::Binary(bin.into())).await.is_err()
                    {
                        break;
                    }
                }
                let msg = BrowserServerMsg::Nav {
                    url: s.nav.url.clone(),
                    title: s.nav.title.clone(),
                };
                if let Ok(bin) = rmp_serde::to_vec_named(&msg)
                    && tx.send(Message::Binary(bin.into())).await.is_err()
                {
                    break;
                }
            }
            msg = rx.next() => {
                let Some(Ok(Message::Binary(b))) = msg else { break };
                let Ok(client) = rmp_serde::from_slice::<BrowserClientMsg>(&b) else { continue };
                match client {
                    BrowserClientMsg::Nav { nav } => state.browser.navigate(&aid, nav).await,
                    BrowserClientMsg::Resize { width, height } => {
                        state.browser.resize(&aid, width, height).await;
                    }
                    BrowserClientMsg::CopyRequest => {
                        if let Some(text) = state.browser.request_selection(&aid).await {
                            let msg = BrowserServerMsg::ClipboardWrite { text };
                            if let Ok(bin) = rmp_serde::to_vec_named(&msg) {
                                let _ = tx.send(Message::Binary(bin.into())).await;
                            }
                        }
                    }
                    other => state.browser.send_input(&aid, other).await,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::test_helpers;
    use axum::body::Body;
    use axum::http::Request;
    use ozmux_multiplexer::ActivityId;
    use tower::ServiceExt;

    #[tokio::test]
    async fn browser_ws_rejects_terminal_activity_with_409() {
        let state = test_helpers::fresh_state();
        let (_sid, wid, pid, term_aid) = test_helpers::bootstrap_default(&state).await;
        let (router, _state) = test_helpers::router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!(
                        "/windows/{wid}/panes/{pid}/activities/{term_aid}/browser/ws"
                    ))
                    .header("origin", "http://127.0.0.1:3200")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn browser_ws_rejects_unknown_activity_with_404() {
        let state = test_helpers::fresh_state();
        let (_sid, wid, pid, _aid) = test_helpers::bootstrap_default(&state).await;
        let (router, _state) = test_helpers::router_with(state);
        let phantom_aid = ActivityId::new();
        let resp = router
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!(
                        "/windows/{wid}/panes/{pid}/activities/{phantom_aid}/browser/ws"
                    ))
                    .header("origin", "http://127.0.0.1:3200")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn browser_ws_rejects_disallowed_origin_with_403() {
        let state = test_helpers::fresh_state();
        let (_sid, wid, pid, aid) = test_helpers::bootstrap_default(&state).await;
        let (router, _state) = test_helpers::router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!(
                        "/windows/{wid}/panes/{pid}/activities/{aid}/browser/ws"
                    ))
                    .header("origin", "http://evil.example.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::FORBIDDEN);
    }
}
