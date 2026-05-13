use crate::AppState;
use axum::{
    Router,
    routing::{delete as method_delete, post},
};
use ozmux_multiplexer::{SessionId, WindowId};

pub mod activate;
pub mod activities;
pub mod close;
pub mod spawn_terminal;
pub mod split;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/{pane_id}/activate", post(activate::activate))
        .route("/{pane_id}/split", post(split::split))
        .route("/{pane_id}", method_delete(close::close))
        .nest("/{pane_id}/activities", activities::router())
}

/// Walk the SessionState to find which Session owns `wid`. Used to populate
/// `OZMUX_SESSION_ID` for the spawned PTY; returns `None` for orphan Windows.
pub(crate) async fn session_owning_window(state: &AppState, wid: &WindowId) -> Option<SessionId> {
    let sess = state.multiplexer.sessions.lock().await;
    sess.iter()
        .find(|(_, s)| s.linked_windows.contains(wid))
        .map(|(id, _)| id.clone())
}
