//! Terminal-level signals: the signals the VT stream raises plus the
//! child process's lifecycle events.

use orzma_vt::prelude::VtSignal;

/// A signal the terminal surfaces to its owner via `OrzmaTty::pump`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TtySignal {
    /// The child shell exited; `code` is `None` if the `wait` itself
    /// failed. Fired at most once per terminal.
    ChildExit { code: Option<i32> },
    /// A signal the VT raised — one an interpreted chunk produced, or the
    /// eviction a resize reported.
    Vt(VtSignal),
}
