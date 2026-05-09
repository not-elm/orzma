//! HTTP-layer error type and axum IntoResponse mapping.

use ozmux_multiplexer::SessionError;
use ozmux_terminal::TerminalError;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum HttpError {
    #[error("failed to launch daemon http server: {0}")]
    FailedLaunch(String),

    #[error(transparent)]
    Session(#[from] SessionError),

    #[error(transparent)]
    Terminal(#[from] TerminalError),
}

pub type HttpResult<T = ()> = Result<T, HttpError>;

impl axum::response::IntoResponse for HttpError {
    fn into_response(self) -> axum::response::Response {
        use axum::http::StatusCode;
        let (status, code) = match &self {
            HttpError::Session(SessionError::SessionNotFound(_)) => {
                (StatusCode::NOT_FOUND, "SESSION_NOT_FOUND")
            }
            HttpError::Session(SessionError::WindowNotFound(_))
            | HttpError::Session(SessionError::WindowDoesNotBelongToSession { .. }) => {
                (StatusCode::NOT_FOUND, "WINDOW_NOT_FOUND")
            }
            HttpError::Session(SessionError::PaneNotFound(_))
            | HttpError::Session(SessionError::CellForPaneNotFound(_)) => {
                (StatusCode::NOT_FOUND, "PANE_NOT_FOUND")
            }
            HttpError::Session(SessionError::CellNotFound(_)) => {
                (StatusCode::NOT_FOUND, "CELL_NOT_FOUND")
            }
            HttpError::Session(SessionError::InvalidCellType(_)) => {
                (StatusCode::BAD_REQUEST, "INVALID_CELL_TYPE")
            }
            HttpError::Session(SessionError::CannotCloseLastWindow(_)) => {
                (StatusCode::CONFLICT, "CANNOT_CLOSE_LAST_WINDOW")
            }
            HttpError::Session(SessionError::CannotCloseLastPane(_))
            | HttpError::Session(SessionError::CannotCloseLastPaneInWindow(_)) => {
                (StatusCode::CONFLICT, "CANNOT_CLOSE_LAST_PANE")
            }
            HttpError::Session(SessionError::WindowNotAttachedToSession(_)) => {
                (StatusCode::CONFLICT, "WINDOW_NOT_ATTACHED")
            }
            HttpError::Terminal(TerminalError::ActivityNotFound(_)) => {
                (StatusCode::NOT_FOUND, "ACTIVITY_NOT_FOUND")
            }
            // MissingParentCell, SplitTargetEqualsNewCell, ActivePaneMustBelongToWindow,
            // Terminal::Pty, FailedLaunch fall through → 500
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
    use axum::body::to_bytes;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    use ozmux_multiplexer::SessionError;
    use ozmux_multiplexer::activity::ActivityId;
    use ozmux_multiplexer::cells::CellId;
    use ozmux_multiplexer::pane::PaneId;
    use ozmux_multiplexer::session::SessionId;
    use ozmux_multiplexer::window::WindowId;
    use ozmux_terminal::TerminalError;

    #[tokio::test]
    async fn session_not_found_maps_to_404_with_code() {
        let err = HttpError::Session(SessionError::SessionNotFound(SessionId::new()));
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["code"].as_str(), Some("SESSION_NOT_FOUND"));
    }

    #[test]
    fn window_not_found_maps_to_404() {
        let err = HttpError::Session(SessionError::WindowNotFound(WindowId::new()));
        assert_eq!(err.into_response().status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn pane_not_found_maps_to_404() {
        let err = HttpError::Session(SessionError::PaneNotFound(PaneId::new()));
        assert_eq!(err.into_response().status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn cell_for_pane_not_found_maps_to_404_pane_not_found() {
        let err = HttpError::Session(SessionError::CellForPaneNotFound(PaneId::new()));
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn cannot_close_last_pane_maps_to_409() {
        let err = HttpError::Session(SessionError::CannotCloseLastPane(CellId::new()));
        assert_eq!(err.into_response().status(), StatusCode::CONFLICT);
    }

    #[test]
    fn window_not_attached_maps_to_409_window_not_attached() {
        let err = HttpError::Session(SessionError::WindowNotAttachedToSession(WindowId::new()));
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[test]
    fn invalid_cell_type_maps_to_400() {
        let err = HttpError::Session(SessionError::InvalidCellType(CellId::new()));
        assert_eq!(err.into_response().status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn split_target_equals_new_cell_falls_through_to_500() {
        let err = HttpError::Session(SessionError::SplitTargetEqualsNewCell(CellId::new()));
        assert_eq!(
            err.into_response().status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn activity_not_found_maps_to_404() {
        let err = HttpError::Terminal(TerminalError::ActivityNotFound(ActivityId::new()));
        assert_eq!(err.into_response().status(), StatusCode::NOT_FOUND);
    }
}
