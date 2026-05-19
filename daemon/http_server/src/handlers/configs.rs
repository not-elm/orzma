//! Handlers serving the loaded `OzmuxConfigs`. Read-only for now.

use crate::AppState;
use axum::{Router, routing::get};

pub mod font;
pub mod shortcuts;

/// Router for read-only config endpoints under `/configs`.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/shortcuts", get(shortcuts::get))
        .route("/font", get(font::get))
}
