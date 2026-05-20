use axum::response::{Html, IntoResponse, Redirect};

const INDEX_HTML: &str = include_str!("index.html");
const FRONTEND_DEV_ENV: &str = "OZMUX_FRONTEND_DEV";
const VITE_DEV_URL: &str = "http://127.0.0.1:5173";

pub async fn handler() -> impl IntoResponse {
    if std::env::var_os(FRONTEND_DEV_ENV).is_some() {
        Redirect::temporary(VITE_DEV_URL).into_response()
    } else {
        Html(INDEX_HTML).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::FRONTEND_DEV_ENV;
    use crate::daemon_router;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    // NOTE: these two tests mutate process env (`OZMUX_FRONTEND_DEV`), so they
    // must run serially. CI invokes `cargo test ... --test-threads=1`; locally,
    // `cargo test` may run them in parallel and produce a spurious failure.
    // Use `--test-threads=1` if reproducing locally.

    #[tokio::test]
    async fn returns_redirect_when_env_var_set() {
        // SAFETY: single-threaded test, env var is set and removed within this
        // function. CI pins `--test-threads=1`.
        unsafe { std::env::set_var(FRONTEND_DEV_ENV, "1") };
        let response = daemon_router(crate::test_helpers::fresh_state())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        // SAFETY: see above.
        unsafe { std::env::remove_var(FRONTEND_DEV_ENV) };

        assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
        let location = response
            .headers()
            .get("location")
            .expect("location header")
            .to_str()
            .unwrap();
        assert_eq!(location, "http://127.0.0.1:5173");
    }

    #[tokio::test]
    async fn returns_html_when_env_var_unset() {
        use http_body_util::BodyExt;

        // SAFETY: ensure the env var is clear regardless of test ordering.
        // CI pins `--test-threads=1`.
        unsafe { std::env::remove_var(FRONTEND_DEV_ENV) };

        let response = daemon_router(crate::test_helpers::fresh_state())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let content_type = response
            .headers()
            .get("content-type")
            .expect("content-type header")
            .to_str()
            .unwrap();
        assert_eq!(content_type, "text/html; charset=utf-8");

        let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body_str = std::str::from_utf8(&body_bytes).unwrap();
        assert!(
            body_str.to_lowercase().contains("<!doctype html"),
            "body should contain doctype, got: {}",
            body_str
        );
    }
}
