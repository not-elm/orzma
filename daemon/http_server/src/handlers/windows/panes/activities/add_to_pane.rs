use crate::AppState;
use crate::error::HttpError;
use crate::handlers::publish_window_layout;
use axum::{
    extract::{Path, State},
    http::StatusCode,
};
use ozmux_multiplexer::{PaneId, WindowId};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct AddActivityRequest {
    activity: super::ActivityInput,
}

pub async fn add_to_pane(
    State(state): State<AppState>,
    Path((wid, pid)): Path<(WindowId, PaneId)>,
    axum::Json(body): axum::Json<AddActivityRequest>,
) -> Result<(StatusCode, axum::Json<serde_json::Value>), HttpError> {
    let parsed = body.activity.into_parsed();
    let aid = parsed.activity.id.clone();
    state
        .with_window_or_404(&wid, |w| w.pane_mut(&pid)?.add_activity(parsed.activity))
        .await?;
    if let Some(name) = parsed.extension_name.as_deref() {
        // `add_to_pane` only mints a new Activity — the Pane already exists —
        // so we only need the activity-owner row. Pane-owner stays untouched.
        state.extensions.record_activity_owner(&aid, name);
    }
    publish_window_layout(&state, &wid).await;
    Ok((
        StatusCode::CREATED,
        axum::Json(serde_json::json!({ "activity_id": aid })),
    ))
}

#[cfg(test)]
mod tests {
    use crate::test_helpers;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use ozmux_multiplexer::ActivityId;
    use tower::ServiceExt;

    #[tokio::test]
    async fn add_to_pane_creates_tab_and_publishes() {
        let state = test_helpers::fresh_state();
        let (_sid, wid, pid, _aid) = test_helpers::bootstrap_default(&state).await;
        let mut rx = state.layout_broadcast.subscribe_or_create(&wid);
        let (router, _state) = test_helpers::router_with(state);
        let new_aid = ActivityId::new();
        let body = serde_json::json!({
            "activity": {
                "activity_id": new_aid,
                "kind": { "type": "terminal" }
            }
        });
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/windows/{wid}/panes/{pid}/activities"))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["activity_id"].as_str(), Some(new_aid.as_ref()));
        let _ = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv())
            .await
            .expect("publish timed out")
            .expect("recv error");
    }

    #[tokio::test]
    async fn add_to_pane_with_extension_kind_accepts_html_root() {
        let state = test_helpers::fresh_state();
        let (_sid, wid, pid, _aid) = test_helpers::bootstrap_default(&state).await;
        let (router, _state) = test_helpers::router_with(state);
        let new_aid = ActivityId::new();
        let body = serde_json::json!({
            "activity": {
                "activity_id": new_aid,
                "name": "memo",
                "kind": {
                    "type": "extension",
                    "html_root": "/tmp",
                    "extension_name": "memo"
                }
            }
        });
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/windows/{wid}/panes/{pid}/activities"))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn add_to_pane_extension_kind_records_activity_owner_in_registry() {
        let state = test_helpers::fresh_state();
        let (_sid, wid, pid, _aid) = test_helpers::bootstrap_default(&state).await;
        // The handler reads ownership info off `state.extensions`, so register
        // the extension up-front. The wire `extension_name` is what drives
        // `record_activity_owner` — the registration we're verifying.
        state
            .extensions
            .register("memo", std::path::Path::new("/tmp"));
        let registry = state.extensions.clone();
        let (router, _state) = test_helpers::router_with(state);
        let new_aid = ActivityId::new();
        let body = serde_json::json!({
            "activity": {
                "activity_id": new_aid,
                "name": "memo",
                "kind": {
                    "type": "extension",
                    "html_root": "/tmp",
                    "extension_name": "memo"
                }
            }
        });
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/windows/{wid}/panes/{pid}/activities"))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        assert_eq!(registry.activity_owner(&new_aid).as_deref(), Some("memo"));
    }

    #[tokio::test]
    async fn add_to_pane_unknown_window_returns_404() {
        let state = test_helpers::fresh_state();
        let (_sid, _wid, pid, _aid) = test_helpers::bootstrap_default(&state).await;
        let (router, _state) = test_helpers::router_with(state);
        let bogus_wid = ozmux_multiplexer::WindowId::new();
        let body = serde_json::json!({
            "activity": {
                "activity_id": ActivityId::new(),
                "kind": { "type": "terminal" }
            }
        });
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/windows/{bogus_wid}/panes/{pid}/activities"))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
