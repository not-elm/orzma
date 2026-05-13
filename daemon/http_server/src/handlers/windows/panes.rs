use crate::AppState;
use ozmux_multiplexer::{SessionId, WindowId};

pub mod activate;
pub mod activities;
pub mod close;
pub mod split;

/// Walk the SessionState to find which Session owns `wid`. Used to populate
/// `OZMUX_SESSION_ID` for the spawned PTY; returns `None` for orphan Windows.
pub(crate) async fn session_owning_window(state: &AppState, wid: &WindowId) -> Option<SessionId> {
    let sess = state.multiplexer.sessions.lock().await;
    sess.iter()
        .find(|(_, s)| s.linked_windows.contains(wid))
        .map(|(id, _)| id.clone())
}
