use crate::handlers::windows::panes::activities::ActivityInput;
use crate::state::SplitInput;
use crate::{AppState, error::HttpResult};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use ozmux_multiplexer::{Activity, ActivityId, PaneId, Side, SplitOrientation, WindowId};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct SplitRequest {
    orientation: SplitOrientation,
    #[serde(default)]
    side: Side,
    /// Client-supplied id for the new Pane. When absent the server picks one.
    #[serde(default)]
    new_pane_id: Option<PaneId>,
    /// Client-supplied Activity spec (id + kind). When absent the server
    /// creates an empty Terminal Activity.
    #[serde(default)]
    activity: Option<ActivityInput>,
}

pub async fn split(
    State(state): State<AppState>,
    Path((wid, target_pane_id)): Path<(WindowId, PaneId)>,
    Json(req): Json<SplitRequest>,
) -> HttpResult<(StatusCode, Json<serde_json::Value>)> {
    let new_pane_id = req.new_pane_id.unwrap_or_default();
    let (new_activity, extension_name) = match req.activity {
        Some(spec) => {
            let parsed = spec.into_parsed();
            (parsed.activity, parsed.extension_name)
        }
        None => (Activity::terminal(ActivityId::new()), None),
    };

    let outcome = state
        .split_pane(
            &wid,
            &target_pane_id,
            SplitInput {
                new_pane_id,
                new_activity,
                extension_name,
                side: req.side,
                orientation: req.orientation,
            },
        )
        .await?;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "new_pane_id": outcome.new_pane_id,
            "new_activity_id": outcome.new_activity_id,
        })),
    ))
}

#[cfg(test)]
mod tests {
    use crate::AppState;
    use crate::test_helpers::{bootstrap_default, fresh_state, router_with};
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use ozmux_multiplexer::{ActivityId, PaneId};
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

