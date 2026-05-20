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
use ozmux_browser_cef_protocol::wire::{BrowserExtraContext, BrowserProfileWire, BrowserRole};
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
                state, wid, pid, aid, None,
            )
            .await
        }
        ActivityKind::Extension { .. } => {
            provision_extension_cef(state, wid, pid, aid).await
        }
        ActivityKind::Browser {
            initial_url,
            profile,
        } => {
            if state.cef_host.is_dead() {
                return Err(HttpError::BrowserUnavailable(
                    BrowserUnavailableReason::RetryExhausted {
                        last_error: "cef_host previously crashed".into(),
                    },
                ));
            }
            let backend = CefBackend {
                dispatcher: Arc::clone(&state.cef_host),
                registry: Arc::clone(&state.browser_cef),
            };
            let cef_aid = CefActivityId(aid.to_string());
            let url = initial_url.as_deref().unwrap_or("about:blank");
            let session_id =
                crate::handlers::windows::panes::session_owning_window(state, wid).await;
            let context = BrowserExtraContext {
                role: BrowserRole::Browser,
                session_id: session_id.map(|s| s.to_string()),
                window_id: wid.to_string(),
                pane_id: pid.to_string(),
                activity_id: aid.to_string(),
                extension_name: None,
            };
            backend
                .provision(&cef_aid, url, browser_profile_to_wire(profile), context)
                .await
                .map_err(|e| HttpError::Internal(format!("cef browser provision failed: {e}")))?;
            Ok(())
        }
    }
}

/// Spawns the embedded CEF browser that backs an Extension Activity.
///
/// The browser is created with `role == Extension` and an initial
/// `ozmux-ext://<extension>/index.html` URL; the `OzmuxClient`'s
/// scheme-handler-factory serves the bytes out of the extension's
/// `launch_dir`, and the render process's `on_context_created` installs
/// `window.ozmux` because the role gate (see `render_process.rs`) is
/// satisfied.
async fn provision_extension_cef(
    state: &AppState,
    wid: &WindowId,
    pid: &PaneId,
    aid: &ActivityId,
) -> HttpResult<()> {
    if state.cef_host.is_dead() {
        return Err(HttpError::BrowserUnavailable(
            BrowserUnavailableReason::RetryExhausted {
                last_error: "cef_host previously crashed".into(),
            },
        ));
    }
    let extension_name = state.extensions.activity_owner(aid).ok_or_else(|| {
        HttpError::Internal(format!(
            "extension activity {aid} has no registered owner; cannot resolve ozmux-ext host",
        ))
    })?;
    let backend = CefBackend {
        dispatcher: Arc::clone(&state.cef_host),
        registry: Arc::clone(&state.browser_cef),
    };
    let cef_aid = CefActivityId(aid.to_string());
    let session_id = crate::handlers::windows::panes::session_owning_window(state, wid).await;
    let context = BrowserExtraContext {
        role: BrowserRole::Extension,
        session_id: session_id.map(|s| s.to_string()),
        window_id: wid.to_string(),
        pane_id: pid.to_string(),
        activity_id: aid.to_string(),
        extension_name: Some(extension_name.clone()),
    };
    let initial_url = format!("ozmux-ext://{extension_name}/index.html");
    backend
        .provision(&cef_aid, &initial_url, BrowserProfileWire::Incognito, context)
        .await
        .map_err(|e| HttpError::Internal(format!("cef extension provision failed: {e}")))?;
    Ok(())
}

/// Converts a multiplexer profile variant to its wire-protocol equivalent
/// across the crate boundary.
fn browser_profile_to_wire(profile: &BrowserProfile) -> BrowserProfileWire {
    match profile {
        BrowserProfile::Named { name } => BrowserProfileWire::Named { name: name.clone() },
        BrowserProfile::Incognito => BrowserProfileWire::Incognito,
    }
}
