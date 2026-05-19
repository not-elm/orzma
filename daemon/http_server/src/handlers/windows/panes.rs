use crate::AppState;
use axum::{
    Router,
    routing::{delete as method_delete, post},
};
use ozmux_multiplexer::{SessionId, WindowId};

pub mod activate;
pub mod activities;
pub mod close;
pub mod cycle_activity;
pub mod resize;
pub mod spawn_terminal;
pub mod split;
pub mod swap;

pub fn router() -> Router<AppState> {
    Router::new().nest("/{pane_id}", pane_id_router())
}

fn pane_id_router() -> Router<AppState> {
    Router::new()
        .route("/", method_delete(close::close))
        .route("/activate", post(activate::activate))
        .route("/cycle-activity", post(cycle_activity::cycle_activity))
        .route("/split", post(split::split))
        .route("/resize", post(resize::resize))
        .route("/swap", post(swap::swap))
        .nest("/activities", activities::router())
}

/// Walk the SessionState to find which Session owns `wid`. Used to populate
/// `OZMUX_SESSION_ID` for the spawned PTY; returns `None` for orphan Windows.
async fn session_owning_window(state: &AppState, wid: &WindowId) -> Option<SessionId> {
    let sess = state.multiplexer.sessions.lock().await;
    sess.iter()
        .find(|(_, s)| s.linked_windows.contains(wid))
        .map(|(id, _)| id.clone())
}
