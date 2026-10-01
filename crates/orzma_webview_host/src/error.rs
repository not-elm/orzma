//! The error type the webview host reports, and the result alias built on
//! it.

use getrandom::Error as CsprngError;
use serde_json::Error as JsonError;
use std::io::Error as IoError;
use thiserror::Error;

/// A `Result` whose error is [`WebviewHostError`].
pub type WebviewHostResult<T = ()> = Result<T, WebviewHostError>;

/// Every failure the webview host reports.
#[derive(Debug, Error)]
pub enum WebviewHostError {
    /// No directory could hold the control socket.
    #[error(transparent)]
    RuntimeRoot(#[from] RuntimeRootError),
    /// A `register` payload failed validation.
    #[error(transparent)]
    Register(#[from] RegisterError),
    /// The host turned down a request.
    #[error(transparent)]
    Refused(#[from] Refusal),
    /// The OS random source failed.
    #[error("the OS random source failed: {0}")]
    Csprng(CsprngError),
    /// A freshly minted id matched a live one, so the random source is not
    /// producing unique values.
    #[error("a freshly minted id collided with a live one")]
    DuplicateId,
    /// A socket or file operation failed.
    #[error(transparent)]
    Io(#[from] IoError),
    /// An outbound line failed to serialize.
    #[error(transparent)]
    Json(#[from] JsonError),
}

impl WebviewHostError {
    /// The short code a control-socket reply carries for this failure: the
    /// code of a validation failure or a refusal, and `internal` for a
    /// failure of the host itself.
    pub fn wire_code(&self) -> &'static str {
        match self {
            Self::Register(error) => error.wire_code(),
            Self::Refused(refusal) => refusal.wire_code(),
            Self::RuntimeRoot(_)
            | Self::Csprng(_)
            | Self::DuplicateId
            | Self::Io(_)
            | Self::Json(_) => "internal",
        }
    }

    /// Whether this failure is the host turning down a request, as opposed
    /// to the host itself failing.
    pub fn is_refusal(&self) -> bool {
        matches!(self, Self::Register(_) | Self::Refused(_))
    }
}

/// The reason no directory could hold the control socket.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RuntimeRootError {
    /// The longest socket path under every candidate directory overflows
    /// `sun_path`.
    #[error("'{name}' socket path exceeds {limit} bytes")]
    SocketPathTooLong {
        /// The name whose socket path overflowed.
        name: String,
        /// The `sun_path` byte limit that was exceeded.
        limit: usize,
    },
}

/// The reason a `register` payload is invalid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum RegisterError {
    /// A `dir` root that is relative or is not a directory.
    #[error("the root is not an absolute directory")]
    InvalidRoot,
    /// A `dir` entry that is empty or holds `..`, `.`, or a root.
    #[error("the entry is not a relative path of normal components")]
    UnsafeEntry,
    /// An `inline` document larger than 4 MiB.
    #[error("the inline HTML is larger than 4 MiB")]
    HtmlTooLarge,
    /// A `url` that does not parse or has no host.
    #[error("the URL does not parse or has no host")]
    InvalidUrl,
    /// A `url` whose scheme is not `http` or `https`.
    #[error("the URL scheme is not http or https")]
    UnsupportedScheme,
}

impl RegisterError {
    /// The short code a `register` reply carries for this failure.
    pub fn wire_code(self) -> &'static str {
        match self {
            Self::InvalidRoot => "invalid_root",
            Self::UnsafeEntry => "unsafe_entry",
            Self::HtmlTooLarge => "html_too_large",
            Self::InvalidUrl => "invalid_url",
            Self::UnsupportedScheme => "unsupported_scheme",
        }
    }
}

/// A request the host turned down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum Refusal {
    /// The pane the connection belongs to has closed.
    #[error("the connection's pane has closed")]
    OwnerGone,
    /// Another connection owns the handle or instance.
    #[error("another connection owns the target")]
    NotOwner,
    /// No live registration carries the handle.
    #[error("no registration carries the handle")]
    UnknownHandle,
    /// No live registration minted the instance.
    #[error("no registration minted the instance")]
    UnknownInstance,
    /// The instance is not spelled as 32 hex digits.
    #[error("the instance is not 32 hex digits")]
    MalformedInstance,
    /// The placement is not mounted.
    #[error("the placement is not mounted")]
    NotMounted,
    /// The mount has ended: its placement was unmounted, or mounted again
    /// under a newer mount.
    #[error("the mount has ended")]
    StaleMount,
    /// The registration does not accept pointer or keyboard input.
    #[error("the placement does not accept input")]
    NotInteractive,
    /// A mount size outside `1..=PlacementSize::MAX_ROWS` rows or
    /// `1..=PlacementSize::MAX_COLS` columns.
    #[error("the mount size is out of range")]
    SizeOutOfRange,
    /// The registration has no `window.orzma` bridge.
    #[error("the registration has no window.orzma bridge")]
    NotBridged,
    /// The registration does not load a remote `http(s)` URL.
    #[error("the registration does not load a remote URL")]
    NotUrlView,
    /// A navigation target that is not a valid `http(s)` URL.
    #[error("the navigation target is not a valid http(s) URL")]
    InvalidNavigation,
    /// A page event with an empty name.
    #[error("the event name is empty")]
    EmptyEventName,
    /// The connection is closed or never completed its `hello`.
    #[error("the connection is closed")]
    ConnectionClosed,
    /// The placement's pane is not on screen.
    #[error("the placement's pane is not on screen")]
    PaneHidden,
}

