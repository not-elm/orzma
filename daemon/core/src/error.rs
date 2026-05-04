use thiserror::Error;

use crate::session::{SessionId, activity::ActivityId, cell::CellId, pane::PaneId};

pub type OzmuxResult<T = ()> = Result<T, OzmuxError>;

#[derive(Error, Debug, Clone)]
pub enum OzmuxError {
    #[error("failed to launch daemon http server:{0}")]
    FailedLaunchHttpServer(String),

    #[error("session not found session-id={0}")]
    SessionNotFound(SessionId),

    #[error("pane not found pane-id={0}")]
    PaneNotFound(PaneId),

    #[error("cell not found id={0}")]
    CellNotFound(CellId),

    #[error("invalid node type node-id={0}")]
    InvalidCellType(CellId),

    #[error("split target equals new_cell: cell-id={0}")]
    SplitTargetEqualsNewCell(CellId),

    #[error("cannot close the last pane in a session: pane-id={0}")]
    CannotCloseLastPane(PaneId),

    #[error("activity not found activity-id={0}")]
    ActivityNotFound(ActivityId),

    #[error("failed pty: {0}")]
    Pty(String),
}

pub trait PtyErrorBridge<T> {
    fn to_ozmux_result(self) -> OzmuxResult<T>;
}

impl<T> PtyErrorBridge<T> for anyhow::Result<T> {
    fn to_ozmux_result(self) -> OzmuxResult<T> {
        match self {
            Ok(t) => Ok(t),
            Err(e) => Err(OzmuxError::Pty(e.to_string())),
        }
    }
}

impl axum::response::IntoResponse for OzmuxError {
    fn into_response(self) -> axum::response::Response {
        use axum::http::StatusCode;
        let (status, code) = match &self {
            OzmuxError::SessionNotFound(_) => (StatusCode::NOT_FOUND, "SESSION_NOT_FOUND"),
            OzmuxError::PaneNotFound(_) => (StatusCode::NOT_FOUND, "PANE_NOT_FOUND"),
            OzmuxError::CellNotFound(_) => (StatusCode::NOT_FOUND, "CELL_NOT_FOUND"),
            OzmuxError::InvalidCellType(_) => (StatusCode::BAD_REQUEST, "INVALID_CELL_TYPE"),
            OzmuxError::CannotCloseLastPane(_) => (StatusCode::CONFLICT, "CANNOT_CLOSE_LAST_PANE"),
            OzmuxError::ActivityNotFound(_) => (StatusCode::NOT_FOUND, "ACTIVITY_NOT_FOUND"),
            _ => (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL"),
        };
        let body = serde_json::json!({
            "error": { "code": code, "message": self.to_string() }
        });
        (status, axum::Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionId;

    #[test]
    fn session_not_found_carries_id_in_message() {
        let id = SessionId::new();
        let err = OzmuxError::SessionNotFound(id.clone());
        assert!(err.to_string().contains(id.as_ref()));
    }

    #[tokio::test]
    async fn session_not_found_maps_to_404_with_code() {
        use crate::session::pane::PaneId;
        use axum::body::to_bytes;
        use axum::http::StatusCode;
        use axum::response::IntoResponse;

        let err = OzmuxError::SessionNotFound(crate::session::SessionId::new());
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["code"].as_str(), Some("SESSION_NOT_FOUND"));
        assert!(v["error"]["message"].is_string());

        // PaneId import keeps the use statement non-unused if you choose to extend.
        let _: Option<PaneId> = None;
    }

    #[test]
    fn cannot_close_last_pane_maps_to_409() {
        use crate::session::pane::PaneId;
        use axum::http::StatusCode;
        use axum::response::IntoResponse;

        let err = OzmuxError::CannotCloseLastPane(PaneId::new());
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[test]
    fn invalid_cell_type_maps_to_400() {
        use crate::session::cell::CellId;
        use axum::http::StatusCode;
        use axum::response::IntoResponse;

        let err = OzmuxError::InvalidCellType(CellId::new());
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn split_target_equals_new_cell_falls_through_to_500() {
        use crate::session::cell::CellId;
        use axum::http::StatusCode;
        use axum::response::IntoResponse;

        let err = OzmuxError::SplitTargetEqualsNewCell(CellId::new());
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
