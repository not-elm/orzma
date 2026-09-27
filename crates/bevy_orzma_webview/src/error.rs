//! The error type the GUI webview layer reports, and the result alias
//! built on it.

use thiserror::Error;

/// A `Result` whose error is [`WebviewError`].
pub type WebviewResult<T = ()> = Result<T, WebviewError>;

/// Every failure the GUI webview layer reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum WebviewError {
    /// An `orzma://` URL that is malformed, names no registered handle, or
    /// asks an inline document for a subresource.
    #[error("no registered asset matches the orzma:// URL")]
    UnknownAsset,
    /// No readable file exists at the requested path under the asset root.
    #[error("no file exists at the requested asset path")]
    AssetNotFound,
    /// The requested path is malformed or escapes the asset root.
    #[error("forbidden asset path")]
    AssetForbidden,
    /// The requested file is larger than 64 MiB.
    #[error("asset too large")]
    AssetTooLarge,
    /// Every overlay texture slot of the terminal is occupied.
    #[error("every overlay slot of the terminal is occupied")]
    NoFreeSlot,
}

impl WebviewError {
    /// The HTTP status an `orzma://` response carries for this failure: 404
    /// for a missing asset, 403 for a forbidden path, 413 for an oversized
    /// file, and 500 for a failure that is not an asset request's.
    pub fn http_status(self) -> u16 {
        match self {
            Self::UnknownAsset | Self::AssetNotFound => 404,
            Self::AssetForbidden => 403,
            Self::AssetTooLarge => 413,
            Self::NoFreeSlot => 500,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts that each asset failure maps to the HTTP status the
    /// `orzma://` scheme answers with.
    ///
    /// Case: a page asks for a file under an unknown handle, a missing file,
    /// a traversal path, and a file larger than the cap.
    #[test]
    fn asset_failures_map_to_their_http_status() {
        assert_eq!(WebviewError::UnknownAsset.http_status(), 404);
        assert_eq!(WebviewError::AssetNotFound.http_status(), 404);
        assert_eq!(WebviewError::AssetForbidden.http_status(), 403);
        assert_eq!(WebviewError::AssetTooLarge.http_status(), 413);
    }
}