impl Refusal {
    /// The short code a reply carries for this refusal: `owner_gone`,
    /// `not_owner`, or `unknown_handle`, and `internal` for a refusal no
    /// reply names.
    pub fn wire_code(self) -> &'static str {
        match self {
            Self::OwnerGone => "owner_gone",
            Self::NotOwner => "not_owner",
            Self::UnknownHandle => "unknown_handle",
            _ => "internal",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts that every failure a reply names maps to its documented wire
    /// code, and that a failure of the host itself maps to `internal`.
    ///
    /// Case: a program registers views that fail each validation check,
    /// asks for placements of handles it does not own, and hits a broken
    /// random source.
    #[test]
    fn each_failure_maps_to_its_wire_code() {
        let cases: [(WebviewHostError, &str); 10] = [
            (Refusal::OwnerGone.into(), "owner_gone"),
            (Refusal::NotOwner.into(), "not_owner"),
            (Refusal::UnknownHandle.into(), "unknown_handle"),
            (RegisterError::InvalidRoot.into(), "invalid_root"),
            (RegisterError::UnsafeEntry.into(), "unsafe_entry"),
            (RegisterError::HtmlTooLarge.into(), "html_too_large"),
            (RegisterError::InvalidUrl.into(), "invalid_url"),
            (
                RegisterError::UnsupportedScheme.into(),
                "unsupported_scheme",
            ),
            (WebviewHostError::DuplicateId, "internal"),
            (Refusal::StaleMount.into(), "internal"),
        ];
        for (error, code) in cases {
            assert_eq!(error.wire_code(), code, "{error}");
        }
    }

    /// Asserts that validation failures and refusals count as refusals,
    /// while I/O and id failures do not.
    ///
    /// Case: the orzmux loop decides whether a failed request is logged at
    /// debug or at warn.
    #[test]
    fn only_refusals_and_validation_failures_are_refusals() {
        assert!(WebviewHostError::from(Refusal::StaleMount).is_refusal());
        assert!(WebviewHostError::from(RegisterError::InvalidUrl).is_refusal());
        assert!(!WebviewHostError::from(IoError::other("gone")).is_refusal());
        assert!(!WebviewHostError::DuplicateId.is_refusal());
    }

    /// Asserts that a socket path overflow names the socket and the limit.
    ///
    /// Case: orzma starts from a temp directory so deep that its control
    /// socket path cannot fit.
    #[test]
    fn a_socket_path_overflow_names_the_socket_and_the_limit() {
        let error = WebviewHostError::from(RuntimeRootError::SocketPathTooLong {
            name: "control".into(),
            limit: 104,
        });
        assert_eq!(error.to_string(), "'control' socket path exceeds 104 bytes");
    }
}
