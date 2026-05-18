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
use ozmux_browser_cef_protocol::wire::BrowserProfileWire;
use ozmux_multiplexer::{ActivityId, ActivityKind, BrowserProfile, PaneId, WindowId};
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
        ActivityKind::Browser {
            initial_url,
            profile,
        } => {
            if state.cef_host.is_dead() {
                return Err(HttpError::CefHostDead(
                    BrowserUnavailableReason::RetryExhausted {
                        last_error: "cef_host previously crashed".into(),
                    },
                ));
            }
            let backend = CefBackend {
                handles: Arc::clone(&state.cef_host),
                registry: Arc::clone(&state.browser_cef),
            };
            let cef_aid = CefActivityId(aid.to_string());
            let url = initial_url.as_deref().unwrap_or("about:blank");
            backend
                .provision(&cef_aid, url, browser_profile_to_wire(profile))
                .await
                .map_err(|e| HttpError::Internal(format!("cef browser provision failed: {e}")))?;
            Ok(())
        }
    }
}

/// Converts a multiplexer profile variant to its wire-protocol equivalent
/// across the crate boundary.
fn browser_profile_to_wire(profile: &BrowserProfile) -> BrowserProfileWire {
    match profile {
        BrowserProfile::Named { name } => BrowserProfileWire::Named { name: name.clone() },
        BrowserProfile::Incognito => BrowserProfileWire::Incognito,
    }
}
