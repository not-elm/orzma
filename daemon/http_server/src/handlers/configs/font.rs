//! `GET /configs/font` returns the merged font configuration.

use axum::Json;
use axum::extract::State;
use ozmux_configs::OzmuxConfigs;
use ozmux_configs::font::FontConfig;
use std::sync::Arc;

/// Returns the active font configuration as JSON.
pub async fn get(State(configs): State<Arc<OzmuxConfigs>>) -> Json<FontConfig> {
    Json(configs.font.clone())
}

#[cfg(test)]
mod tests {
    use crate::daemon_router;
    use crate::test_helpers::fresh_state;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use http_body_util::BodyExt;
    use ozmux_configs::font::FontConfig;
    use tower::ServiceExt;

    #[tokio::test]
    async fn get_returns_default_font_as_json() {
        let response = daemon_router(fresh_state())
            .oneshot(
                Request::builder()
                    .uri("/configs/font")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let content_type = response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(
            content_type.starts_with("application/json"),
            "got content-type {content_type}"
        );

        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let round_trip: FontConfig = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(round_trip, FontConfig::default());
    }
}
