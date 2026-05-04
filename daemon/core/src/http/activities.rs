use crate::session::activity::ActivityId;
use axum::extract::Path;

pub async fn terminal_ws(Path(_activity_id): Path<ActivityId>) -> axum::http::StatusCode {
    axum::http::StatusCode::NOT_IMPLEMENTED
}
