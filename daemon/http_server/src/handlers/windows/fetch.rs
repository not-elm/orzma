//! `GET /windows/{window_id}` — return the full WindowView (panes + layout).

use crate::AppState;
use crate::error::HttpResult;
use crate::window_view::WindowView;
use axum::{
    Json,
    extract::{Path, State},
};
use ozmux_multiplexer::WindowId;

pub async fn get(
    State(state): State<AppState>,
    Path(window_id): Path<WindowId>,
) -> HttpResult<Json<WindowView>> {
    let titles = state.titles.snapshot().await;
    let view = state
        .multiplexer
        .with_window_or_404(&window_id, |w| WindowView::from_window(w, &titles))
        .await?;
    Ok(Json(view))
}
