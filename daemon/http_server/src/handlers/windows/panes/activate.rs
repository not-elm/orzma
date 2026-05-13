use crate::{AppState, error::HttpResult};
use axum::{
    extract::{Path, State},
    http::StatusCode,
};
use ozmux_multiplexer::{PaneId, WindowId};

pub async fn activate(
    State(state): State<AppState>,
    Path((window_id, pane_id)): Path<(WindowId, PaneId)>,
) -> HttpResult<StatusCode> {
    state.activate_pane(&window_id, &pane_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use crate::AppState;
    use crate::test_helpers::{bootstrap_default, fresh_state, router_with};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use ozmux_multiplexer::{Activity, ActivityId, PaneId, Side, SplitOrientation, WindowId};
    use tower::ServiceExt;

    async fn split_via_window(state: &AppState, wid: &WindowId, target: &PaneId) -> PaneId {
        let new_pane_id = PaneId::new();
        let new_activity_id = ActivityId::new();
        state
            .multiplexer
            .with_window_or_404(wid, |w| {
                w.split_pane(
                    target,
                    new_pane_id.clone(),
                    Activity::terminal(new_activity_id.clone()),
                    Side::After,
                    SplitOrientation::Horizontal,
                )
            })
            .await
            .unwrap();
        state
            .multiplexer
            .pane_owner_window
            .insert(new_pane_id.clone(), wid.clone());
        new_pane_id
    }

    #[tokio::test]
    async fn activate_returns_204_when_pane_becomes_active() {
        let state = fresh_state();
        let (_sid, wid, original, _aid) = bootstrap_default(&state).await;
        let _new_pane = split_via_window(&state, &wid, &original).await;
        let (router, _state) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/windows/{}/panes/{}/activate", wid, original))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn activate_already_active_pane_returns_204() {
        let state = fresh_state();
        let (_sid, wid, pid, _aid) = bootstrap_default(&state).await;
        let (router, _state) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/windows/{}/panes/{}/activate", wid, pid))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn activate_unknown_window_returns_404() {
        let state = fresh_state();
        let (_sid, _wid, pid, _aid) = bootstrap_default(&state).await;
        let (router, _state) = router_with(state);
        let bogus_wid = WindowId::new();
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/windows/{}/panes/{}/activate", bogus_wid, pid))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn activate_pane_in_other_window_returns_409() {
        let state = fresh_state();
        let (sid, _wid_a, pid_a, _aid) = bootstrap_default(&state).await;
        let (wid_b, _, _) = state
            .multiplexer
            .create_window(Some(&sid), None)
            .await
            .unwrap();
        let (router, _state) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/windows/{}/panes/{}/activate", wid_b, pid_a))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }
}
