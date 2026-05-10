use crate::{MultiplexerState, error::{HttpError, HttpResult}};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use crate::extractors::ExtensionName;
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
    Path(pane_id): Path<PaneId>,
    Json(req): Json<SplitRequest>,
) -> HttpResult<(StatusCode, Json<serde_json::Value>)> {
    let (new_pane_id, new_activity_id) = {
        let mut ms = ms.lock().await;
        ms.split_pane(pane_id, req.side, req.orientation)?
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
        if let Err(rollback_err) = ms.lock().await.close_pane(&new_pane_id) {
            tracing::warn!(
                error = %rollback_err,
                new_pane_id = %new_pane_id,
                "split rollback failed to close pane after spawn failure"
            );
        }
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
    let owner = registry
        .activity_owner(&body.activity_id)
        .ok_or_else(|| {
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

pub async fn close(
    State(ms): State<MultiplexerState>,
    State(terminal): State<TerminalService>,
    Path(pane_id): Path<PaneId>,
) -> HttpResult<StatusCode> {
    let activities_to_kill = {
        let ms = ms.lock().await;
        ms.panes()
            .get(&pane_id)
            .map(|p| p.activities.clone())
            .unwrap_or_default()
    };
    {
        let mut ms = ms.lock().await;
        ms.close_pane(&pane_id)?;
    }
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
    use ozmux_multiplexer::MultiplexerService;
    use ozmux_extension::ExtensionRegistry;
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
    async fn close_non_last_pane_returns_204_and_removes_it() {
        let mut ms = MultiplexerService::default();
        let (_sid, _wid, original_pid, _aid) = ms.bootstrap_default().unwrap();
        // Split to have 2 panes (without going through HTTP, to avoid PTY).
        let (new_pid, _new_aid) = ms
            .split_pane(
                original_pid.clone(),
                Side::After,
                SplitOrientation::Horizontal,
            )
            .unwrap();
        let panes_before = ms.panes().len();
        let (router, state) = router_with(ms);

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
    async fn close_last_pane_returns_409() {
        let mut ms = MultiplexerService::default();
        let (_sid, _wid, pid, _aid) = ms.bootstrap_default().unwrap();
        let (router, _) = router_with(ms);
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
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["code"].as_str(), Some("CANNOT_CLOSE_LAST_PANE"));
    }

    #[tokio::test]
    async fn close_unknown_pane_returns_404() {
        let (router, _) = router_with(MultiplexerService::default());
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
                    .body(Body::from(format!(
                        r#"{{"activity_id":"{activity_id}"}}"#
                    )))
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
}
