//! Custom axum extractors for the daemon HTTP API.

use crate::error::HttpError;
use axum::{
    extract::{FromRef, FromRequestParts},
    http::request::Parts,
};
use ozmux_extension::ExtensionRegistry;

pub struct ExtensionName(pub String);

impl<S> FromRequestParts<S> for ExtensionName
where
    ExtensionRegistry: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = HttpError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let header_value = parts
            .headers
            .get("X-Ozmux-Extension")
            .ok_or(HttpError::MissingExtensionHeader)?
            .to_str()
            .map_err(|_| HttpError::MissingExtensionHeader)?
            .trim();
        if header_value.is_empty() {
            return Err(HttpError::MissingExtensionHeader);
        }
        let registry = ExtensionRegistry::from_ref(state);
        if registry.get(header_value).is_none() {
            return Err(HttpError::UnknownExtension(header_value.to_string()));
        }
        Ok(ExtensionName(header_value.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::extract::State;
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use std::path::Path as StdPath;
    use tower::ServiceExt;

    #[derive(Clone)]
    struct TestState {
        registry: ExtensionRegistry,
    }

    impl FromRef<TestState> for ExtensionRegistry {
        fn from_ref(input: &TestState) -> Self {
            input.registry.clone()
        }
    }

    async fn echo_handler(
        ExtensionName(name): ExtensionName,
        State(_): State<TestState>,
    ) -> String {
        name
    }

    fn router_with(registry: ExtensionRegistry) -> Router {
        Router::new()
            .route("/echo", get(echo_handler))
            .with_state(TestState { registry })
    }

    #[tokio::test]
    async fn returns_name_when_header_set_and_registered() {
        let registry = ExtensionRegistry::default();
        registry.register("memo", StdPath::new("/tmp/memo"));
        let resp = router_with(registry)
            .oneshot(
                Request::builder()
                    .uri("/echo")
                    .header("X-Ozmux-Extension", "memo")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn returns_401_when_header_missing() {
        let registry = ExtensionRegistry::default();
        let resp = router_with(registry)
            .oneshot(Request::builder().uri("/echo").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn returns_401_when_header_empty() {
        let registry = ExtensionRegistry::default();
        let resp = router_with(registry)
            .oneshot(
                Request::builder()
                    .uri("/echo")
                    .header("X-Ozmux-Extension", "")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn returns_403_when_extension_not_registered() {
        let registry = ExtensionRegistry::default();
        let resp = router_with(registry)
            .oneshot(
                Request::builder()
                    .uri("/echo")
                    .header("X-Ozmux-Extension", "ghost")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }
}
