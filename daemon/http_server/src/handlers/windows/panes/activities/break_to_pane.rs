//! `POST /windows/{wid}/panes/{pid}/activities/{aid}/break-to-pane` — split
//! the pane and move the activity into the new pane.

use crate::state::BreakActivityInput;
use crate::{AppState, error::HttpResult};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use ozmux_multiplexer::{ActivityId, PaneId, Side, SplitOrientation, WindowId};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct BreakToPaneRequest {
    orientation: SplitOrientation,
    #[serde(default)]
    side: Side,
    /// Client-supplied id for the new Pane. When absent the server picks one.
    #[serde(default)]
    new_pane_id: Option<PaneId>,
}

/// Splits the target pane and moves the activity into the new pane.
pub async fn break_to_pane(
    State(state): State<AppState>,
    Path((wid, pid, aid)): Path<(WindowId, PaneId, ActivityId)>,
    Json(req): Json<BreakToPaneRequest>,
) -> HttpResult<(StatusCode, Json<serde_json::Value>)> {
    let new_pane_id = req.new_pane_id.unwrap_or_default();
    let created = state
        .break_activity_to_pane(
            &wid,
            &pid,
            &aid,
            BreakActivityInput {
                new_pane_id,
                side: req.side,
                orientation: req.orientation,
            },
        )
        .await?;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({ "new_pane_id": created })),
    ))
}

#[cfg(test)]
mod tests {
    use crate::test_helpers::{bootstrap_default, fresh_state, router_with};
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use ozmux_multiplexer::{Activity, ActivityId, PaneId, WindowId};
    use tower::ServiceExt;

