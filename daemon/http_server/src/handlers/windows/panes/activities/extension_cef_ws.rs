//! `/windows/{wid}/panes/{pid}/activities/{aid}/extension/cef/ws` — Extension
//! screencast over WebSocket. Delegates to the shared cef_screencast helper.

use super::cef_screencast::cef_screencast_ws;
use crate::AppState;
use crate::error::HttpError;
use crate::state::ActivityKindDiscriminant;
use axum::extract::{Path, State};
use axum::response::Response;
use ozmux_multiplexer::{ActivityId, PaneId, WindowId};

/// `GET /windows/{wid}/panes/{pid}/activities/{aid}/extension/cef/ws`
///
/// Validates origin and that the activity kind is `Extension`, then upgrades
/// to a WebSocket bound to the per-activity CEF `FrameRing`.
pub async fn extension_cef_ws(
    State(state): State<AppState>,
    Path((wid, pid, aid)): Path<(WindowId, PaneId, ActivityId)>,
    req: axum::extract::Request,
) -> Result<Response, HttpError> {
    cef_screencast_ws(
        state,
        wid,
        pid,
        aid,
        ActivityKindDiscriminant::Extension,
        req,
    )
    .await
}
