use crate::{AppState, error::HttpResult};
use axum::{
    extract::{Path, State},
    http::StatusCode,
};
use ozmux_multiplexer::{PaneId, WindowId};

pub async fn close(
    State(state): State<AppState>,
    Path((wid, pane_id)): Path<(WindowId, PaneId)>,
) -> HttpResult<StatusCode> {
    state.close_pane(&wid, &pane_id).await?;
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

    async fn total_panes(state: &AppState) -> usize {
        let mut total = 0;
        for entry in state.multiplexer.windows.iter() {
            let arc = entry.value().clone();
            drop(entry);
            let win = arc.lock().await;
            total += win.panes.len();
        }
        total
    }

    async fn pane_to_cell_contains(state: &AppState, pid: &PaneId) -> bool {
        for entry in state.multiplexer.windows.iter() {
            let arc = entry.value().clone();
            drop(entry);
            let win = arc.lock().await;
            if win.pane_to_cell.contains_key(pid) {
                return true;
            }
        }
        false
    }

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
    async fn close_returns_204_and_removes_pane() {
        let state = fresh_state();
        let (_sid, wid, original_pid, _aid) = bootstrap_default(&state).await;
        let new_pid = split_via_window(&state, &wid, &original_pid).await;
        let panes_before = total_panes(&state).await;
        let (router, state) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/windows/{}/panes/{}", wid, new_pid))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert_eq!(total_panes(&state).await, panes_before - 1);
        assert!(!pane_to_cell_contains(&state, &new_pid).await);
    }

    #[tokio::test]
    async fn close_with_wrong_wid_returns_409() {
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
                    .method("DELETE")
                    .uri(format!("/windows/{}/panes/{}", wid_b, pid_a))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn close_unknown_pane_returns_404() {
        let state = fresh_state();
        let (_sid, wid, _pid, _aid) = bootstrap_default(&state).await;
        let (router, _state) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/windows/{}/panes/{}", wid, PaneId::new()))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn close_last_pane_returns_409() {
        let state = fresh_state();
        let (_sid, wid, pid, _aid) = bootstrap_default(&state).await;
        let (router, _state) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/windows/{}/panes/{}", wid, pid))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn close_owned_pane_forgets_owner() {
        let state = fresh_state();
        let (_sid, wid, original_pid, _aid) = bootstrap_default(&state).await;
        let new_pid = split_via_window(&state, &wid, &original_pid).await;
        state
            .extensions
            .register("memo", std::path::Path::new("/tmp"));
        state.extensions.record_pane_owner(&new_pid, "memo");
        let (router, state) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/windows/{}/panes/{}", wid, new_pid))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert!(
            state.extensions.pane_owner(&new_pid).is_none(),
            "pane owner registry entry must be cleared after close"
        );
    }

    #[tokio::test]
    async fn close_publishes_layout_to_subscriber() {
        let state = fresh_state();
        let (_sid, wid, pid_a, _aid) = bootstrap_default(&state).await;
        let pid_b = split_via_window(&state, &wid, &pid_a).await;
        let mut rx = state.layout_broadcast.subscribe_or_create(&wid);
        let (router, _state) = router_with(state);

        let resp = router
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/windows/{}/panes/{}", wid, pid_b))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        let view = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv())
            .await
            .expect("publish timed out")
            .expect("recv error");
        assert_eq!(view["id"].as_str(), Some(wid.as_ref()));
        assert_eq!(view["layout"]["child"]["type"].as_str(), Some("pane"));
        assert_eq!(
            view["layout"]["child"]["pane_id"].as_str(),
            Some(pid_a.as_ref())
        );
    }
}
