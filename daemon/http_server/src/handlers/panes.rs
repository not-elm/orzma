use crate::extractors::ExtensionName;
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
    Activity, ActivityId, MultiplexerError, MultiplexerResult, PaneId, SetActivePaneOutcome, Side,
    SplitOrientation, WindowId,
};
use ozmux_terminal::SpawnOptions;
use serde::Deserialize;

#[derive(Deserialize)]
pub struct SplitRequest {
    orientation: SplitOrientation,
    #[serde(default)]
    side: Side,
}

pub async fn split(
    State(state): State<AppState>,
    Path(target_pane_id): Path<PaneId>,
    Json(req): Json<SplitRequest>,
) -> HttpResult<(StatusCode, Json<serde_json::Value>)> {
    let wid = lookup_pane_window(&state, &target_pane_id)?;

    let (new_pane_id, new_activity_id) = state
        .with_window_or_404(&wid, |w| -> MultiplexerResult<_> {
            let new_pane_id = PaneId::new();
            let new_activity_id = ActivityId::new();
            let activity = Activity::terminal(new_activity_id.clone());
            w.split_pane(
                &target_pane_id,
                new_pane_id.clone(),
                activity,
                req.side,
                req.orientation,
            )?;
            Ok((new_pane_id, new_activity_id))
        })
        .await?;

    state
        .pane_owner_window
        .insert(new_pane_id.clone(), wid.clone());

    publish_window_layout(&state, &wid).await;

    if let Err(spawn_err) = state
        .terminal
        .spawn(
            new_pane_id.clone(),
            new_activity_id.clone(),
            SpawnOptions {
                cols: 80,
                rows: 24,
                shell: std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into()),
                cwd: None,
            },
        )
        .await
    {
        rollback_split(&state, &wid, &new_pane_id).await;
        return Err(spawn_err.into());
    }

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "new_pane_id": new_pane_id,
            "new_activity_id": new_activity_id,
        })),
    ))
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

#[derive(Deserialize)]
pub struct CreatePaneRequest {
    activity_id: ActivityId,
}

pub async fn create(
    ExtensionName(ext_name): ExtensionName,
    State(state): State<AppState>,
    Json(body): Json<CreatePaneRequest>,
) -> HttpResult<(StatusCode, Json<serde_json::Value>)> {
    let owner = state
        .extensions
        .activity_owner(&body.activity_id)
        .ok_or(HttpError::Session(MultiplexerError::ActivityNotFound(
            body.activity_id.clone(),
        )))?;
    if owner != ext_name {
        return Err(HttpError::ActivityNotOwned);
    }
    if !state.limbo.activities.contains_key(&body.activity_id) {
        return Err(HttpError::Session(MultiplexerError::ActivityNotFound(
            body.activity_id,
        )));
    }
    let pane_id = PaneId::new();
    state.limbo.panes.insert(pane_id.clone(), body.activity_id);
    state.extensions.record_pane_owner(&pane_id, &ext_name);
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({ "pane_id": pane_id })),
    ))
}

#[derive(Deserialize)]
pub struct SplitWithRequest {
    pane_id: PaneId,
    side: Side,
    orientation: SplitOrientation,
}

pub async fn split_with(
    ExtensionName(ext_name): ExtensionName,
    State(state): State<AppState>,
    Path(src): Path<PaneId>,
    Json(body): Json<SplitWithRequest>,
) -> HttpResult<StatusCode> {
    let owner = state
        .extensions
        .pane_owner(&body.pane_id)
        .ok_or_else(|| HttpError::Session(MultiplexerError::PaneNotFound(body.pane_id.clone())))?;
    if owner != ext_name {
        return Err(HttpError::PaneNotOwned);
    }

    // The plan requires the limbo lookup to happen *before* the window
    // lock, so we move the Activity out of the limbo store eagerly.
    let already_placed = state.pane_owner_window.contains_key(&body.pane_id);
    if already_placed {
        return Err(HttpError::Session(MultiplexerError::PaneAlreadyPlaced(
            body.pane_id,
        )));
    }
    let activity = take_limbo_pane(&state, &body.pane_id)?;
    let wid = lookup_pane_window(&state, &src)?;
    let new_pane_id = body.pane_id.clone();

    state
        .with_window_or_404(&wid, |w| {
            w.split_pane(&src, new_pane_id, activity, body.side, body.orientation)
        })
        .await?;

    state
        .pane_owner_window
        .insert(body.pane_id.clone(), wid.clone());

    publish_window_layout(&state, &wid).await;
    Ok(StatusCode::NO_CONTENT)
}

