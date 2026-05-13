use crate::window_view::WindowView;
use crate::{AppState, error::HttpResult};
use axum::{
    Json,
    extract::{Path, State},
};
use ozmux_multiplexer::WindowId;

pub mod create;
pub mod delete;
pub mod events;
pub mod panes;
pub mod rename;
pub mod select;

pub async fn get(
    State(state): State<AppState>,
    Path(window_id): Path<WindowId>,
) -> HttpResult<Json<WindowView>> {
    let view = state
        .multiplexer
        .with_window_or_404(&window_id, |w| WindowView::from_window(w))
        .await?;
    Ok(Json(view))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::{bootstrap_default, fresh_state, router_with};
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use ozmux_multiplexer::{Activity, ActivityId, PaneId, Side, SplitOrientation};
    use std::path::PathBuf;
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
    async fn get_returns_window_view_with_panes() {
        let state = fresh_state();
        let (_sid, wid, pid, aid) = bootstrap_default(&state).await;
        let root_cell = state
            .multiplexer
            .with_window(&wid, |w| w.root_cell.clone())
            .await
            .unwrap();

        let (router, _) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .uri(format!("/windows/{}", wid))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["id"].as_str(), Some(wid.as_ref()));
        assert!(v["name"].is_string());
        assert_eq!(v["root_cell"].as_str(), Some(root_cell.as_ref()));
        assert_eq!(v["active_pane"].as_str(), Some(pid.as_ref()));
        let panes = v["panes"].as_array().unwrap();
        assert_eq!(panes.len(), 1);
        assert_eq!(panes[0]["id"].as_str(), Some(pid.as_ref()));
        assert_eq!(panes[0]["activities"][0]["id"].as_str(), Some(aid.as_ref()));
        assert_eq!(panes[0]["active_activity"].as_str(), Some(aid.as_ref()));
    }

    #[tokio::test]
    async fn get_after_split_returns_two_panes() {
        let state = fresh_state();
        let (_sid, wid, pid, _aid) = bootstrap_default(&state).await;
        let _ = split_via_window(&state, &wid, &pid).await;

        let (router, _) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .uri(format!("/windows/{}", wid))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["panes"].as_array().map(|a| a.len()), Some(2));
    }

    #[tokio::test]
    async fn get_orphan_window_returns_window_view() {
        let state = fresh_state();
        let (wid, _, _) = state
            .multiplexer
            .create_window(None, Some("orphan".into()))
            .await
            .unwrap();
        let (router, _) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .uri(format!("/windows/{}", wid))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["id"].as_str(), Some(wid.as_ref()));
        assert_eq!(v["name"].as_str(), Some("orphan"));
        assert_eq!(v["panes"].as_array().map(|a| a.len()), Some(1));
    }

    #[tokio::test]
    async fn get_unknown_window_returns_404() {
        let (router, _) = router_with(fresh_state());
        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/windows/missing")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["code"].as_str(), Some("WINDOW_NOT_FOUND"));
    }

    #[tokio::test]
    async fn get_window_active_activity_matches_initial_activity() {
        let state = fresh_state();
        let (_sid, wid, _pid, aid) = bootstrap_default(&state).await;
        let (router, _) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .uri(format!("/windows/{}", wid))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            v["panes"][0]["active_activity"].as_str(),
            Some(aid.as_ref())
        );
    }

    #[tokio::test]
    async fn get_window_returns_activities_with_kind_for_terminal() {
        let state = fresh_state();
        let (_sid, wid, _pid, _aid) = bootstrap_default(&state).await;
        let (router, _) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .uri(format!("/windows/{}", wid))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let panes = v["panes"].as_array().unwrap();
        let activities = panes[0]["activities"].as_array().unwrap();
        assert!(activities[0]["id"].is_string());
        assert_eq!(activities[0]["kind"].as_str(), Some("terminal"));
    }

    #[tokio::test]
    async fn get_window_includes_layout_root_with_pane_for_single_pane_window() {
        let state = fresh_state();
        let (_sid, wid, pid, _aid) = bootstrap_default(&state).await;
        let (router, _) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .uri(format!("/windows/{}", wid))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let layout = &v["layout"];
        assert_eq!(layout["type"].as_str(), Some("root"));
        let child = &layout["child"];
        assert_eq!(child["type"].as_str(), Some("pane"));
        assert_eq!(child["pane_id"].as_str(), Some(pid.as_ref()));
    }

    #[tokio::test]
    async fn get_window_layout_after_split_has_split_node() {
        let state = fresh_state();
        let (_sid, wid, pid, _aid) = bootstrap_default(&state).await;
        let _ = split_via_window(&state, &wid, &pid).await;
        let (router, _) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .uri(format!("/windows/{}", wid))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let split = &v["layout"]["child"];
        assert_eq!(split["type"].as_str(), Some("split"));
        assert_eq!(split["orientation"].as_str(), Some("horizontal"));
        assert!(split["lhs"].is_object());
        assert!(split["rhs"].is_object());
    }

    #[tokio::test]
    async fn get_window_includes_iframe_url_for_extension_activity() {
        let state = fresh_state();
        let (_sid, wid, bootstrap_pane, _aid) = bootstrap_default(&state).await;
        let activity = Activity::extension(ActivityId::new(), "ext", PathBuf::from("/tmp"));
        let activity_id = activity.id.clone();
        let new_pane = PaneId::new();
        state
            .multiplexer
            .with_window_or_404(&wid, |w| {
                w.split_pane(
                    &bootstrap_pane,
                    new_pane.clone(),
                    activity,
                    Side::After,
                    SplitOrientation::Horizontal,
                )
            })
            .await
            .unwrap();
        state
            .multiplexer
            .pane_owner_window
            .insert(new_pane.clone(), wid.clone());

        let (router, _) = router_with(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .uri(format!("/windows/{}", wid))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let panes = v["panes"].as_array().unwrap();
        let ext_pane = panes
            .iter()
            .find(|p| p["activities"][0]["kind"].as_str() == Some("extension"))
            .expect("extension pane not found");
        let iframe_url = ext_pane["activities"][0]["iframe_url"].as_str().unwrap();
        assert_eq!(
            iframe_url,
            format!("/windows/{wid}/panes/{new_pane}/activities/{activity_id}/iframe/index.html")
        );
    }
}