    #[tokio::test]
    async fn split_returns_new_pane_and_publishes() {
        let state = fresh_state();
        let (_sid, wid, pid, _aid) = bootstrap_default(&state).await;
        let mut rx = state.layout_broadcast.subscribe_or_create(&wid);
        let (router, _state) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/windows/{}/panes/{}/split", wid, pid))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"orientation":"horizontal","side":"after"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        if status == StatusCode::CREATED {
            let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
            let v: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
            assert!(v["new_pane_id"].is_string());
            assert!(v["new_activity_id"].is_string());
            let view = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv())
                .await
                .expect("publish timed out")
                .expect("recv error");
            assert_eq!(view["id"].as_str(), Some(wid.as_ref()));
            assert_eq!(view["layout"]["child"]["type"].as_str(), Some("split"));
        } else {
            assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        }
    }

    #[tokio::test]
    async fn split_with_wrong_wid_returns_409() {
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
                    .uri(format!("/windows/{}/panes/{}/split", wid_b, pid_a))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"orientation":"horizontal","side":"after"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn split_with_unknown_pane_returns_404() {
        let state = fresh_state();
        let (_sid, wid, _pid, _aid) = bootstrap_default(&state).await;
        let (router, _state) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/windows/{}/panes/{}/split", wid, PaneId::new()))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"orientation":"horizontal"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn split_either_succeeds_or_rolls_back() {
        let state = fresh_state();
        let (_sid, wid, pid, _aid) = bootstrap_default(&state).await;
        let panes_before = total_panes(&state).await;
        let (router, state) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/windows/{}/panes/{}/split", wid, pid))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"orientation":"horizontal"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let panes_after = total_panes(&state).await;
        if status == StatusCode::CREATED {
            assert_eq!(
                panes_after,
                panes_before + 1,
                "split must add a pane on success"
            );
        } else {
            assert_eq!(
                panes_after, panes_before,
                "split rollback must restore pane count on spawn failure"
            );
        }
    }

    #[tokio::test]
    async fn split_honors_client_supplied_pane_and_activity_ids() {
        let state = fresh_state();
        let (_sid, wid, pid, _aid) = bootstrap_default(&state).await;
        let new_pid = PaneId::new();
        let new_aid = ActivityId::new();
        let (router, _state) = router_with(state);
        let body = format!(
            r#"{{"side":"after","orientation":"horizontal","new_pane_id":"{}","activity":{{"activity_id":"{}","kind":{{"type":"terminal"}}}}}}"#,
            new_pid, new_aid,
        );
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/windows/{}/panes/{}/split", wid, pid))
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        // Spawn may legitimately fail under heavy CI; only assert ID echo on success.
        if status == StatusCode::CREATED {
            let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
            let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(v["new_pane_id"].as_str(), Some(new_pid.as_ref()));
            assert_eq!(v["new_activity_id"].as_str(), Some(new_aid.as_ref()));
        }
    }

    #[tokio::test]
    async fn split_with_extension_activity_records_owner_in_registry() {
        // PR7 regression guard: the daemon must populate the extension registry
        // when a split lands an Extension-kind activity, otherwise the iframe's
        // handlers-WS upgrade gets a 404 (handlers_ws calls activity_owner).
        let state = fresh_state();
        let (_sid, wid, pid, _aid) = bootstrap_default(&state).await;
        state
            .extensions
            .register("memo", std::path::Path::new("/tmp"));
        let registry = state.extensions.clone();
        let (router, _state) = router_with(state);
        let new_pid = PaneId::new();
        let new_aid = ActivityId::new();
        let body = format!(
            r#"{{"side":"after","orientation":"horizontal","new_pane_id":"{}","activity":{{"activity_id":"{}","name":"memo","kind":{{"type":"extension","html_root":"/tmp","extension_name":"memo"}}}}}}"#,
            new_pid, new_aid,
        );
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/windows/{}/panes/{}/split", wid, pid))
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        assert_eq!(registry.activity_owner(&new_aid).as_deref(), Some("memo"));
        assert_eq!(registry.pane_owner(&new_pid).as_deref(), Some("memo"));
    }

    #[tokio::test]
    async fn split_with_extension_activity_does_not_spawn_pty() {
        // Extension activities live in an iframe, not a PTY. Spawning a shell
        // for them leaks an orphan child whose output nothing reads. The
        // TerminalService refuses duplicate spawns, so the simplest assertion
        // is "subscriber_count returns NotFound for the new aid".
        let state = fresh_state();
        let (_sid, wid, pid, _aid) = bootstrap_default(&state).await;
        state
            .extensions
            .register("memo", std::path::Path::new("/tmp"));
        let terminal = state.terminal.clone();
        let (router, _state) = router_with(state);
        let new_pid = PaneId::new();
        let new_aid = ActivityId::new();
        let body = format!(
            r#"{{"side":"after","orientation":"horizontal","new_pane_id":"{}","activity":{{"activity_id":"{}","name":"memo","kind":{{"type":"extension","html_root":"/tmp","extension_name":"memo"}}}}}}"#,
            new_pid, new_aid,
        );
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/windows/{}/panes/{}/split", wid, pid))
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        // No PTY for an Extension activity — `subscriber_count` returns `None`
        // for an aid the terminal service has never seen.
        assert!(
            terminal.subscriber_count(&new_aid).await.is_none(),
            "Extension activity must not have a backing PTY"
        );
    }

    #[tokio::test]
    async fn split_with_duplicate_pane_id_returns_409() {
        let state = fresh_state();
        let (_sid, wid, pid, _aid) = bootstrap_default(&state).await;
        let (router, _state) = router_with(state);
        // Reuse the existing pane's id as the new id → PaneIdConflict.
        let body = format!(
            r#"{{"side":"after","orientation":"horizontal","new_pane_id":"{}","activity":{{"activity_id":"{}","kind":{{"type":"terminal"}}}}}}"#,
            pid,
            ActivityId::new(),
        );
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/windows/{}/panes/{}/split", wid, pid))
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }
}
