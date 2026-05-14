use crate::{AppState, error::HttpResult};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use ozmux_multiplexer::WindowId;
use serde::Deserialize;

#[derive(Deserialize)]
pub struct RenameRequest {
    name: String,
}

pub async fn rename(
    State(state): State<AppState>,
    Path(window_id): Path<WindowId>,
    Json(body): Json<RenameRequest>,
) -> HttpResult<StatusCode> {
    state.rename_window(&window_id, body.name).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use crate::test_helpers::{fresh_state, router_with};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn rename_returns_204_and_updates_name() {
        let state = fresh_state();
        let (wid, _, _) = state
            .multiplexer
            .create_window(None, Some("orig".into()))
            .await
            .unwrap();
        let (router, state) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/windows/{}", wid))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"renamed"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let name = state
            .multiplexer
            .with_window(&wid, |w| w.name.clone())
            .await
            .unwrap();
        assert_eq!(name, "renamed");
    }

    #[tokio::test]
    async fn rename_publishes_layout_with_new_name() {
        let state = fresh_state();
        let (wid, _, _) = state
            .multiplexer
            .create_window(None, Some("orig".into()))
            .await
            .unwrap();
        let mut rx = state.layout_broadcast.subscribe_or_create(&wid);
        let (router, _state) = router_with(state);

        let resp = router
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/windows/{}", wid))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"renamed"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        let view = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv())
            .await
            .expect("publish timed out")
            .expect("recv error");
        assert_eq!(view["name"].as_str(), Some("renamed"));
    }
}
