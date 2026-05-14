//! Close a single Activity in a Pane. Refuses to close the last activity
//! (caller should use close-pane if they want to drop the whole pane).

use crate::handlers::ensure_activity_in_pane_in_window;
use crate::{AppState, error::HttpResult};
use axum::{
    extract::{Path, State},
    http::StatusCode,
};
use ozmux_multiplexer::{ActivityId, PaneId, WindowId};

pub async fn close_activity(
    State(state): State<AppState>,
    Path((wid, pid, aid)): Path<(WindowId, PaneId, ActivityId)>,
) -> HttpResult<StatusCode> {
    ensure_activity_in_pane_in_window(&state, &wid, &pid, &aid).await?;
    state.close_activity(&wid, &pid, &aid).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use crate::test_helpers::{bootstrap_default, fresh_state, router_with};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use ozmux_multiplexer::{Activity, ActivityId, PaneId, WindowId};
    use tower::ServiceExt;

    async fn add_activity_via_window(
        state: &crate::AppState,
        wid: &WindowId,
        pid: &PaneId,
    ) -> ActivityId {
        let new_aid = ActivityId::new();
        state
            .multiplexer
            .with_window_or_404(wid, |w| {
                w.pane_mut(pid)?
                    .add_activity(Activity::terminal(new_aid.clone()))
            })
            .await
            .unwrap();
        new_aid
    }

    #[tokio::test]
    async fn close_activity_returns_204_when_pane_has_two() {
        let state = fresh_state();
        let (_sid, wid, pid, _aid) = bootstrap_default(&state).await;
        let extra_aid = add_activity_via_window(&state, &wid, &pid).await;
        let (router, state) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/windows/{wid}/panes/{pid}/activities/{extra_aid}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let remaining = state
            .multiplexer
            .with_window_or_404(&wid, |w| -> ozmux_multiplexer::MultiplexerResult<usize> {
                Ok(w.pane(&pid).map(|p| p.activities.len()).unwrap_or(0))
            })
            .await
            .unwrap();
        assert_eq!(remaining, 1);
    }

    #[tokio::test]
    async fn close_activity_returns_409_when_only_one_activity() {
        let state = fresh_state();
        let (_sid, wid, pid, aid) = bootstrap_default(&state).await;
        let (router, _state) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/windows/{wid}/panes/{pid}/activities/{aid}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn close_activity_returns_404_for_unknown_aid() {
        let state = fresh_state();
        let (_sid, wid, pid, _aid) = bootstrap_default(&state).await;
        let (router, _state) = router_with(state);
        let bogus = ActivityId::new();
        let resp = router
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/windows/{wid}/panes/{pid}/activities/{bogus}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn close_activity_publishes_layout_on_success() {
        let state = fresh_state();
        let (_sid, wid, pid, _aid) = bootstrap_default(&state).await;
        let extra_aid = add_activity_via_window(&state, &wid, &pid).await;
        let mut rx = state.layout_broadcast.subscribe_or_create(&wid);
        let (router, _state) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/windows/{wid}/panes/{pid}/activities/{extra_aid}"))
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
    async fn close_activity_does_not_publish_when_last_activity() {
        let state = fresh_state();
        let (_sid, wid, pid, aid) = bootstrap_default(&state).await;
        let mut rx = state.layout_broadcast.subscribe_or_create(&wid);
        let (router, _state) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/windows/{wid}/panes/{pid}/activities/{aid}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        let recv = tokio::time::timeout(std::time::Duration::from_millis(150), rx.recv()).await;
        assert!(
            recv.is_err(),
            "no broadcast must be sent when close-activity is refused"
        );
    }
}
