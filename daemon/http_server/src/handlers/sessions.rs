use crate::AppState;
use axum::{Router, routing::get};

mod create;
mod delete;
mod events;
mod fetch;
mod list;
mod rename;
mod tree;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list::list).post(create::create))
        .route("/tree", get(tree::tree))
        .nest("/{session_id}", session_id_router())
}

fn session_id_router() -> Router<AppState> {
    Router::new()
        .route(
            "/",
            get(fetch::get).patch(rename::rename).delete(delete::delete),
        )
        .route("/events", get(events::events))
}
