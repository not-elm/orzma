use crate::{AppState, error::HttpResult};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use ozmux_multiplexer::{CycleDirection, PaneId, WindowId};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct CycleRequest {
    direction: CycleDirection,
}

pub async fn cycle_activity(
    State(state): State<AppState>,
    Path((wid, pid)): Path<(WindowId, PaneId)>,
    Json(req): Json<CycleRequest>,
) -> HttpResult<StatusCode> {
    state
        .cycle_active_activity(&wid, &pid, req.direction)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use crate::test_helpers::{bootstrap_default, fresh_state, router_with};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use ozmux_multiplexer::{Activity, ActivityId, PaneId};
    use tower::ServiceExt;

    async fn add_activity(
        state: &crate::AppState,
        wid: &ozmux_multiplexer::WindowId,
        pid: &PaneId,
    ) -> ActivityId {
        let aid = ActivityId::new();
        state
            .multiplexer
            .with_window_or_404(wid, |w| {
                w.pane_mut(pid)?
                    .add_activity(Activity::terminal(aid.clone()))
            })
            .await
            .unwrap();
        aid
    }

    #[tokio::test]
    async fn cycle_next_changes_active_and_publishes() {
        let state = fresh_state();
        let (_sid, wid, pid, _aid0) = bootstrap_default(&state).await;
        let aid1 = add_activity(&state, &wid, &pid).await;
        let mut rx = state.layout_broadcast.subscribe_or_create(&wid);
        let (router, _state) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/windows/{}/panes/{}/cycle-activity", wid, pid))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"direction":"next"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let view = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv())
            .await
            .expect("publish timed out")
            .expect("recv error");
        assert_eq!(view["id"].as_str(), Some(wid.as_ref()));
        assert_eq!(
            view["panes"][0]["active_activity"].as_str(),
            Some(aid1.as_ref()),
            "cycle_next must advance active_activity to the second activity"
        );
    }

    #[tokio::test]
    async fn cycle_single_activity_no_publish() {
        let state = fresh_state();
        let (_sid, wid, pid, _aid) = bootstrap_default(&state).await;
        let mut rx = state.layout_broadcast.subscribe_or_create(&wid);
        let (router, _state) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/windows/{}/panes/{}/cycle-activity", wid, pid))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"direction":"next"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let res = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await;
        assert!(res.is_err(), "Unchanged outcome must not publish");
    }

    #[tokio::test]
    async fn cycle_unknown_pane_returns_404() {
        let state = fresh_state();
        let (_sid, wid, _pid, _aid) = bootstrap_default(&state).await;
        let (router, _state) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/windows/{}/panes/{}/cycle-activity",
                        wid,
                        PaneId::new()
                    ))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"direction":"next"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn cycle_invalid_direction_returns_422() {
        let state = fresh_state();
        let (_sid, wid, pid, _aid) = bootstrap_default(&state).await;
        let (router, _state) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/windows/{}/panes/{}/cycle-activity", wid, pid))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"direction":"sideways"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn cycle_malformed_json_returns_400() {
        let state = fresh_state();
        let (_sid, wid, pid, _aid) = bootstrap_default(&state).await;
        let (router, _state) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/windows/{}/panes/{}/cycle-activity", wid, pid))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"direction":"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}
