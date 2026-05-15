use crate::AppState;
use crate::error::HttpError;
use crate::state::ActivityKindDiscriminant;
use axum::{
    extract::{Path, State},
    http::header::CONTENT_TYPE,
    response::{IntoResponse, Response},
};
use ozmux_multiplexer::{Activity, ActivityId, ActivityKind, PaneId, WindowId};
use serde::Serialize;

/// Validates (window, pane, activity) membership and injects
/// `window.__OZMUX__` globals into HTML responses so the iframe SDK can
/// discover its position in the hierarchy without parsing the URL.
pub async fn iframe_serve(
    State(state): State<AppState>,
    Path((wid, pid, aid, path)): Path<(WindowId, PaneId, ActivityId, String)>,
) -> Result<Response, HttpError> {
    let activity = state
        .ensure_activity_kind(&wid, &pid, &aid, ActivityKindDiscriminant::Extension)
        .await?;
    let session_id = crate::handlers::windows::panes::session_owning_window(&state, &wid).await;
    let ids = OzmuxIds {
        session_id: session_id.map(|s| s.to_string()),
        window_id: wid.to_string(),
        pane_id: pid.to_string(),
        activity_id: aid.to_string(),
    };
    serve_iframe_asset(&activity, &path, Some(&ids)).await
}

async fn serve_iframe_asset(
    activity: &Activity,
    path: &str,
    ctx: Option<&OzmuxIds>,
) -> Result<Response, HttpError> {
    let ActivityKind::Extension { html_root } = &activity.kind else {
        return Err(HttpError::IframeFileNotFound(path.to_string()));
    };
    let html_root_canon = html_root
        .canonicalize()
        .map_err(|_| HttpError::IframeFileNotFound(path.to_string()))?;
    let resolved = html_root_canon
        .join(path)
        .canonicalize()
        .map_err(|_| HttpError::IframeFileNotFound(path.to_string()))?;
    if !resolved.starts_with(&html_root_canon) {
        return Err(HttpError::InvalidHtmlPath(path.to_string()));
    }
    let resolved_clone = resolved.clone();
    let path_owned = path.to_string();
    let bytes = tokio::task::spawn_blocking(move || std::fs::read(&resolved_clone))
        .await
        .map_err(|_| HttpError::IframeFileNotFound(path_owned.clone()))?
        .map_err(|_| HttpError::IframeFileNotFound(path_owned))?;
    let mime = mime_guess::from_path(&resolved).first_or_octet_stream();
    // Only HTML responses carry the bootstrap script. Other assets (CSS, JS,
    // fonts, images) are served byte-for-byte so caching and integrity checks
    // stay intact.
    if let Some(ids) = ctx
        && mime.essence_str() == "text/html"
    {
        let body = String::from_utf8_lossy(&bytes);
        let injected = inject_ozmux_globals(&body, ids);
        return Ok((
            axum::http::StatusCode::OK,
            [(CONTENT_TYPE, mime.as_ref().to_string())],
            injected,
        )
            .into_response());
    }
    Ok((
        axum::http::StatusCode::OK,
        [(CONTENT_TYPE, mime.as_ref().to_string())],
        bytes,
    )
        .into_response())
}

#[derive(Serialize)]
struct OzmuxIds {
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    #[serde(rename = "windowId")]
    window_id: String,
    #[serde(rename = "paneId")]
    pane_id: String,
    #[serde(rename = "activityId")]
    activity_id: String,
}

/// Inject `<script>window.__OZMUX__={...}</script>` into the iframe HTML so the
/// SDK can read its position in the hierarchy without parsing the URL.
///
/// Injection order: after `<head>` (preferred — lands before any user script),
/// else after `<html ...>` (so the script is still in document order), else
/// prepend (degraded fallback for headless/fragmentary HTML).
fn inject_ozmux_globals(html: &str, ctx: &OzmuxIds) -> String {
    let payload = serde_json::to_string(ctx).expect("OzmuxIds is always serializable");
    let script = format!("<script>window.__OZMUX__={payload};</script>");
    if let Some(pos) = html.find("<head>") {
        let cut = pos + "<head>".len();
        return format!("{}{}{}", &html[..cut], script, &html[cut..]);
    }
    if let Some(pos) = html.find("<html")
        && let Some(end) = html[pos..].find('>')
    {
        let cut = pos + end + 1;
        return format!("{}{}{}", &html[..cut], script, &html[cut..]);
    }
    format!("{script}{html}")
}

#[cfg(test)]
mod tests {
    use crate::test_helpers;
    use ozmux_multiplexer::{Activity, ActivityId, WindowId};
    use tower::ServiceExt;

