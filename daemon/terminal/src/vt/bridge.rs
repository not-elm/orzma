//! VtState + vt_bridge_task: drives Term from PTY chunks.
//!
//! Phase 1: bridge は process_bytes を呼ぶところまで実装し、frame emit は
//! しない。Phase 2 で frame_ring/frame_broadcast/coalescer と統合する。

use std::sync::Arc;
use std::time::Instant;

use alacritty_terminal::Term;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::Config;
use bytes::Bytes;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::vt::frame_ring::FrameRing;
use crate::vt::listener::{ControlFrame, ReplyFrame, TermListener};

/// Bundles all state mutated by the VT bridge task. Wrapped in
/// `std::sync::Mutex` by `PtyHandle` (Task 12) so the bridge can take a
/// short non-await lock per PTY chunk.
pub struct VtState {
    pub term: Term<TermListener>,
    pub parser: alacritty_terminal::vte::ansi::Processor,
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "consumed by Phase 2 frame coalescer")
    )]
    pub frame_ring: FrameRing,
    /// Most recent client → server input timestamp (used by Phase 2
    /// coalescer to allow interactive-echo immediate flush).
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "consumed by Phase 2 frame coalescer")
    )]
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
            parser: alacritty_terminal::vte::ansi::Processor::new(),
            frame_ring: FrameRing::new(256 * 1024),
            last_input_at: None,
        }
    }

    /// Feed a chunk of PTY bytes through `vte::Parser` into `Term`.
    /// Wrapped as a helper so the bridge task can borrow `parser` and `term`
    /// disjointly without tripping the borrow checker.
    pub fn advance(&mut self, chunk: &[u8]) {
        self.parser.advance(&mut self.term, chunk);
    }
}

/// Phase 1 bridge task: drains PTY chunks into Term via vte::Parser.
/// Phase 1 does NOT emit any frames; reply_rx / control_rx are drained
/// minimally to keep channels from filling up.
///
/// Wired up in Phase 2 with frame coalescing + broadcast emission.
pub async fn run_bridge_task(
    vt_state: Arc<std::sync::Mutex<VtState>>,
    mut pty_rx: mpsc::Receiver<Bytes>,
    mut reply_rx: mpsc::UnboundedReceiver<ReplyFrame>,
    mut control_rx: mpsc::Receiver<ControlFrame>,
    cancel: CancellationToken,
) {
    loop {
        tokio::select! {
            biased;
            () = cancel.cancelled() => break,

            // PTY chunk → Term
            chunk = pty_rx.recv() => {
                let Some(chunk) = chunk else { break };
                let mut state = vt_state.lock().expect("vt_state poisoned");
                state.advance(&chunk);
            }

            // Reply-required (drain only in Phase 1; Phase 2 wires PTY writer)
            reply = reply_rx.recv() => {
                let Some(_reply) = reply else { break };
                // Phase 1: discard. Phase 2 will write PtyWrite bytes back
                // to the PTY and invoke reply closures for size/color requests.
            }

            // Best-effort (drain only in Phase 1)
            ctrl = control_rx.recv() => {
                let Some(_ctrl) = ctrl else { break };
                // Phase 1: discard. Phase 2 emits these as JSON frames.
            }
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

    use bytes::Bytes;
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn bridge_task_consumes_pty_chunks_and_updates_term() {
        // Setup: VtState を Arc<Mutex<>> で共有し、bridge task に渡す。
        let (reply_tx, reply_rx) = mpsc::unbounded_channel::<ReplyFrame>();
        let (control_tx, control_rx) = mpsc::channel::<ControlFrame>(64);
        let drop_counter = Arc::new(DropCounter::new());
        let listener = TermListener {
            reply_tx,
            control_tx,
            drop_counter: drop_counter.clone(),
        };
        let vt_state = Arc::new(std::sync::Mutex::new(VtState::new(10, 3, listener)));

        let (pty_tx, pty_rx) = mpsc::channel::<Bytes>(8);
        let cancel = CancellationToken::new();
        let handle = tokio::spawn(super::run_bridge_task(
            vt_state.clone(),
            pty_rx,
            reply_rx,
            control_rx,
            cancel.clone(),
        ));

        // Send a "hello" chunk
        pty_tx.send(Bytes::from_static(b"hello")).await.unwrap();

        // Give the bridge task a moment to consume and update Term
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Verify Term grid has "hello" on row 0 (lock dropped before any await
        // to avoid `clippy::await_holding_lock`).
        let line0_text: String = {
            let state = vt_state.lock().unwrap();
            let row = &state.term.grid()[alacritty_terminal::index::Line(0)];
            let slice =
                &row[alacritty_terminal::index::Column(0)..alacritty_terminal::index::Column(5)];
            slice.iter().map(|cell| cell.c).collect()
        };
        assert!(
            line0_text.starts_with("hello"),
            "expected 'hello' on row 0, got: {:?}",
            line0_text
        );

        // Cancel and wait
        cancel.cancel();
        let _ = handle.await;
    }
}
