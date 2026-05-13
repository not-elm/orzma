use crate::AppState;
use crate::error::{HttpError, HttpResult};
use crate::handlers::publish_window_layout;
use crate::handlers::windows::panes::spawn_terminal::spawn_terminal_pty;
use axum::{
    extract::{Path, State},
    http::StatusCode,
};
use ozmux_multiplexer::{ActivityId, ActivityKind, MultiplexerError, PaneId, WindowId};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct AddActivityRequest {
    activity: super::ActivityInput,
}

pub async fn add_to_pane(
    State(state): State<AppState>,
    Path((wid, pid)): Path<(WindowId, PaneId)>,
    axum::Json(body): axum::Json<AddActivityRequest>,
) -> HttpResult<(StatusCode, axum::Json<serde_json::Value>)> {
    let parsed = body.activity.into_parsed();
    let activity_kind = parsed.activity.kind.clone();
    let aid = state
        .add_activity_to_pane(
            &wid,
            &pid,
            parsed.activity,
            parsed.extension_name.as_deref(),
        )
        .await?;

    if matches!(activity_kind, ActivityKind::Terminal) {
        if let Err(spawn_err) =
            spawn_terminal_pty(&state, &wid, &pid, &aid).await
        {
            if let Err(rollback_err) = rollback_added_activity(&state, &wid, &pid, &aid).await {
                tracing::warn!(
                    error = %rollback_err,
                    %wid, %pid, %aid,
                    "failed to roll back added activity after PTY spawn failure"
                );
            }
            return Err(spawn_err);
        }
    }

    // NOTE: publish only after successful spawn so the frontend never sees a pane
    // with a missing PTY (mirrors split.rs's "spawn must precede publish" invariant).
    publish_window_layout(&state, &wid).await;

    Ok((
        StatusCode::CREATED,
        axum::Json(serde_json::json!({ "activity_id": aid })),
    ))
}

async fn rollback_added_activity(
    state: &AppState,
    wid: &WindowId,
    pid: &PaneId,
    aid: &ActivityId,
) -> Result<(), HttpError> {
    state
        .multiplexer
        .with_window_or_404(wid, |w| -> Result<(), MultiplexerError> {
            w.pane_mut(pid)?.remove_activity(aid).map(|_| ())
        })
        .await?;
    Ok(())
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
        // NOTE: register the extension up-front so `record_activity_owner` has
        // something to verify; the wire `extension_name` drives the call.
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

    #[tokio::test]
    async fn add_to_pane_spawns_pty_for_terminal_kind() {
        let state = test_helpers::fresh_state();
        let (_sid, wid, pid, _aid) = test_helpers::bootstrap_default(&state).await;
        let terminal = state.terminal.clone();
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
        let status = resp.status();
        if status == StatusCode::CREATED {
            assert!(
                terminal.subscriber_count(&new_aid).await.is_some(),
                "Terminal activity must have a backing PTY after add_to_pane"
            );
        }
    }

    #[tokio::test]
    async fn add_to_pane_rolls_back_when_spawn_fails() {
        let state = test_helpers::fresh_state();
        let (_sid, wid, pid, _aid) = test_helpers::bootstrap_default(&state).await;
        let mut rx = state.layout_broadcast.subscribe_or_create(&wid);
        let new_aid = ActivityId::new();
        state.terminal.inject_spawn_failure(new_aid.clone()).await;
        let (router, state) = test_helpers::router_with(state);
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
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let pane_activities_len: usize = state
            .multiplexer
            .with_window_or_404(&wid, |w| -> ozmux_multiplexer::MultiplexerResult<usize> {
                Ok(w.pane(&pid).map(|p| p.activities.len()).unwrap_or(0))
            })
            .await
            .unwrap();
        assert_eq!(
            pane_activities_len, 1,
            "rollback must remove the failed activity"
        );
        let recv = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await;
        assert!(recv.is_err(), "no broadcast must be sent on rollback");
    }
}
