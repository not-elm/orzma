//! Error and result types for tmux control-mode parsing.

use thiserror::Error;

/// Result alias used throughout the parser.
pub type TmuxResult<T = ()> = Result<T, TmuxError>;

/// An error produced while parsing a control-mode line or assembling a block.
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum TmuxError {
    /// The line does not begin with `%`.
    #[error("line does not start with '%'")]
    NotControlLine,

    /// The line was empty or whitespace-only.
    #[error("empty or whitespace-only line")]
    Empty,

    /// A required positional field was missing from the line.
    #[error("missing field '{field}' in {event}")]
    MissingField {
        /// The notification keyword being parsed (e.g. `window-renamed`).
        event: &'static str,
        /// The name of the absent field.
        field: &'static str,
    },

    /// An entity id was malformed (bad/missing prefix or non-integer body).
    #[error("invalid id {raw:?}, expected prefix '{expected}'")]
    InvalidId {
        /// The offending raw token.
        raw: String,
        /// The prefix that was expected (`%`, `@`, or `$`).
        expected: char,
    },

    /// An integer field could not be parsed.
    #[error("invalid integer for field '{field}': {raw:?}")]
    InvalidInt {
        /// The field whose integer parse failed.
        field: &'static str,
        /// The offending raw token.
        raw: String,
    },

    /// A `\xxx` octal escape in `%output` data was malformed.
    #[error("invalid octal escape: {raw:?}")]
    InvalidOctal {
        /// The offending escape fragment.
        raw: String,
    },

    /// A text sub-slice (name, layout, reply body) was not valid UTF-8.
    #[error("invalid UTF-8 in field '{field}'")]
    InvalidUtf8 {
        /// The field whose UTF-8 decode failed.
        field: &'static str,
    },

    /// A `%end` / `%error` guard arrived with no open `%begin` block.
    #[error("%end/%error with no open block (number {number})")]
    UnexpectedEnd {
        /// The command number carried by the stray guard.
        number: u32,
    },

    /// A tmux window-layout string was malformed.
    #[error("invalid layout: {reason}")]
    InvalidLayout {
        /// The specific structural failure.
        reason: LayoutError,
    },
}

/// The specific structural failure behind a [`TmuxError::InvalidLayout`].
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum LayoutError {
    /// The string had no comma separating checksum from body.
    #[error("missing comma after checksum")]
    MissingChecksumComma,

    /// The checksum was not exactly 4 lowercase hex digits.
    #[error("checksum must be 4 lowercase hex digits")]
    BadChecksum,

    /// Expected a specific literal byte that was absent.
    #[error("expected byte {expected:?}")]
    Expected {
        /// The byte the grammar required at this position.
        expected: u8,
    },

    /// A `,` introduced a pane id but no digits followed.
    #[error("expected pane id digits after ','")]
    ExpectedPaneId,

    /// A split was never closed by its matching bracket.
    #[error("unbalanced bracket, expected {expected:?}")]
    UnbalancedBracket {
        /// The closing byte (`}`, `]`, or `>`) that was expected.
        expected: u8,
    },

    /// An unexpected byte appeared where a cell or separator was expected.
    #[error("unexpected byte {byte:?}")]
    UnexpectedByte {
        /// The offending byte.
        byte: u8,
    },

    /// Bytes remained after the root cell was fully parsed.
    #[error("trailing data after layout root cell")]
    TrailingData,
}
