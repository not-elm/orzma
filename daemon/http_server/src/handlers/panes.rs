use crate::extractors::ExtensionName;
use crate::{
    MultiplexerState,
    error::{HttpError, HttpResult},
};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use ozmux_extension::ExtensionRegistry;
use ozmux_multiplexer::{
    activity::ActivityId,
    cells::{Side, SplitOrientation},
    pane::PaneId,
};
use ozmux_terminal::{SpawnOptions, TerminalService};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct SplitRequest {
    orientation: SplitOrientation,
    #[serde(default)]
    side: Side,
}

pub async fn split(
    State(ms): State<MultiplexerState>,
    State(terminal): State<TerminalService>,
    State(broadcaster): State<crate::layout_broadcast::LayoutBroadcaster>,
    Path(pane_id): Path<PaneId>,
    Json(req): Json<SplitRequest>,
) -> HttpResult<(StatusCode, Json<serde_json::Value>)> {
    let (new_pane_id, new_activity_id, wid) = {
        let mut ms = ms.lock().await;
        let (new_pane_id, new_activity_id) = ms.split_pane(pane_id, req.side, req.orientation)?;
        // Publish the new layout snapshot while still holding the lock.
        let wid = ms.window_id_of_pane(&new_pane_id).ok();
        if let Some(wid) = wid.as_ref() {
            if let Some(window) = ms.windows().get(wid) {
                match crate::handlers::windows::window_view_for(&ms, wid, window) {
                    Ok(view) => broadcaster.publish(wid, view),
                    Err(e) => tracing::warn!(error = %e, %wid, "skipped layout publish on split"),
                }
            }
        }
        (new_pane_id, new_activity_id, wid)
    };

    if let Err(spawn_err) = terminal
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
        // Rollback: close the new pane and publish a corrective snapshot so
        // subscribers don't see the phantom split that just got reverted.
        let mut ms = ms.lock().await;
        let close_ok = ms.close_pane(&new_pane_id).is_ok();
        if !close_ok {
            tracing::warn!(
                new_pane_id = %new_pane_id,
                "split rollback failed to close pane after spawn failure"
            );
        }
        if close_ok {
            if let Some(wid) = wid.as_ref() {
                if let Some(window) = ms.windows().get(wid) {
                    match crate::handlers::windows::window_view_for(&ms, wid, window) {
                        Ok(view) => broadcaster.publish(wid, view),
                        Err(e) => {
                            tracing::warn!(error = %e, %wid, "skipped corrective publish on split rollback")
                        }
                    }
                }
            }
        }
        drop(ms);
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

#[derive(Deserialize)]
pub struct CreatePaneRequest {
    activity_id: ActivityId,
}

pub async fn create(
    ExtensionName(ext_name): ExtensionName,
    State(ms): State<MultiplexerState>,
    State(registry): State<ExtensionRegistry>,
    Json(body): Json<CreatePaneRequest>,
) -> HttpResult<(StatusCode, Json<serde_json::Value>)> {
    let owner = registry.activity_owner(&body.activity_id).ok_or_else(|| {
        HttpError::Session(ozmux_multiplexer::SessionError::ActivityNotFound(
            body.activity_id.clone(),
        ))
    })?;
    if owner != ext_name {
        return Err(HttpError::ActivityNotOwned);
    }
    let pane_id = {
        let mut ms = ms.lock().await;
        ms.new_pane_with_activity(body.activity_id)?
    };
    registry.record_pane_owner(&pane_id, &ext_name);
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
    State(ms): State<MultiplexerState>,
    State(registry): State<ExtensionRegistry>,
    State(broadcaster): State<crate::layout_broadcast::LayoutBroadcaster>,
    Path(src): Path<PaneId>,
    Json(body): Json<SplitWithRequest>,
) -> HttpResult<StatusCode> {
    // src は owner check しない (D11)
    let owner = registry.pane_owner(&body.pane_id).ok_or_else(|| {
        HttpError::Session(ozmux_multiplexer::SessionError::PaneNotFound(
            body.pane_id.clone(),
        ))
    })?;
    if owner != ext_name {
        return Err(HttpError::PaneNotOwned);
    }
    {
        let mut ms = ms.lock().await;
        ms.split_with_pane(src.clone(), body.pane_id, body.side, body.orientation)?;
        if let Ok(wid) = ms.window_id_of_pane(&src) {
            if let Some(window) = ms.windows().get(&wid) {
                match crate::handlers::windows::window_view_for(&ms, &wid, window) {
                    Ok(view) => broadcaster.publish(&wid, view),
                    Err(e) => tracing::warn!(error = %e, %wid, "skipped layout publish on split_with"),
                }
            }
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn close(
    ExtensionName(ext_name): ExtensionName,
    State(ms): State<MultiplexerState>,
    State(terminal): State<TerminalService>,
    State(registry): State<ExtensionRegistry>,
    State(broadcaster): State<crate::layout_broadcast::LayoutBroadcaster>,
    Path(pane_id): Path<PaneId>,
) -> HttpResult<StatusCode> {
    // 1. owner check（system-owned pane は entry 無しで 403）
    let owner = registry
        .pane_owner(&pane_id)
        .ok_or(HttpError::PaneNotOwned)?;
    if owner != ext_name {
        return Err(HttpError::PaneNotOwned);
    }
    // 2. lock 内で activities 取得 + close
    let activities_to_kill = {
        let mut ms = ms.lock().await;
        // Capture wid BEFORE the mutation removes the pane index.
        let wid = ms.window_id_of_pane(&pane_id).ok();
        let activities = ms
            .panes()
            .get(&pane_id)
            .map(|p| p.activities.clone())
            .unwrap_or_default();
        ms.close_pane(&pane_id)?;
        // After close, publish the new layout while still holding the lock.
        if let Some(wid) = wid {
            if let Some(window) = ms.windows().get(&wid) {
                match crate::handlers::windows::window_view_for(&ms, &wid, window) {
                    Ok(view) => broadcaster.publish(&wid, view),
                    Err(e) => tracing::warn!(error = %e, %wid, "skipped layout publish on close"),
                }
            }
        }
        activities
    };
    // 3. registry forget
    registry.forget_pane(&pane_id);
    for aid in &activities_to_kill {
        registry.forget_activity(aid);
    }
    // 4. terminal kill (best-effort)
    for aid in activities_to_kill {
        let _ = terminal.kill(&aid).await;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::router_with;
    use crate::{AppState, TerminalService};
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use ozmux_extension::ExtensionRegistry;
    use ozmux_multiplexer::MultiplexerService;
    use ozmux_multiplexer::activity::{Activity, ActivityKind};
    use std::path::PathBuf;
    use tower::ServiceExt;

    #[tokio::test]
    async fn split_either_succeeds_or_rolls_back() {
        let mut ms = MultiplexerService::default();
        let (_sid, _wid, pid, _aid) = ms.bootstrap_default().unwrap();
        let panes_before = ms.panes().len();
        let (router, state) = router_with(ms);
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
        let panes_after = state.multiplexer.lock().await.panes().len();
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
    async fn close_owned_non_last_pane_returns_204_and_removes_it() {
        let mut ms = MultiplexerService::default();
        let (_sid, _wid, original_pid, _aid) = ms.bootstrap_default().unwrap();
        let (new_pid, _new_aid) = ms
            .split_pane(
                original_pid.clone(),
                Side::After,
                SplitOrientation::Horizontal,
            )
            .unwrap();
        let panes_before = ms.panes().len();
        let registry = ExtensionRegistry::default();
        registry.register("memo", std::path::Path::new("/tmp"));
        registry.record_pane_owner(&new_pid, "memo");
        let state = AppState {
            multiplexer: crate::MultiplexerState(std::sync::Arc::new(tokio::sync::Mutex::new(ms))),
            terminal: TerminalService::default(),
            extensions: registry,
            layout_broadcast: crate::layout_broadcast::LayoutBroadcaster::default(),
        };
        let router = crate::test_helpers::daemon_router_for_test(state.clone());
        let resp = router
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/panes/{}", new_pid))
                    .header("X-Ozmux-Extension", "memo")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let ms = state.multiplexer.lock().await;
        assert_eq!(ms.panes().len(), panes_before - 1);
        assert!(!ms.pane_to_cell_index().contains_key(&new_pid));
    }

    #[tokio::test]
    async fn split_unknown_pane_returns_404() {
        let (router, _) = router_with(MultiplexerService::default());
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
    async fn close_owned_last_pane_returns_409() {
        let mut ms = MultiplexerService::default();
        let (_sid, _wid, pid, _aid) = ms.bootstrap_default().unwrap();
        let registry = ExtensionRegistry::default();
        registry.register("memo", std::path::Path::new("/tmp"));
        registry.record_pane_owner(&pid, "memo");
        let state = AppState {
            multiplexer: crate::MultiplexerState(std::sync::Arc::new(tokio::sync::Mutex::new(ms))),
            terminal: TerminalService::default(),
            extensions: registry,
            layout_broadcast: crate::layout_broadcast::LayoutBroadcaster::default(),
        };
        let router = crate::test_helpers::daemon_router_for_test(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/panes/{}", pid))
                    .header("X-Ozmux-Extension", "memo")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn close_unknown_pane_returns_403_owner_not_found() {
        let registry = ExtensionRegistry::default();
        registry.register("memo", std::path::Path::new("/tmp"));
        let state = AppState {
            multiplexer: crate::MultiplexerState(std::sync::Arc::new(tokio::sync::Mutex::new(
                MultiplexerService::default(),
            ))),
            terminal: TerminalService::default(),
            extensions: registry,
            layout_broadcast: crate::layout_broadcast::LayoutBroadcaster::default(),
        };
        let router = crate::test_helpers::daemon_router_for_test(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/panes/missing")
                    .header("X-Ozmux-Extension", "memo")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn close_pane_owned_by_other_extension_returns_403() {
        let mut ms = MultiplexerService::default();
        let (_sid, _wid, original_pid, _aid) = ms.bootstrap_default().unwrap();
        let (new_pid, _) = ms
            .split_pane(
                original_pid.clone(),
                Side::After,
                SplitOrientation::Horizontal,
            )
            .unwrap();
        let registry = ExtensionRegistry::default();
        registry.register("memo", std::path::Path::new("/tmp"));
        registry.register("other", std::path::Path::new("/tmp"));
        registry.record_pane_owner(&new_pid, "other");
        let state = AppState {
            multiplexer: crate::MultiplexerState(std::sync::Arc::new(tokio::sync::Mutex::new(ms))),
            terminal: TerminalService::default(),
            extensions: registry,
            layout_broadcast: crate::layout_broadcast::LayoutBroadcaster::default(),
        };
        let router = crate::test_helpers::daemon_router_for_test(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/panes/{}", new_pid))
                    .header("X-Ozmux-Extension", "memo")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    fn router_with_owned_activity(
        ext_name: &str,
    ) -> (
        axum::Router,
        ozmux_multiplexer::activity::ActivityId,
        AppState,
    ) {
        let mut ms = MultiplexerService::default();
        let _ = ms.bootstrap_default().unwrap();
        let activity_id = ms.new_activity(Activity {
            name: "ext".into(),
            kind: ActivityKind::Extension {
                html_root: PathBuf::from("/tmp"),
            },
        });
        let registry = ExtensionRegistry::default();
        registry.register(ext_name, std::path::Path::new("/tmp"));
        registry.record_activity_owner(&activity_id, ext_name);
        let state = AppState {
            multiplexer: crate::MultiplexerState(std::sync::Arc::new(tokio::sync::Mutex::new(ms))),
            terminal: TerminalService::default(),
            extensions: registry,
            layout_broadcast: crate::layout_broadcast::LayoutBroadcaster::default(),
        };
        (
            crate::test_helpers::daemon_router_for_test(state.clone()),
            activity_id,
            state,
        )
    }

    #[tokio::test]
    async fn create_pane_returns_201_with_pane_id() {
        let (router, activity_id, _) = router_with_owned_activity("memo");
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
        let mut ms2 = MultiplexerService::default();
        let _ = ms2.bootstrap_default().unwrap();
        let aid_other = ms2.new_activity(Activity::default());
        let registry = ExtensionRegistry::default();
        registry.register("memo", std::path::Path::new("/tmp"));
        registry.register("other", std::path::Path::new("/tmp"));
        registry.record_activity_owner(&aid_other, "other");
        let state = AppState {
            multiplexer: crate::MultiplexerState(std::sync::Arc::new(tokio::sync::Mutex::new(ms2))),
            terminal: TerminalService::default(),
            extensions: registry,
            layout_broadcast: crate::layout_broadcast::LayoutBroadcaster::default(),
        };
        let router = crate::test_helpers::daemon_router_for_test(state);
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
        let registry = ExtensionRegistry::default();
        registry.register("memo", std::path::Path::new("/tmp"));
        let state = AppState {
            multiplexer: crate::MultiplexerState(std::sync::Arc::new(tokio::sync::Mutex::new(
                MultiplexerService::default(),
            ))),
            terminal: TerminalService::default(),
            extensions: registry,
            layout_broadcast: crate::layout_broadcast::LayoutBroadcaster::default(),
        };
        let router = crate::test_helpers::daemon_router_for_test(state);
        let phantom = ozmux_multiplexer::activity::ActivityId::new();
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
        let mut ms = MultiplexerService::default();
        let (_sid, _wid, target_pane, _aid) = ms.bootstrap_default().unwrap();
        let activity_id = ms.new_activity(Activity {
            name: "ext".into(),
            kind: ActivityKind::Extension {
                html_root: PathBuf::from("/tmp"),
            },
        });
        let limbo = ms.new_pane_with_activity(activity_id.clone()).unwrap();
        let registry = ExtensionRegistry::default();
        registry.register("memo", std::path::Path::new("/tmp"));
        registry.record_activity_owner(&activity_id, "memo");
        registry.record_pane_owner(&limbo, "memo");
        let state = AppState {
            multiplexer: crate::MultiplexerState(std::sync::Arc::new(tokio::sync::Mutex::new(ms))),
            terminal: TerminalService::default(),
            extensions: registry,
            layout_broadcast: crate::layout_broadcast::LayoutBroadcaster::default(),
        };
        let router = crate::test_helpers::daemon_router_for_test(state);
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
        let mut ms = MultiplexerService::default();
        let (_sid, _wid, target_pane, _aid) = ms.bootstrap_default().unwrap();
        let activity_id = ms.new_activity(Activity::default());
        let limbo = ms.new_pane_with_activity(activity_id.clone()).unwrap();
        let registry = ExtensionRegistry::default();
        registry.register("memo", std::path::Path::new("/tmp"));
        registry.register("other", std::path::Path::new("/tmp"));
        registry.record_pane_owner(&limbo, "other");
        let state = AppState {
            multiplexer: crate::MultiplexerState(std::sync::Arc::new(tokio::sync::Mutex::new(ms))),
            terminal: TerminalService::default(),
            extensions: registry,
            layout_broadcast: crate::layout_broadcast::LayoutBroadcaster::default(),
        };
        let router = crate::test_helpers::daemon_router_for_test(state);
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
        let mut ms = MultiplexerService::default();
        let (_sid, wid, pid, _aid) = ms.bootstrap_default().unwrap();
        let (router, state) = router_with(ms);

        // Subscribe BEFORE the mutation.
        let mut rx = state.layout_broadcast.subscribe_or_create(&wid);

        // Note: split also tries to spawn a PTY; in unit-test env this will
        // typically fail and the handler rolls back. The publish is wired to
        // happen *after* the multiplexer mutation but *before* the spawn, so
        // a frame is published even when the spawn later fails. We assert
        // that frame.
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

        // Whether or not the HTTP request succeeded (PTY may have failed),
        // we should observe the post-mutation snapshot frame.
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
        let mut ms = MultiplexerService::default();
        let (_sid, wid, pid_a, _aid) = ms.bootstrap_default().unwrap();
        let (pid_b, _) = ms
            .split_pane(pid_a.clone(), Side::After, SplitOrientation::Horizontal)
            .unwrap();
        let registry = ExtensionRegistry::default();
        registry.register("memo", std::path::Path::new("/tmp"));
        registry.record_pane_owner(&pid_b, "memo");
        let state = AppState {
            multiplexer: crate::MultiplexerState(std::sync::Arc::new(tokio::sync::Mutex::new(ms))),
            terminal: TerminalService::default(),
            extensions: registry,
            layout_broadcast: crate::layout_broadcast::LayoutBroadcaster::default(),
        };
        let mut rx = state.layout_broadcast.subscribe_or_create(&wid);
        let router = crate::test_helpers::daemon_router_for_test(state.clone());

        let resp = router
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/panes/{}", pid_b))
                    .header("X-Ozmux-Extension", "memo")
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
        // After collapsing the split, the layout's child should be a single pane (pid_a).
        assert_eq!(view["layout"]["child"]["type"].as_str(), Some("pane"));
        assert_eq!(
            view["layout"]["child"]["pane_id"].as_str(),
            Some(pid_a.as_ref())
        );
    }

    #[tokio::test]
    async fn split_with_publishes_layout_to_subscriber() {
        let mut ms = MultiplexerService::default();
        let (_sid, wid, src_pid, _aid) = ms.bootstrap_default().unwrap();
        // Create a limbo pane (in panes, not in cells) — split_with places it.
        let aid_new = ms.new_activity(Activity {
            name: "ext".into(),
            kind: ActivityKind::Extension { html_root: PathBuf::from("/tmp") },
        });
        let new_pid = ms.new_pane_with_activity(aid_new).unwrap();
        let registry = ExtensionRegistry::default();
        registry.register("memo", std::path::Path::new("/tmp"));
        registry.record_pane_owner(&new_pid, "memo");
        let state = AppState {
            multiplexer: crate::MultiplexerState(std::sync::Arc::new(tokio::sync::Mutex::new(ms))),
            terminal: TerminalService::default(),
            extensions: registry,
            layout_broadcast: crate::layout_broadcast::LayoutBroadcaster::default(),
        };
        let mut rx = state.layout_broadcast.subscribe_or_create(&wid);
        let router = crate::test_helpers::daemon_router_for_test(state.clone());

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
        let mut ms = MultiplexerService::default();
        let (_sid, _wid, target_pane, _aid) = ms.bootstrap_default().unwrap();
        let registry = ExtensionRegistry::default();
        registry.register("memo", std::path::Path::new("/tmp"));
        registry.record_pane_owner(&target_pane, "memo");
        let state = AppState {
            multiplexer: crate::MultiplexerState(std::sync::Arc::new(tokio::sync::Mutex::new(ms))),
            terminal: TerminalService::default(),
            extensions: registry,
            layout_broadcast: crate::layout_broadcast::LayoutBroadcaster::default(),
        };
        let router = crate::test_helpers::daemon_router_for_test(state);
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
}