    #[tokio::test]
    async fn iframe_serve_returns_html_with_correct_content_type() {
        let (router, _state, wid, pid, aid, _tmp) =
            super::super::test_support::setup_hierarchical_extension(
                b"<html><body>memo</body></html>",
            )
            .await;
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!(
                        "/windows/{wid}/panes/{pid}/activities/{aid}/iframe/index.html"
                    ))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let ct = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(ct.starts_with("text/html"));
    }

    #[tokio::test]
    async fn iframe_serve_returns_css_with_correct_content_type() {
        let (router, _state, wid, pid, aid, _tmp) =
            super::super::test_support::setup_hierarchical_extension(
                b"<html><body>memo</body></html>",
            )
            .await;
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!(
                        "/windows/{wid}/panes/{pid}/activities/{aid}/iframe/style.css"
                    ))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let ct = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(ct.starts_with("text/css"));
    }

    #[tokio::test]
    async fn iframe_serve_returns_404_for_missing_file() {
        let (router, _state, wid, pid, aid, _tmp) =
            super::super::test_support::setup_hierarchical_extension(b"<html></html>").await;
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!(
                        "/windows/{wid}/panes/{pid}/activities/{aid}/iframe/missing.html"
                    ))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn iframe_serve_blocks_path_traversal() {
        let (router, _state, wid, pid, aid, tmp) =
            super::super::test_support::setup_hierarchical_extension(b"<html></html>").await;
        let outside = tmp.path().parent().unwrap().join("outside.txt");
        std::fs::write(&outside, b"secret").ok();
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!(
                        "/windows/{wid}/panes/{pid}/activities/{aid}/iframe/../outside.txt"
                    ))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(matches!(
            resp.status(),
            axum::http::StatusCode::BAD_REQUEST | axum::http::StatusCode::NOT_FOUND
        ));
        let _ = std::fs::remove_file(outside);
    }

    #[tokio::test]
    async fn iframe_for_memo_extension_returns_visible_content() {
        // Resolve the real extensions/memo/ path so this test verifies the
        // file shipped in the repo (not a tempdir copy). CARGO_MANIFEST_DIR
        // is daemon/http_server, so ../.. lands at the workspace root and
        // extensions/memo is just below it.
        let memo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../extensions/memo")
            .canonicalize()
            .expect("extensions/memo must exist relative to daemon/http_server");

        let state = test_helpers::fresh_state();
        let (_sid, wid, pid, _initial_aid) = test_helpers::bootstrap_default(&state).await;
        state.extensions.register("memo", &memo_root);
        let activity = Activity::extension(ActivityId::new(), "ext", memo_root.clone());
        let aid = activity.id.clone();
        state
            .multiplexer
            .with_window_or_404(&wid, |w| w.pane_mut(&pid)?.add_activity(activity))
            .await
            .unwrap();
        state.extensions.record_activity_owner(&aid, "memo");
        let (router, _) = test_helpers::router_with(state.clone());

        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!(
                        "/windows/{wid}/panes/{pid}/activities/{aid}/iframe/index.html"
                    ))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let ct = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(ct.starts_with("text/html"), "expected text/html, got {ct}");

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_str = std::str::from_utf8(&body).unwrap();
        assert!(
            body_str.contains("Memo"),
            "expected memo HTML to contain visible 'Memo' heading, got: {body_str}"
        );
    }

    #[tokio::test]
    async fn iframe_html_contains_ozmux_globals_script() {
        let (router, _state, wid, pid, aid, _tmp) =
            super::super::test_support::setup_hierarchical_extension(
                b"<html><head></head><body>memo</body></html>",
            )
            .await;
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!(
                        "/windows/{wid}/panes/{pid}/activities/{aid}/iframe/index.html"
                    ))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_str = std::str::from_utf8(&body).unwrap();
        assert!(
            body_str.contains("window.__OZMUX__"),
            "expected globals script, got: {body_str}"
        );
        // The injected payload must use camelCase keys with the IDs that came
        // off the URL — that is the contract the iframe SDK depends on.
        assert!(
            body_str.contains(&format!("\"windowId\":\"{wid}\"")),
            "wid: {body_str}"
        );
        assert!(
            body_str.contains(&format!("\"paneId\":\"{pid}\"")),
            "pid: {body_str}"
        );
        assert!(
            body_str.contains(&format!("\"activityId\":\"{aid}\"")),
            "aid: {body_str}"
        );
        // The injection must land inside <head> so it runs before any user
        // script tag that appears later in the document.
        let head_pos = body_str.find("<head>").unwrap();
        let script_pos = body_str.find("window.__OZMUX__").unwrap();
        let body_tag = body_str.find("<body>").unwrap();
        assert!(head_pos < script_pos && script_pos < body_tag);
    }

    #[tokio::test]
    async fn iframe_injection_falls_back_to_html_when_head_missing() {
        let (router, _state, wid, pid, aid, _tmp) =
            super::super::test_support::setup_hierarchical_extension(
                b"<html lang=\"en\"><body>no head</body></html>",
            )
            .await;
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!(
                        "/windows/{wid}/panes/{pid}/activities/{aid}/iframe/index.html"
                    ))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_str = std::str::from_utf8(&body).unwrap();
        assert!(body_str.contains("window.__OZMUX__"));
        let html_open_end = body_str.find('>').unwrap();
        let script_pos = body_str.find("window.__OZMUX__").unwrap();
        assert!(html_open_end < script_pos);
    }

    #[tokio::test]
    async fn iframe_injection_skips_non_html_assets() {
        let (router, _state, wid, pid, aid, _tmp) =
            super::super::test_support::setup_hierarchical_extension(b"<html><head></head></html>")
                .await;
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!(
                        "/windows/{wid}/panes/{pid}/activities/{aid}/iframe/style.css"
                    ))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(!std::str::from_utf8(&body).unwrap().contains("__OZMUX__"));
    }

    #[tokio::test]
    async fn iframe_rejects_mismatched_window() {
        let (router, _state, _wid, pid, aid, _tmp) =
            super::super::test_support::setup_hierarchical_extension(b"<html><head></head></html>")
                .await;
        let phantom_wid = WindowId::new();
        let resp = router
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!(
                        "/windows/{phantom_wid}/panes/{pid}/activities/{aid}/iframe/index.html"
                    ))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // pane_owner_window says (pid → real wid) which mismatches phantom_wid.
        assert_eq!(resp.status(), axum::http::StatusCode::CONFLICT);
    }
}
