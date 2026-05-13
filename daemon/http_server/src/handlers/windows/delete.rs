use crate::{AppState, error::HttpResult};
use axum::{
    extract::{Path, State},
    http::StatusCode,
};
use ozmux_multiplexer::WindowId;

pub async fn delete(
    State(state): State<AppState>,
    Path(window_id): Path<WindowId>,
) -> HttpResult<StatusCode> {
    state.close_window(&window_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use crate::test_helpers::{fresh_state, router_with};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn delete_returns_204_and_removes_window() {
        let state = fresh_state();
        let (wid, _, _) = state.multiplexer.create_window(None, None).await.unwrap();
        let (router, state) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/windows/{}", wid))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert!(!state.multiplexer.windows.contains_key(&wid));
    }

    #[tokio::test]
    async fn delete_unknown_window_returns_404() {
        let (router, _) = router_with(fresh_state());
        let resp = router
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/windows/missing")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn delete_window_kicks_subscribers_with_recv_closed() {
        use tokio::sync::broadcast::error::RecvError;
        let state = fresh_state();
        let (wid, _, _) = state.multiplexer.create_window(None, None).await.unwrap();
        let mut rx = state.layout_broadcast.subscribe_or_create(&wid);
        let (router, _state) = router_with(state);

        let resp = router
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/windows/{}", wid))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        let err = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv())
            .await
            .expect("recv timed out")
            .expect_err("expected RecvError::Closed");
        assert!(matches!(err, RecvError::Closed));
    }
}
