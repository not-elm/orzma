//! HTTP-layer error type and axum IntoResponse mapping.

use crate::state::ActivityKindDiscriminant;
use ozmux_browser::BrowserUnavailableReason;
use ozmux_multiplexer::{ActivityId, MultiplexerError};
use ozmux_terminal::TerminalError;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum HttpError {
    #[error("failed to launch daemon http server: {0}")]
    FailedLaunch(String),

    #[error(transparent)]
    Session(#[from] MultiplexerError),

    #[error(transparent)]
    Terminal(#[from] TerminalError),

    #[error("X-Ozmux-Extension header missing")]
    MissingExtensionHeader,

    #[error("unknown extension: {0}")]
    UnknownExtension(String),

    #[error("activity not owned by caller")]
    ActivityNotOwned,

    #[error("pane not owned by caller")]
    PaneNotOwned,

    #[error("forbidden: {0}")]
    Forbidden(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("service unavailable: {0}")]
    ServiceUnavailable(String),

    /// An unrecoverable server-side error with a free-form message.
    #[error("internal error: {0}")]
    Internal(String),

    /// The request's `Origin` header is not in the allowlist.
    #[error("forbidden origin")]
    ForbiddenOrigin,

    /// The resolved activity's kind does not match what the route requires.
    #[error("activity {aid} kind mismatch: want {want:?}, got {got:?}")]
    ActivityKindMismatch {
        /// Id of the activity whose kind was checked.
        aid: ActivityId,
        /// The kind the route expected.
        want: ActivityKindDiscriminant,
        /// The kind the activity actually has.
        got: ActivityKindDiscriminant,
    },

    /// The browser backend is unavailable (e.g. retries exhausted).
    #[error("browser unavailable: {0:?}")]
    BrowserUnavailable(BrowserUnavailableReason),

    #[error("invalid dimensions: {field} must be >= 1")]
    InvalidDimensions { field: &'static str },

    #[error("invalid amount: must be >= 1")]
    InvalidAmount,
}

pub type HttpResult<T = ()> = Result<T, HttpError>;

impl axum::response::IntoResponse for HttpError {
    fn into_response(self) -> axum::response::Response {
        use axum::http::StatusCode;
        if let HttpError::BrowserUnavailable(reason) = &self {
            let body = serde_json::json!({
                "error": { "code": "BROWSER_UNAVAILABLE", "message": self.to_string() },
                "reason": reason,
            });
            return (StatusCode::SERVICE_UNAVAILABLE, axum::Json(body)).into_response();
        }
        let (status, code) = match &self {
            HttpError::Session(MultiplexerError::SessionNotFound(_)) => {
                (StatusCode::NOT_FOUND, "SESSION_NOT_FOUND")
            }
            HttpError::Session(MultiplexerError::WindowNotFound(_))
            | HttpError::Session(MultiplexerError::WindowDoesNotBelongToSession { .. }) => {
                (StatusCode::NOT_FOUND, "WINDOW_NOT_FOUND")
            }
            HttpError::Session(MultiplexerError::PaneNotFound(_))
            | HttpError::Session(MultiplexerError::CellForPaneNotFound(_)) => {
                (StatusCode::NOT_FOUND, "PANE_NOT_FOUND")
            }
            HttpError::Session(MultiplexerError::PaneNotInWindow { .. }) => {
                (StatusCode::CONFLICT, "PANE_NOT_IN_WINDOW")
            }
            HttpError::Session(MultiplexerError::WindowNotMeasured(_)) => {
                (StatusCode::CONFLICT, "WINDOW_NOT_MEASURED")
            }
            HttpError::Session(MultiplexerError::CellNotFound(_)) => {
                (StatusCode::NOT_FOUND, "CELL_NOT_FOUND")
            }
            HttpError::Session(MultiplexerError::InvalidCellType(_)) => {
                (StatusCode::BAD_REQUEST, "INVALID_CELL_TYPE")
            }
            HttpError::Session(MultiplexerError::CannotCloseLastWindow(_)) => {
                (StatusCode::CONFLICT, "CANNOT_CLOSE_LAST_WINDOW")
            }
            HttpError::Session(MultiplexerError::CannotCloseLastPane(_))
            | HttpError::Session(MultiplexerError::CannotCloseLastPaneInWindow(_)) => {
                (StatusCode::CONFLICT, "CANNOT_CLOSE_LAST_PANE")
            }
            HttpError::Session(MultiplexerError::WindowNotAttachedToSession(_)) => {
                (StatusCode::CONFLICT, "WINDOW_NOT_ATTACHED")
            }
            HttpError::Terminal(TerminalError::ActivityNotFound(_)) => {
                (StatusCode::NOT_FOUND, "ACTIVITY_NOT_FOUND")
            }
            HttpError::MissingExtensionHeader => {
                (StatusCode::UNAUTHORIZED, "MISSING_EXTENSION_HEADER")
            }
            HttpError::UnknownExtension(_) => (StatusCode::FORBIDDEN, "UNKNOWN_EXTENSION"),
            HttpError::ActivityNotOwned => (StatusCode::FORBIDDEN, "ACTIVITY_NOT_OWNED"),
            HttpError::PaneNotOwned => (StatusCode::FORBIDDEN, "PANE_NOT_OWNED"),
            HttpError::Forbidden(_) => (StatusCode::FORBIDDEN, "FORBIDDEN"),
            HttpError::NotFound(_) => (StatusCode::NOT_FOUND, "NOT_FOUND"),
            HttpError::ServiceUnavailable(_) => {
                (StatusCode::SERVICE_UNAVAILABLE, "SERVICE_UNAVAILABLE")
            }
            HttpError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL"),
            HttpError::InvalidDimensions { .. } => {
                (StatusCode::UNPROCESSABLE_ENTITY, "INVALID_DIMENSIONS")
            }
            HttpError::InvalidAmount => (StatusCode::UNPROCESSABLE_ENTITY, "INVALID_AMOUNT"),
            HttpError::Session(MultiplexerError::ActivityNotFound(_))
            | HttpError::Session(MultiplexerError::ActivityNotInPane { .. }) => {
                (StatusCode::NOT_FOUND, "ACTIVITY_NOT_FOUND")
            }
            HttpError::Session(MultiplexerError::ActivityIdConflict(_)) => {
                (StatusCode::CONFLICT, "ACTIVITY_ID_CONFLICT")
            }
            HttpError::Session(MultiplexerError::PaneIdConflict(_)) => {
                (StatusCode::CONFLICT, "PANE_ID_CONFLICT")
            }
            HttpError::Session(MultiplexerError::PaneAlreadyPlaced(_)) => {
                (StatusCode::CONFLICT, "PANE_ALREADY_PLACED")
            }
            HttpError::Session(MultiplexerError::CannotRemoveLastActivity(_)) => {
                (StatusCode::CONFLICT, "CANNOT_REMOVE_LAST_ACTIVITY")
            }
            HttpError::ForbiddenOrigin => (StatusCode::FORBIDDEN, "FORBIDDEN_ORIGIN"),
            HttpError::ActivityKindMismatch { .. } => {
                (StatusCode::CONFLICT, "ACTIVITY_KIND_MISMATCH")
            }
            HttpError::BrowserUnavailable(_) => unreachable!("handled by early return above"),
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
    use ozmux_multiplexer::MultiplexerError;
    use ozmux_multiplexer::{ActivityId, CellId, PaneId, SessionId, WindowId};
    use ozmux_terminal::TerminalError;

    #[tokio::test]
    async fn session_not_found_maps_to_404_with_code() {
        let err = HttpError::Session(MultiplexerError::SessionNotFound(SessionId::new()));
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["code"].as_str(), Some("SESSION_NOT_FOUND"));
    }

    #[test]
    fn window_not_found_maps_to_404() {
        let err = HttpError::Session(MultiplexerError::WindowNotFound(WindowId::new()));
        assert_eq!(err.into_response().status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn pane_not_found_maps_to_404() {
        let err = HttpError::Session(MultiplexerError::PaneNotFound(PaneId::new()));
        assert_eq!(err.into_response().status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn cell_for_pane_not_found_maps_to_404_pane_not_found() {
        let err = HttpError::Session(MultiplexerError::CellForPaneNotFound(PaneId::new()));
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn cannot_close_last_pane_maps_to_409() {
        let err = HttpError::Session(MultiplexerError::CannotCloseLastPane(CellId::new()));
        assert_eq!(err.into_response().status(), StatusCode::CONFLICT);
    }

    #[test]
    fn window_not_attached_maps_to_409_window_not_attached() {
        let err = HttpError::Session(MultiplexerError::WindowNotAttachedToSession(WindowId::new()));
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[test]
    fn invalid_cell_type_maps_to_400() {
        let err = HttpError::Session(MultiplexerError::InvalidCellType(CellId::new()));
        assert_eq!(err.into_response().status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn split_target_equals_new_cell_falls_through_to_500() {
        let err = HttpError::Session(MultiplexerError::SplitTargetEqualsNewCell(CellId::new()));
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

    #[test]
    fn missing_extension_header_maps_to_401() {
        let err = HttpError::MissingExtensionHeader;
        assert_eq!(err.into_response().status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn unknown_extension_maps_to_403() {
        let err = HttpError::UnknownExtension("ghost".into());
        assert_eq!(err.into_response().status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn activity_not_owned_maps_to_403() {
        let err = HttpError::ActivityNotOwned;
        assert_eq!(err.into_response().status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn pane_not_owned_maps_to_403() {
        let err = HttpError::PaneNotOwned;
        assert_eq!(err.into_response().status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn pane_already_placed_maps_to_409() {
        let err = HttpError::Session(MultiplexerError::PaneAlreadyPlaced(PaneId::new()));
        assert_eq!(err.into_response().status(), StatusCode::CONFLICT);
    }

    #[test]
    fn pane_id_conflict_maps_to_409() {
        let err = HttpError::Session(MultiplexerError::PaneIdConflict(PaneId::new()));
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[test]
    fn activity_id_conflict_maps_to_409() {
        let err = HttpError::Session(MultiplexerError::ActivityIdConflict(ActivityId::new()));
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[test]
    fn activity_not_found_session_maps_to_404() {
        let err = HttpError::Session(MultiplexerError::ActivityNotFound(ActivityId::new()));
        assert_eq!(err.into_response().status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn pane_not_in_window_maps_to_409() {
        let err = HttpError::Session(MultiplexerError::PaneNotInWindow {
            window: WindowId::new(),
            pane: PaneId::new(),
        });
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[test]
    fn cannot_remove_last_activity_maps_to_409() {
        let err = HttpError::Session(MultiplexerError::CannotRemoveLastActivity(PaneId::new()));
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn browser_unavailable_maps_to_503() {
        let err = HttpError::BrowserUnavailable(BrowserUnavailableReason::RetryExhausted {
            last_error: "boom".into(),
        });
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["code"].as_str(), Some("BROWSER_UNAVAILABLE"));
        assert_eq!(v["reason"]["kind"].as_str(), Some("retry_exhausted"));
    }

    #[test]
    fn window_not_measured_maps_to_409_window_not_measured() {
        let err = HttpError::Session(MultiplexerError::WindowNotMeasured(WindowId::new()));
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[test]
    fn invalid_dimensions_maps_to_422_invalid_dimensions() {
        let err = HttpError::InvalidDimensions { field: "cols" };
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[test]
    fn invalid_amount_maps_to_422() {
        let err = HttpError::InvalidAmount;
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
}
