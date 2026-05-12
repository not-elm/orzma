pub mod activities;
pub mod health;
pub mod index;
pub mod panes;
pub mod sessions;
pub mod windows;

use crate::AppState;
use ozmux_multiplexer::WindowId;

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
