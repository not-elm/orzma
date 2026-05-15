//! Two-phase activity provisioning: dispatch on `ActivityKind` after the
//! multiplexer has accepted the new activity. On failure the caller rolls
//! back the multiplexer mutation before publishing the layout.
//!
//! The dispatch is intentionally *missing-ok* for kinds without external
//! runtime resources, so future kinds can be added by extending the
//! `ActivityKind` enum and adding one arm here.

use crate::AppState;
use crate::error::{HttpError, HttpResult};
use ozmux_multiplexer::{ActivityId, ActivityKind, PaneId, WindowId};

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
        ActivityKind::Browser { initial_url } => state
            .browser
            .spawn(wid, pid, aid, initial_url.clone())
            .await
            .map_err(|e| HttpError::Internal(format!("browser spawn failed: {e}"))),
    }
}