fn take_limbo_pane(state: &AppState, pane_id: &PaneId) -> HttpResult<Activity> {
    let activity_id = state
        .limbo
        .panes
        .remove(pane_id)
        .map(|(_, v)| v)
        .ok_or_else(|| HttpError::Session(MultiplexerError::PaneNotFound(pane_id.clone())))?;
    state
        .limbo
        .activities
        .remove(&activity_id)
        .map(|(_, v)| v)
        .ok_or(HttpError::Session(MultiplexerError::ActivityNotFound(
            activity_id,
        )))
}

pub async fn close(
    State(state): State<AppState>,
    Path(pane_id): Path<PaneId>,
) -> HttpResult<StatusCode> {
    if let Some((_, activity_id)) = state.limbo.panes.remove(&pane_id) {
        state.limbo.activities.remove(&activity_id);
        state.extensions.forget_pane(&pane_id);
        state.extensions.forget_activity(&activity_id);
        let _ = state.terminal.kill(&activity_id).await;
        return Ok(StatusCode::NO_CONTENT);
    }

    let wid = lookup_pane_window(&state, &pane_id)?;
    let activities = state
        .with_window_or_404(&wid, |w| w.close_pane(&pane_id))
        .await?;

    state.pane_owner_window.remove(&pane_id);
    state.extensions.forget_pane(&pane_id);
    for aid in &activities {
        state.extensions.forget_activity(aid);
    }
    for aid in &activities {
        let _ = state.terminal.kill(aid).await;
    }

    publish_window_layout(&state, &wid).await;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::{bootstrap_default, fresh_state, router_with};
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use ozmux_multiplexer::Activity;
    use std::path::PathBuf;
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
    async fn split_either_succeeds_or_rolls_back() {
        let state = fresh_state();
        let (_sid, _wid, pid, _aid) = bootstrap_default(&state).await;
        let panes_before = total_panes(&state).await;
        let (router, state) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/panes/{}/split", pid))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"orientation":"horizontal"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
        let panes_after = total_panes(&state).await;
        if status == StatusCode::CREATED {
            assert!(v["new_pane_id"].is_string());
            assert!(v["new_activity_id"].is_string());
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
    async fn close_pane_without_header_returns_204_and_removes_it() {
        let state = fresh_state();
        let (_sid, wid, original_pid, _aid) = bootstrap_default(&state).await;
        let new_pid = split_via_window(&state, &wid, &original_pid).await;
        let panes_before = total_panes(&state).await;
        let (router, state) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/panes/{}", new_pid))
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
    async fn split_unknown_pane_returns_404() {
        let (router, _) = router_with(fresh_state());
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/panes/missing/split")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"orientation":"horizontal"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn close_last_pane_returns_409() {
        let state = fresh_state();
        let (_sid, _wid, pid, _aid) = bootstrap_default(&state).await;
        let (router, _state) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/panes/{}", pid))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn close_missing_pane_returns_404() {
        let (router, _) = router_with(fresh_state());
        let resp = router
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/panes/missing")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn close_owned_pane_without_header_forgets_owner() {
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
                    .uri(format!("/panes/{}", new_pid))
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

    async fn router_with_owned_activity(ext_name: &str) -> (axum::Router, ActivityId, AppState) {
        let state = fresh_state();
        let _ = bootstrap_default(&state).await;
        let activity = Activity::extension(ActivityId::new(), "ext", PathBuf::from("/tmp"));
        let activity_id = activity.id.clone();
        state.limbo.activities.insert(activity_id.clone(), activity);
        state
            .extensions
            .register(ext_name, std::path::Path::new("/tmp"));
        state
            .extensions
            .record_activity_owner(&activity_id, ext_name);
        let (router, state) = router_with(state);
        (router, activity_id, state)
    }

    #[tokio::test]
    async fn create_pane_returns_201_with_pane_id() {
        let (router, activity_id, _) = router_with_owned_activity("memo").await;
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/panes")
                    .header("X-Ozmux-Extension", "memo")
                    .header("content-type", "application/json")
                    .body(Body::from(format!(r#"{{"activity_id":"{activity_id}"}}"#)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(v["pane_id"].is_string());
    }

    #[tokio::test]
    async fn create_pane_rejects_other_extensions_activity() {
        let state = fresh_state();
        let _ = bootstrap_default(&state).await;
        let activity = Activity::terminal(ActivityId::new());
        let aid_other = activity.id.clone();
        state.limbo.activities.insert(aid_other.clone(), activity);
        state
            .extensions
            .register("memo", std::path::Path::new("/tmp"));
        state
            .extensions
            .register("other", std::path::Path::new("/tmp"));
        state.extensions.record_activity_owner(&aid_other, "other");
        let (router, _) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/panes")
                    .header("X-Ozmux-Extension", "memo")
                    .header("content-type", "application/json")
                    .body(Body::from(format!(r#"{{"activity_id":"{aid_other}"}}"#)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn create_pane_returns_404_for_unknown_activity() {
        let state = fresh_state();
        state
            .extensions
            .register("memo", std::path::Path::new("/tmp"));
        let (router, _) = router_with(state);
        let phantom = ActivityId::new();
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/panes")
                    .header("X-Ozmux-Extension", "memo")
                    .header("content-type", "application/json")
                    .body(Body::from(format!(r#"{{"activity_id":"{phantom}"}}"#)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn split_with_places_limbo_pane_after_target() {
        let state = fresh_state();
        let (_sid, _wid, target_pane, _aid) = bootstrap_default(&state).await;
        let activity = Activity::extension(ActivityId::new(), "ext", PathBuf::from("/tmp"));
        let activity_id = activity.id.clone();
        state.limbo.activities.insert(activity_id.clone(), activity);
        let limbo = PaneId::new();
        state.limbo.panes.insert(limbo.clone(), activity_id.clone());
        state
            .extensions
            .register("memo", std::path::Path::new("/tmp"));
        state.extensions.record_activity_owner(&activity_id, "memo");
        state.extensions.record_pane_owner(&limbo, "memo");
        let (router, _) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/panes/{target_pane}/split-with"))
                    .header("X-Ozmux-Extension", "memo")
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"pane_id":"{limbo}","side":"after","orientation":"horizontal"}}"#
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn split_with_rejects_pane_owned_by_other_extension() {
        let state = fresh_state();
        let (_sid, _wid, target_pane, _aid) = bootstrap_default(&state).await;
        let activity = Activity::terminal(ActivityId::new());
        let activity_id = activity.id.clone();
        state.limbo.activities.insert(activity_id.clone(), activity);
        let limbo = PaneId::new();
        state.limbo.panes.insert(limbo.clone(), activity_id);
        state
            .extensions
            .register("memo", std::path::Path::new("/tmp"));
        state
            .extensions
            .register("other", std::path::Path::new("/tmp"));
        state.extensions.record_pane_owner(&limbo, "other");
        let (router, _) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/panes/{target_pane}/split-with"))
                    .header("X-Ozmux-Extension", "memo")
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"pane_id":"{limbo}","side":"after","orientation":"horizontal"}}"#
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn split_publishes_layout_to_subscriber() {
        let state = fresh_state();
        let (_sid, wid, pid, _aid) = bootstrap_default(&state).await;
        let (router, state) = router_with(state);

        let mut rx = state.layout_broadcast.subscribe_or_create(&wid);

        let _ = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/panes/{}/split", pid))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"orientation":"horizontal"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        match tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await {
            Ok(Ok(view)) => {
                assert_eq!(view["id"].as_str(), Some(wid.as_ref()));
                let layout_child = &view["layout"]["child"];
                assert_eq!(layout_child["type"].as_str(), Some("split"));
            }
            Ok(Err(e)) => panic!("recv error: {e:?}"),
            Err(_) => panic!("publish timed out — split handler did not publish"),
        }
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
                    .uri(format!("/panes/{}", pid_b))
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
    async fn split_with_publishes_layout_to_subscriber() {
        let state = fresh_state();
        let (_sid, wid, src_pid, _aid) = bootstrap_default(&state).await;
        let activity = Activity::extension(ActivityId::new(), "ext", PathBuf::from("/tmp"));
        let aid_new = activity.id.clone();
        state.limbo.activities.insert(aid_new.clone(), activity);
        let new_pid = PaneId::new();
        state.limbo.panes.insert(new_pid.clone(), aid_new);
        state
            .extensions
            .register("memo", std::path::Path::new("/tmp"));
        state.extensions.record_pane_owner(&new_pid, "memo");
        let mut rx = state.layout_broadcast.subscribe_or_create(&wid);
        let (router, _state) = router_with(state);

        let body = format!(
            r#"{{"pane_id":"{}","side":"after","orientation":"horizontal"}}"#,
            new_pid
        );
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/panes/{}/split-with", src_pid))
                    .header("X-Ozmux-Extension", "memo")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
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
        assert_eq!(view["layout"]["child"]["type"].as_str(), Some("split"));
    }

    #[tokio::test]
    async fn split_with_returns_409_for_already_placed_pane() {
        let state = fresh_state();
        let (_sid, _wid, target_pane, _aid) = bootstrap_default(&state).await;
        state
            .extensions
            .register("memo", std::path::Path::new("/tmp"));
        state.extensions.record_pane_owner(&target_pane, "memo");
        let (router, _) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/panes/{target_pane}/split-with"))
                    .header("X-Ozmux-Extension", "memo")
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"pane_id":"{target_pane}","side":"after","orientation":"horizontal"}}"#
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
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
}
