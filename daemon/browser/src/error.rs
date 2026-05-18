//! Crate-level error type.

use ozmux_multiplexer::ActivityId;

/// Error type for `ozmux_browser`.
#[derive(Debug, thiserror::Error)]
pub enum BrowserError {
    /// The requested Activity is not registered with `BrowserService`.
    #[error("activity not found: {0}")]
    NotFound(ActivityId),
    /// Headless Chromium failed to launch.
    #[error("chromium launch failed: {0}")]
    Launch(String),
    /// A CDP method call failed.
    #[error("cdp error: {0}")]
    Cdp(String),
    /// Cookie import from the local Chrome profile failed.
    #[error("cookie import failed: {0}")]
    Cookie(String),
    /// A filesystem or IO error occurred (e.g. creating `user-data-dir`).
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Convenience alias for `Result<T, BrowserError>`.
pub type BrowserResult<T> = Result<T, BrowserError>;
