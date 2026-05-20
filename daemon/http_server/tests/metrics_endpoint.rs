//! Smoke test for the `OZMUX_METRICS=1`-gated /metrics endpoint.
//!
//! NOTE: The global `OnceLock<PrometheusHandle>` is permanent once set.
//! Tests are named with numeric prefixes so that the pre-install case
//! (`01_`) runs before the post-install case (`02_`) under the
//! alphabetical ordering that Rust's test harness uses with
//! `--test-threads=1`.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

#[tokio::test]
async fn t01_metrics_returns_404_when_env_unset() {
    // SAFETY: tests run serial (--test-threads=1 in CI).
    unsafe {
        std::env::remove_var("OZMUX_METRICS");
    }
    let app = build_test_router();
    let response = app
        .oneshot(Request::get("/metrics").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn t02_metrics_returns_200_when_env_set() {
    // SAFETY: tests run serial.
    unsafe {
        std::env::set_var("OZMUX_METRICS", "1");
    }
    let _ = ozmux_http_server::handlers::metrics::maybe_install();
    let app = build_test_router();
    let response = app
        .oneshot(Request::get("/metrics").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    unsafe {
        std::env::remove_var("OZMUX_METRICS");
    }
}

fn build_test_router() -> axum::Router {
    use axum::routing::get;
    axum::Router::new().route(
        "/metrics",
        get(ozmux_http_server::handlers::metrics::metrics_handler),
    )
}