    async fn post_break(
        router: axum::Router,
        wid: &WindowId,
        pid: &PaneId,
        aid: &ActivityId,
        body: &str,
    ) -> axum::response::Response {
        router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/windows/{wid}/panes/{pid}/activities/{aid}/break-to-pane"
                    ))
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn add_second_activity(
        state: &crate::AppState,
        wid: &WindowId,
        pid: &PaneId,
    ) -> ActivityId {
        let activity = Activity::terminal(ActivityId::new());
        let aid = activity.id.clone();
        state
            .multiplexer
            .with_window_or_404(wid, |w| w.pane_mut(pid)?.add_activity(activity))
            .await
            .unwrap();
        aid
    }

    #[tokio::test]
    async fn break_to_pane_returns_201_and_new_pane_id() {
        let state = fresh_state();
        let (_sid, wid, pid, aid) = bootstrap_default(&state).await;
        let _second = add_second_activity(&state, &wid, &pid).await;
        let mut rx = state.layout_broadcast.subscribe_or_create(&wid);
        let (router, _state) = router_with(state);

        let resp = post_break(
            router,
            &wid,
            &pid,
            &aid,
            r#"{"orientation":"horizontal","side":"after"}"#,
        )
        .await;

        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(v["new_pane_id"].is_string());

        let view = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv())
            .await
            .expect("publish timed out")
            .expect("recv error");
        assert_eq!(view["id"].as_str(), Some(wid.as_ref()));
    }

    #[tokio::test]
    async fn break_to_pane_single_activity_returns_409() {
        let state = fresh_state();
        let (_sid, wid, pid, aid) = bootstrap_default(&state).await;
        let (router, _state) = router_with(state);
        let resp = post_break(router, &wid, &pid, &aid, r#"{"orientation":"horizontal"}"#).await;
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn break_to_pane_unknown_activity_returns_404() {
        let state = fresh_state();
        let (_sid, wid, pid, _aid) = bootstrap_default(&state).await;
        let _second = add_second_activity(&state, &wid, &pid).await;
        let (router, _state) = router_with(state);
        let resp = post_break(
            router,
            &wid,
            &pid,
            &ActivityId::new(),
            r#"{"orientation":"horizontal"}"#,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn break_to_pane_duplicate_new_pane_id_returns_409() {
        let state = fresh_state();
        let (_sid, wid, pid, aid) = bootstrap_default(&state).await;
        let _second = add_second_activity(&state, &wid, &pid).await;
        let (router, _state) = router_with(state);
        let body = format!(r#"{{"orientation":"horizontal","new_pane_id":"{pid}"}}"#);
        let resp = post_break(router, &wid, &pid, &aid, &body).await;
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn break_to_pane_moved_activity_pty_is_not_respawned() {
        let state = fresh_state();
        let (_sid, wid, pid, aid) = bootstrap_default(&state).await;
        let _second = add_second_activity(&state, &wid, &pid).await;
        state
            .terminal
            .spawn(
                pid.clone(),
                aid.clone(),
                ozmux_terminal::SpawnOptions {
                    cols: 80,
                    rows: 24,
                    shell: "/bin/sh".to_string(),
                    cwd: None,
                    window_id: None,
                    session_id: None,
                },
            )
            .await
            .unwrap();
        let terminal = state.terminal.clone();
        let (router, _state) = router_with(state);
        let resp = post_break(router, &wid, &pid, &aid, r#"{"orientation":"horizontal"}"#).await;
        assert_eq!(resp.status(), StatusCode::CREATED);
        assert!(
            terminal.subscriber_count(&aid).await.is_some(),
            "moved activity keeps its original PTY"
        );
    }

    #[tokio::test]
    async fn break_to_pane_sibling_pty_stays_alive() {
        // Spec guarantee: moving an activity must not disturb sibling PTYs in
        // the source pane.
        let state = fresh_state();
        let (_sid, wid, pid, aid) = bootstrap_default(&state).await;
        let sibling_aid = add_second_activity(&state, &wid, &pid).await;
        for spawn_aid in [&aid, &sibling_aid] {
            state
                .terminal
                .spawn(
                    pid.clone(),
                    spawn_aid.clone(),
                    ozmux_terminal::SpawnOptions {
                        cols: 80,
                        rows: 24,
                        shell: "/bin/sh".to_string(),
                        cwd: None,
                        window_id: None,
                        session_id: None,
                    },
                )
                .await
                .unwrap();
        }
        let terminal = state.terminal.clone();
        let (router, _state) = router_with(state);
        let resp = post_break(router, &wid, &pid, &aid, r#"{"orientation":"horizontal"}"#).await;
        assert_eq!(resp.status(), StatusCode::CREATED);
        assert!(
            terminal.subscriber_count(&sibling_aid).await.is_some(),
            "the sibling activity's PTY must survive the move"
        );
    }

    #[tokio::test]
    async fn break_to_pane_new_pane_is_activatable() {
        let state = fresh_state();
        let (_sid, wid, pid, aid) = bootstrap_default(&state).await;
        let _second = add_second_activity(&state, &wid, &pid).await;
        let (router, _state) = router_with(state);
        let resp = post_break(
            router.clone(),
            &wid,
            &pid,
            &aid,
            r#"{"orientation":"horizontal"}"#,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let new_pid = v["new_pane_id"].as_str().unwrap();

        let activate = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/windows/{wid}/panes/{new_pid}/activate"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(activate.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn break_to_pane_extension_activity_records_new_pane_owner() {
        let state = fresh_state();
        let (_sid, wid, pid, _aid) = bootstrap_default(&state).await;
        state
            .extensions
            .register("memo", std::path::Path::new("/tmp"));
        let ext_activity =
            Activity::extension(ActivityId::new(), "ext", std::path::PathBuf::from("/tmp"));
        let ext_aid = ext_activity.id.clone();
        state
            .multiplexer
            .with_window_or_404(&wid, |w| w.pane_mut(&pid)?.add_activity(ext_activity))
            .await
            .unwrap();
        state.extensions.record_activity_owner(&ext_aid, "memo");
        let registry = state.extensions.clone();
        let (router, _state) = router_with(state);

        let resp = post_break(
            router,
            &wid,
            &pid,
            &ext_aid,
            r#"{"orientation":"horizontal"}"#,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let new_pid: PaneId = serde_json::from_value(v["new_pane_id"].clone()).unwrap();
        assert_eq!(registry.pane_owner(&new_pid).as_deref(), Some("memo"));
        assert_eq!(
            registry.activity_owner(&ext_aid).as_deref(),
            Some("memo"),
            "moving an extension activity must not drop its activity->owner row"
        );
    }

    #[tokio::test]
    async fn break_to_pane_with_wrong_wid_returns_409() {
        let state = fresh_state();
        let (sid, wid_a, pid_a, aid_a) = bootstrap_default(&state).await;
        let _second = add_second_activity(&state, &wid_a, &pid_a).await;
        let (wid_b, _, _) = state
            .multiplexer
            .create_window(Some(&sid), None)
            .await
            .unwrap();
        let (router, _state) = router_with(state);
        let resp = post_break(
            router,
            &wid_b,
            &pid_a,
            &aid_a,
            r#"{"orientation":"horizontal"}"#,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }
}
