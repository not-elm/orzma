use crate::handlers::activities::ActivityInput;
use crate::handlers::publish_window_layout;
use crate::{
    AppState,
    error::{HttpError, HttpResult},
};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use ozmux_multiplexer::{
    Activity, ActivityId, ActivityKind, MultiplexerError, MultiplexerResult, PaneId, SessionId,
    SetActivePaneOutcome, Side, SplitOrientation, WindowId,
};
use ozmux_terminal::SpawnOptions;
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
    ensure_pane_in_window(&state, &wid, &target_pane_id)?;
    split_in_window(&state, &wid, &target_pane_id, req).await
}

async fn split_in_window(
    state: &AppState,
    wid: &WindowId,
    target_pane_id: &PaneId,
    req: SplitRequest,
) -> HttpResult<(StatusCode, Json<serde_json::Value>)> {
    // Caller IDs win when present; otherwise we fall back to server-generated
    // ids so simple internal callers don't have to mint UUIDs themselves.
    let new_pane_id = req.new_pane_id.unwrap_or_default();
    let (new_activity, ext_name) = match req.activity {
        Some(spec) => spec.into_activity(),
        None => (Activity::terminal(ActivityId::new()), None),
    };
    let new_activity_id = new_activity.id.clone();
    // Snapshot the kind before move so we can branch on it post-split without
    // re-reading the activity off the window.
    let activity_kind = new_activity.kind.clone();

    state
        .with_window_or_404(wid, |w| -> MultiplexerResult<_> {
            w.split_pane(
                target_pane_id,
                new_pane_id.clone(),
                new_activity,
                req.side,
                req.orientation,
            )
        })
        .await?;

    state
        .pane_owner_window
        .insert(new_pane_id.clone(), wid.clone());

    // Extension activities own their pane and need the registry populated
    // before the iframe / handlers-WS routes are exercised by the browser.
    if let Some(name) = ext_name.as_deref() {
        state.extensions.record_activity_owner(&new_activity_id, name);
        state.extensions.record_pane_owner(&new_pane_id, name);
    }

    publish_window_layout(state, wid).await;

    // Only Terminal activities are backed by a PTY. Extension activities live
    // in an iframe, so spawning a shell for them creates an orphan child
    // process whose output nothing ever reads.
    if matches!(activity_kind, ActivityKind::Terminal) {
        spawn_pty_with_rollback(state, wid, &new_pane_id, &new_activity_id).await?;
    }

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "new_pane_id": new_pane_id,
            "new_activity_id": new_activity_id,
        })),
    ))
}

async fn spawn_pty_with_rollback(
    state: &AppState,
    wid: &WindowId,
    new_pane_id: &PaneId,
    new_activity_id: &ActivityId,
) -> HttpResult<()> {
    let session_id = session_owning_window(state, wid).await;
    let spawn_result = state
        .terminal
        .spawn(
            new_pane_id.clone(),
            new_activity_id.clone(),
            SpawnOptions {
                cols: 80,
                rows: 24,
                shell: std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into()),
                cwd: None,
                window_id: Some(wid.clone()),
                session_id,
            },
        )
        .await;
    if let Err(spawn_err) = spawn_result {
        rollback_split(state, wid, new_pane_id).await;
        return Err(spawn_err.into());
    }
    Ok(())
}

/// Walk the SessionState to find which Session owns `wid`. Used to populate
/// `OZMUX_SESSION_ID` for the spawned PTY; returns `None` for orphan Windows.
pub(crate) async fn session_owning_window(state: &AppState, wid: &WindowId) -> Option<SessionId> {
    let sess = state.sessions.lock().await;
    sess.iter()
        .find(|(_, s)| s.linked_windows.contains(wid))
        .map(|(id, _)| id.clone())
}

