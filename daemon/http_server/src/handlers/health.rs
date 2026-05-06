use axum::{body::Body, response::Response};

pub async fn check() -> Response {
    Response::new(Body::empty())
}

#[cfg(test)]
mod tests {
    use crate::daemon_router;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    #[tokio::test]
    async fn check_health() {
        let response = daemon_router(crate::AppState::default())
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }
}
