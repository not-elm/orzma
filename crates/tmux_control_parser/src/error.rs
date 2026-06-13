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
}
