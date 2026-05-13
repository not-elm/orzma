use crate::AppState;
use crate::error::HttpError;
use crate::handlers::publish_window_layout;
use axum::{
    extract::{Path, State},
    http::StatusCode,
};
use ozmux_multiplexer::{ActivityId, PaneId, SetActiveOutcome, WindowId};

pub async fn activate(
    State(state): State<AppState>,
    Path((wid, pid, aid)): Path<(WindowId, PaneId, ActivityId)>,
) -> Result<StatusCode, HttpError> {
    let outcome = state
        .multiplexer
        .with_window_or_404(&wid, |w| w.pane_mut(&pid)?.set_active_activity(&aid))
        .await?;
    if matches!(outcome, SetActiveOutcome::Changed) {
        publish_window_layout(&state, &wid).await;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use crate::test_helpers;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use ozmux_multiplexer::{Activity, ActivityId};
    use tower::ServiceExt;

    #[tokio::test]
    async fn activate_switches_active_activity_and_publishes() {
        let state = test_helpers::fresh_state();
        let (_sid, wid, pid, _aid_initial) = test_helpers::bootstrap_default(&state).await;
        let new_aid = ActivityId::new();
        state
            .multiplexer
            .with_window_or_404(&wid, |w| {
                w.pane_mut(&pid)?
                    .add_activity(Activity::terminal(new_aid.clone()))
            })
            .await
            .unwrap();
        let mut rx = state.layout_broadcast.subscribe_or_create(&wid);
        let (router, _state) = test_helpers::router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/windows/{wid}/panes/{pid}/activities/{new_aid}/activate"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let _ = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv())
            .await
            .expect("publish timed out")
            .expect("recv error");
    }

    #[tokio::test]
    async fn activate_already_active_does_not_publish() {
        let state = test_helpers::fresh_state();
        let (_sid, wid, pid, aid) = test_helpers::bootstrap_default(&state).await;
        let mut rx = state.layout_broadcast.subscribe_or_create(&wid);
        let (router, _state) = test_helpers::router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/windows/{wid}/panes/{pid}/activities/{aid}/activate"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let res = tokio::time::timeout(std::time::Duration::from_millis(80), rx.recv()).await;
        assert!(res.is_err(), "Unchanged outcome must not publish");
    }

    #[tokio::test]
    async fn activate_unknown_activity_returns_404() {
        let state = test_helpers::fresh_state();
        let (_sid, wid, pid, _aid) = test_helpers::bootstrap_default(&state).await;
        let (router, _state) = test_helpers::router_with(state);
        let phantom = ActivityId::new();
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/windows/{wid}/panes/{pid}/activities/{phantom}/activate"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
