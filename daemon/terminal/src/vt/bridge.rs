//! VtState + vt_bridge_task: drives Term from PTY chunks.
//!
//! Phase 1: bridge は process_bytes を呼ぶところまで実装し、frame emit は
//! しない。Phase 2 で frame_ring/frame_broadcast/coalescer と統合する。

#![cfg_attr(
    not(test),
    expect(dead_code, reason = "VtState is wired up by PtyHandle in Task 12")
)]

use std::time::Instant;

use alacritty_terminal::Term;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::Config;

use crate::vt::frame_ring::FrameRing;
use crate::vt::listener::TermListener;

/// Bundles all state mutated by the VT bridge task. Wrapped in
/// `std::sync::Mutex` by `PtyHandle` (Task 12) so the bridge can take a
/// short non-await lock per PTY chunk.
pub struct VtState {
    pub term: Term<TermListener>,
    #[expect(dead_code, reason = "consumed by vt_bridge_task in Task 13")]
    pub parser: vte::Parser,
    pub frame_ring: FrameRing,
    /// Most recent client → server input timestamp (used by Phase 2
    /// coalescer to allow interactive-echo immediate flush).
    pub last_input_at: Option<Instant>,
}

impl VtState {
    /// Construct a fresh `VtState` with the given terminal dimensions.
    ///
    /// Alacritty 0.26 API actually used:
    /// - `alacritty_terminal::Term::new<D: Dimensions>(Config, &D, T) -> Term<T>`
    ///   (`src/term/mod.rs:410`).
    /// - `Dimensions` trait lives at `alacritty_terminal::grid::Dimensions`
    ///   (`src/grid/mod.rs:486`) — it is **not** re-exported from
    ///   `alacritty_terminal::term`.
    /// - `Config::default()` is sufficient; no feature flags required.
    pub fn new(cols: u16, rows: u16, listener: TermListener) -> Self {
        let size = LocalDim {
            cols: cols.into(),
            rows: rows.into(),
        };
        let config = Config::default();
        let term = Term::new(config, &size, listener);
        Self {
            term,
            parser: vte::Parser::new(),
            frame_ring: FrameRing::new(256 * 1024),
            last_input_at: None,
        }
    }
}

/// Minimal local impl of `alacritty_terminal::grid::Dimensions` so we can
/// construct `Term::new` without depending on internal helpers (`TermSize`
/// is `pub(crate)` inside alacritty's `term::tests`).
struct LocalDim {
    cols: usize,
    rows: usize,
}

impl Dimensions for LocalDim {
    fn columns(&self) -> usize {
        self.cols
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn total_lines(&self) -> usize {
        self.rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vt::listener::{ControlFrame, DropCounter, ReplyFrame};
    use std::sync::Arc;
    use tokio::sync::mpsc;

    fn make_listener() -> TermListener {
        let (reply_tx, _) = mpsc::unbounded_channel::<ReplyFrame>();
        let (control_tx, _) = mpsc::channel::<ControlFrame>(64);
        TermListener {
            reply_tx,
            control_tx,
            drop_counter: Arc::new(DropCounter::new()),
        }
    }

    #[test]
    fn vt_state_constructs_with_dimensions() {
        let state = VtState::new(80, 24, make_listener());
        assert!(state.frame_ring.is_empty());
        assert!(state.last_input_at.is_none());
        assert_eq!(state.term.columns(), 80);
        assert_eq!(state.term.screen_lines(), 24);
    }
}