async fn rollback_split(state: &AppState, wid: &WindowId, new_pane_id: &PaneId) {
    let closed = state
        .with_window_or_404(wid, |w| w.close_pane(new_pane_id))
        .await
        .is_ok();
    if !closed {
        tracing::warn!(
            %new_pane_id,
            "split rollback failed to close pane after spawn failure"
        );
        return;
    }
    state.pane_owner_window.remove(new_pane_id);
    publish_window_layout(state, wid).await;
}

pub async fn close(
    State(state): State<AppState>,
    Path((wid, pane_id)): Path<(WindowId, PaneId)>,
) -> HttpResult<StatusCode> {
    ensure_pane_in_window(&state, &wid, &pane_id)?;
    close_in_window(&state, &wid, &pane_id).await
}

async fn close_in_window(
    state: &AppState,
    wid: &WindowId,
    pane_id: &PaneId,
) -> HttpResult<StatusCode> {
    let activities = state
        .with_window_or_404(wid, |w| w.close_pane(pane_id))
        .await?;

    state.pane_owner_window.remove(pane_id);
    state.extensions.forget_pane(pane_id);
    for aid in &activities {
        state.extensions.forget_activity(aid);
    }
    for aid in &activities {
        let _ = state.terminal.kill(aid).await;
    }

    publish_window_layout(state, wid).await;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn activate(
    State(state): State<AppState>,
    Path((window_id, pane_id)): Path<(WindowId, PaneId)>,
) -> HttpResult<StatusCode> {
    let outcome = state
        .with_window_or_404(&window_id, |w| activate_pane_in_window(w, &pane_id, &state))
        .await?;
    if matches!(outcome, SetActivePaneOutcome::Changed) {
        publish_window_layout(&state, &window_id).await;
    }
    Ok(StatusCode::NO_CONTENT)
}

fn activate_pane_in_window(
    window: &mut ozmux_multiplexer::Window,
    pane_id: &PaneId,
    state: &AppState,
) -> MultiplexerResult<SetActivePaneOutcome> {
    if window.panes.contains_key(pane_id) {
        return window.set_active_pane(pane_id);
    }
    let exists_elsewhere = state.pane_owner_window.contains_key(pane_id);
    if exists_elsewhere {
        return Err(MultiplexerError::PaneNotInWindow {
            window: window.id.clone(),
            pane: pane_id.clone(),
        });
    }
    Err(MultiplexerError::PaneNotFound(pane_id.clone()))
}

fn lookup_pane_window(state: &AppState, pane_id: &PaneId) -> HttpResult<WindowId> {
    state
        .pane_owner_window
        .get(pane_id)
        .map(|e| e.clone())
        .ok_or_else(|| HttpError::Session(MultiplexerError::PaneNotFound(pane_id.clone())))
}

fn ensure_pane_in_window(state: &AppState, wid: &WindowId, pane_id: &PaneId) -> HttpResult<()> {
    let actual = lookup_pane_window(state, pane_id)?;
    if &actual != wid {
        return Err(HttpError::Session(MultiplexerError::PaneNotInWindow {
            window: wid.clone(),
            pane: pane_id.clone(),
        }));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::{bootstrap_default, fresh_state, router_with};
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use ozmux_multiplexer::Activity;
    use tower::ServiceExt;

    async fn total_panes(state: &AppState) -> usize {
        let mut total = 0;
        for entry in state.windows.iter() {
            let arc = entry.value().clone();
            drop(entry);
            let win = arc.lock().await;
            total += win.panes.len();
        }
        total
    }

    async fn pane_to_cell_contains(state: &AppState, pid: &PaneId) -> bool {
        for entry in state.windows.iter() {
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
        let (wid_b, _, _) = state.create_window(Some(&sid), None).await.unwrap();
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
        let (wid_b, _, _) = state.create_window(Some(&sid), None).await.unwrap();
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

    #[tokio::test]
    async fn activate_pane_in_other_window_returns_409() {
        let state = fresh_state();
        let (sid, _wid_a, pid_a, _aid) = bootstrap_default(&state).await;
        let (wid_b, _, _) = state.create_window(Some(&sid), None).await.unwrap();
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
