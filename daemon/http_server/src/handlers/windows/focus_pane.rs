//! `POST /windows/{wid}/focus-pane` — moves focus to the geometric neighbor
//! in the requested direction.

use crate::{AppState, error::HttpResult};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use ozmux_multiplexer::{PaneDirection, WindowId};
use serde::Deserialize;

/// Body for `POST /windows/{wid}/focus-pane`. Carries the cardinal direction
/// to move focus toward, relative to the window's currently active pane.
#[derive(Deserialize)]
pub struct FocusPaneRequest {
    direction: PaneDirection,
}

/// Move focus to the geometric neighbor of the active pane in the requested
/// `direction`. Returns `204 No Content` whether or not the active pane
/// changed (single-pane windows and direction with no neighbor are no-ops).
pub async fn focus_pane(
    State(state): State<AppState>,
    Path(window_id): Path<WindowId>,
    Json(req): Json<FocusPaneRequest>,
) -> HttpResult<StatusCode> {
    state
        .focus_pane_direction(&window_id, req.direction)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use crate::test_helpers::{bootstrap_default, fresh_state, router_with};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use ozmux_multiplexer::{
        Activity, ActivityId, MultiplexerError, PaneId, Side, SplitOrientation, WindowId,
    };
    use tower::ServiceExt;

    async fn split_via_window(
        state: &crate::AppState,
        wid: &WindowId,
        target: &PaneId,
        orient: SplitOrientation,
    ) -> PaneId {
        let new_pane_id = PaneId::new();
        let new_activity_id = ActivityId::new();
        state
            .multiplexer
            .with_window_or_404(wid, |w| {
                w.split_pane(
                    target,
                    new_pane_id.clone(),
                    Activity::terminal(new_activity_id.clone()),
                    Side::After,
                    orient,
                )
            })
            .await
            .unwrap();
        state
            .multiplexer
            .pane_owner_window
            .insert(new_pane_id.clone(), wid.clone());
        new_pane_id
    }

    #[tokio::test]
    async fn focus_pane_right_returns_204_and_updates_active_pane() {
        let state = fresh_state();
        let (_sid, wid, left, _aid) = bootstrap_default(&state).await;
        let right = split_via_window(&state, &wid, &left, SplitOrientation::Horizontal).await;
        let _ = state
            .multiplexer
            .with_window_or_404(&wid, |w| w.set_active_pane(&left))
            .await
            .unwrap();
        let (router, state2) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/windows/{}/focus-pane", wid))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"direction":"right"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let active = state2
            .multiplexer
            .with_window_or_404(&wid, |w| Ok::<_, MultiplexerError>(w.active_pane.clone()))
            .await
            .unwrap();
        assert_eq!(active, right);
    }

    #[tokio::test]
    async fn focus_pane_single_pane_returns_204_without_change() {
        let state = fresh_state();
        let (_sid, wid, pid, _aid) = bootstrap_default(&state).await;
        let (router, state2) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/windows/{}/focus-pane", wid))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"direction":"left"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let active = state2
            .multiplexer
            .with_window_or_404(&wid, |w| Ok::<_, MultiplexerError>(w.active_pane.clone()))
            .await
            .unwrap();
        assert_eq!(active, pid);
    }

    #[tokio::test]
    async fn focus_pane_unknown_window_returns_404() {
        let state = fresh_state();
        let _ = bootstrap_default(&state).await;
        let (router, _) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/windows/{}/focus-pane", WindowId::new()))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"direction":"right"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn focus_pane_unknown_direction_returns_422() {
        let state = fresh_state();
        let (_sid, wid, _pid, _aid) = bootstrap_default(&state).await;
        let (router, _) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/windows/{}/focus-pane", wid))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"direction":"sideways"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn focus_pane_malformed_json_returns_400() {
        let state = fresh_state();
        let (_sid, wid, _pid, _aid) = bootstrap_default(&state).await;
        let (router, _) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/windows/{}/focus-pane", wid))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"direction":"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}
