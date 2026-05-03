use crate::session::SessionState;
use axum::Router;

pub fn router() -> Router<SessionState> {
    Router::new()
}
