//! Two-phase activity provisioning: dispatch on `ActivityKind` after the
//! multiplexer has accepted the new activity. On failure the caller rolls
//! back the multiplexer mutation before publishing the layout.
//!
//! The dispatch is intentionally *missing-ok* for kinds without external
//! runtime resources, so future kinds can be added by extending the
//! `ActivityKind` enum and adding one arm here.

use crate::AppState;
use crate::error::{HttpError, HttpResult};
use ozmux_browser::BrowserUnavailableReason;
use ozmux_browser::cef_backend::CefBackend;
use ozmux_browser_cef_protocol::types::ActivityId as CefActivityId;
use ozmux_multiplexer::{ActivityId, ActivityKind, PaneId, WindowId};
use std::sync::Arc;

/// Spin up whatever runtime resource the given activity kind needs.
/// Called by `add_activity_to_pane` and `split_pane` immediately after the
/// multiplexer accepts the new activity, before publishing the window layout.
/// Returns Err if the runtime cannot be provisioned; the caller MUST roll
/// back the multiplexer mutation before propagating the error.
pub(crate) async fn provision_activity_runtime(
    state: &AppState,
    wid: &WindowId,
    pid: &PaneId,
    aid: &ActivityId,
    kind: &ActivityKind,
) -> HttpResult<()> {
    match kind {
        ActivityKind::Terminal => {
            crate::handlers::windows::panes::spawn_terminal::spawn_terminal_pty(
                state, wid, pid, aid,
            )
            .await
        }
        ActivityKind::Extension { .. } => Ok(()),
        ActivityKind::Browser { initial_url } => {
            if state.cef_host.is_dead() {
                return Err(HttpError::CefHostDead(
                    BrowserUnavailableReason::RetryExhausted {
                        last_error: "cef_host previously crashed".into(),
                    },
                ));
            }
            state
                .browser
                .spawn(wid, pid, aid, initial_url.clone())
                .await
                .map_err(|e| HttpError::Internal(format!("browser spawn failed: {e}")))?;

            // NOTE: Phase A/B dual-provision — cef path runs alongside the
            // chromiumoxide path so `?cef=1` in the frontend can render via the
            // new pipeline. Phase C deletes the chromiumoxide branch entirely.
            let backend = CefBackend {
                handles: Arc::clone(&state.cef_host),
                registry: Arc::clone(&state.browser_cef),
            };
            let cef_aid = CefActivityId(aid.to_string());
            let url = initial_url.as_deref().unwrap_or("about:blank");
            // NOTE: real cookies wired in Plan 2 Task B12; empty Vec is correct
            // for Phase A — cookie harvesting in cef_backend is deferred.
            if let Err(e) = backend.provision(&cef_aid, url, Vec::new()).await {
                tracing::warn!(?aid, error = %e, "cef provisioning failed; chromiumoxide path continues");
            }
            Ok(())
        }
    }
}
