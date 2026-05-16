//! `PATCH /windows/{wid}/dimensions` — record the client's measured
//! cell-grid dimensions for the window. Required input for the
//! resize-pane algorithm.

use crate::{
    AppState,
    error::{HttpError, HttpResult},
};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use ozmux_multiplexer::WindowId;
use serde::Deserialize;

/// Body for `PATCH /windows/{wid}/dimensions`. Both fields are required
/// and must be `>= 1`; zero is rejected with `INVALID_DIMENSIONS`.
#[derive(Deserialize)]
pub struct DimensionsRequest {
    cols: u16,
    rows: u16,
}

/// Validate the request body and forward to
/// [`AppState::set_window_dimensions`]. Returns `204 No Content` on
/// success, `422 INVALID_DIMENSIONS` for zero cols/rows, and `404
/// WINDOW_NOT_FOUND` when `wid` does not exist.
pub async fn patch_dimensions(
    State(state): State<AppState>,
    Path(window_id): Path<WindowId>,
    Json(body): Json<DimensionsRequest>,
) -> HttpResult<StatusCode> {
    if body.cols == 0 {
        return Err(HttpError::InvalidDimensions { field: "cols" });
    }
    if body.rows == 0 {
        return Err(HttpError::InvalidDimensions { field: "rows" });
    }
    state
        .set_window_dimensions(&window_id, body.cols, body.rows)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use crate::test_helpers::{bootstrap_default, fresh_state, router_with};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use ozmux_multiplexer::{MultiplexerError, WindowDimensions, WindowId};
    use tower::ServiceExt;

    #[tokio::test]
    async fn patch_dimensions_returns_204_and_stores_value() {
        let state = fresh_state();
        let (_sid, wid, _pid, _aid) = bootstrap_default(&state).await;
        let (router, state2) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/windows/{}/dimensions", wid))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"cols":120,"rows":40}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let dims = state2
            .multiplexer
            .with_window_or_404(&wid, |w| Ok::<_, MultiplexerError>(w.dimensions.clone()))
            .await
            .unwrap();
        assert_eq!(
            dims,
            Some(WindowDimensions {
                cols: 120,
                rows: 40
            })
        );
    }

    #[tokio::test]
    async fn patch_dimensions_unknown_window_returns_404() {
        let state = fresh_state();
        let (router, _) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/windows/{}/dimensions", WindowId::new()))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"cols":80,"rows":24}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn patch_dimensions_zero_cols_returns_422() {
        let state = fresh_state();
        let (_sid, wid, _pid, _aid) = bootstrap_default(&state).await;
        let (router, _) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/windows/{}/dimensions", wid))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"cols":0,"rows":24}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn patch_dimensions_zero_rows_returns_422() {
        let state = fresh_state();
        let (_sid, wid, _pid, _aid) = bootstrap_default(&state).await;
        let (router, _) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/windows/{}/dimensions", wid))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"cols":80,"rows":0}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn patch_dimensions_malformed_json_returns_400() {
        let state = fresh_state();
        let (_sid, wid, _pid, _aid) = bootstrap_default(&state).await;
        let (router, _) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/windows/{}/dimensions", wid))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"cols":"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}
