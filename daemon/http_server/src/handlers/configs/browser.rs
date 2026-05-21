//! `GET /configs/browser` returns the merged browser-activity configuration.

use axum::Json;
use axum::extract::State;
use ozmux_configs::OzmuxConfigs;
use ozmux_configs::browser::BrowserConfig;
use std::sync::Arc;

/// Returns the active browser configuration as JSON.
pub async fn get(State(configs): State<Arc<OzmuxConfigs>>) -> Json<BrowserConfig> {
    Json(configs.browser.clone())
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
    use ozmux_configs::browser::BrowserConfig;
    use tower::ServiceExt;

    #[tokio::test]
    async fn get_returns_default_browser_config_as_json() {
        let response = daemon_router(fresh_state())
            .oneshot(
                Request::builder()
                    .uri("/configs/browser")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let round_trip: BrowserConfig = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(round_trip, BrowserConfig::default());
    }
}
