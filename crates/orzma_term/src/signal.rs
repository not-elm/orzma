//! Terminal-level signals: the VT-stream signals plus process
//! lifecycle events the VT cannot observe.

use orzma_vt::prelude::VtSignal;

/// A signal the terminal surfaces to its owner via `OrzmaTerm::pump`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TermSignal {
    /// The child shell exited; `code` is `None` if the `wait` itself
    /// failed. Fired exactly once per terminal.
    ChildExit { code: Option<i32> },
    /// A signal parsed from the VT byte stream.
    Vt(VtSignal),
}
