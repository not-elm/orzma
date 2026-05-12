pub mod health;
pub mod index;
pub mod sessions;
pub mod windows;

use crate::{AppState, HttpError, HttpResult};
use ozmux_multiplexer::{Activity, ActivityId, MultiplexerError, PaneId, WindowId};

/// Build the current Window layout snapshot under the Window lock and
/// broadcast it. Used by every handler that mutates a Window.
pub(crate) async fn publish_window_layout(state: &AppState, wid: &WindowId) {
    let _ = state
        .with_window(wid, |w| match windows::window_view_for(w) {
            Ok(view) => state.layout_broadcast.publish(wid, view),
            Err(e) => tracing::warn!(error = %e, %wid, "skipped layout publish"),
        })
        .await;
}

/// Validate that `pid` lives inside `wid`. Returns `PaneNotFound` when the
/// pane has no owner and `PaneNotInWindow` when it lives in a different
/// Window. Used by every URL of shape `/windows/:wid/panes/:pid/*`.
pub(crate) fn ensure_pane_in_window(
    state: &AppState,
    wid: &WindowId,
    pid: &PaneId,
) -> HttpResult<()> {
    let actual = state.lookup_pane_window(pid)?;
    if &actual != wid {
        return Err(HttpError::Session(MultiplexerError::PaneNotInWindow {
            window: wid.clone(),
            pane: pid.clone(),
        }));
    }
    Ok(())
}

/// Combined membership check for `/windows/:wid/panes/:pid/activities/:aid/*`
/// that also returns the resolved `Activity`. Callers like `iframe_serve`
/// need both the validation and the activity metadata; doing them in one
/// helper avoids a second Window lock acquisition.
pub(crate) async fn ensure_activity_in_pane_in_window_and_fetch(
    state: &AppState,
    wid: &WindowId,
    pid: &PaneId,
    aid: &ActivityId,
) -> HttpResult<Activity> {
    ensure_pane_in_window(state, wid, pid)?;
    let activity = state
        .with_window(wid, |w| w.pane(pid).map(|p| p.activity(aid).cloned()))
        .await
        .ok_or_else(|| HttpError::Session(MultiplexerError::WindowNotFound(wid.clone())))??
        .ok_or_else(|| {
            HttpError::Session(MultiplexerError::ActivityNotInPane {
                pane: pid.clone(),
                activity: aid.clone(),
            })
        })?;
    Ok(activity)
}

/// Membership-only variant for handlers that don't need the Activity payload
/// (terminal WS, handlers WS).
pub(crate) async fn ensure_activity_in_pane_in_window(
    state: &AppState,
    wid: &WindowId,
    pid: &PaneId,
    aid: &ActivityId,
) -> HttpResult<()> {
    let _ = ensure_activity_in_pane_in_window_and_fetch(state, wid, pid, aid).await?;
    Ok(())
}
