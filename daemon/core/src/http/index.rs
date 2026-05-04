#[cfg(not(debug_assertions))]
use axum::response::Html;
use axum::response::IntoResponse;
#[cfg(debug_assertions)]
use axum::response::Redirect;

#[cfg(not(debug_assertions))]
const INDEX_HTML: &str = include_str!("index.html");

pub async fn handler() -> impl IntoResponse {
    #[cfg(debug_assertions)]
    {
        Redirect::temporary("http://127.0.0.1:5173").into_response()
    }
    #[cfg(not(debug_assertions))]
    {
        Html(INDEX_HTML).into_response()
    }
}

#[cfg(test)]
mod tests {
    use crate::http::daemon_router;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    #[cfg(debug_assertions)]
    #[tokio::test]
    async fn debug_build_redirects_to_vite_dev() {
        let response = daemon_router(crate::http::AppState::default())
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
        let location = response
            .headers()
            .get("location")
            .expect("location header")
            .to_str()
            .unwrap();
        assert_eq!(location, "http://127.0.0.1:5173");
    }

    #[cfg(not(debug_assertions))]
    #[tokio::test]
    async fn release_build_returns_html() {
        use http_body_util::BodyExt;

        let response = daemon_router(crate::http::AppState::default())
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
