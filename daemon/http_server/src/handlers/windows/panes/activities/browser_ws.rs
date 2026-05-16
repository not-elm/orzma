//! Legacy chromiumoxide WebSocket handler stub. Replaced in Task C8 by the
//! cef-backed handler (browser_cef_ws → browser_ws rename). This file now
//! only exists so the route table compiles while both modules are still
//! declared; it is deleted in the C8 commit.

use crate::error::{HttpError, HttpResult};
use crate::state::{ActivityKindDiscriminant, AppState};
use axum::extract::{FromRequest, Path, State, WebSocketUpgrade};
use axum::response::Response;
use ozmux_multiplexer::{ActivityId, PaneId, WindowId};

/// `GET /windows/{wid}/panes/{pid}/activities/{aid}/browser/ws`
///
/// Stub — returns 501 until C8 renames `browser_cef_ws` over this file.
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
    let _ws = WebSocketUpgrade::from_request(req, &())
        .await
        .map_err(|e| HttpError::Forbidden(e.to_string()))?;
    Err(HttpError::Internal(
        "browser_ws: stub — use browser_cef/ws until C8 is committed".into(),
    ))
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
