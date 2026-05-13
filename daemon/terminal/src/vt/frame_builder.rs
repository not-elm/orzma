//! Pure functions that turn a Term snapshot into wire frames.
//!
//! All entry points take the Term by mutable reference because
//! `Term::damage()` mutates internal damage state. Callers must hold the
//! `vt_state` lock; this module performs no locking.

use alacritty_terminal::Term;
use alacritty_terminal::term::TermDamage;

/// Outcome of damage inspection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirtyRows {
    /// Entire screen is dirty (resize / alt-screen swap / clear / reset).
    Full,
    /// Specific row indices are dirty.
    Rows(Vec<u16>),
}

/// Reads the damage tracker and returns row indices that changed.
///
/// `&mut Term` is required because `Term::damage()` takes `&mut self`.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "consumed by Phase 2A frame emit path (Tasks 7-9)")
)]
pub fn collect_dirty_rows<T>(term: &mut Term<T>) -> DirtyRows {
    match term.damage() {
        TermDamage::Full => DirtyRows::Full,
        TermDamage::Partial(iter) => DirtyRows::Rows(iter.map(|d| d.line as u16).collect()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vt::frame_ring::WireMessage;
    use crate::vt::listener::{ControlFrame, DropCounter, ReplyFrame, TermListener};
    use std::sync::Arc;
    use tokio::sync::{broadcast, mpsc};

    fn make_term(cols: u16, rows: u16) -> Term<TermListener> {
        let (reply_tx, _) = mpsc::unbounded_channel::<ReplyFrame>();
        let (control_tx, _) = mpsc::channel::<ControlFrame>(64);
        let _ = broadcast::channel::<WireMessage>(8);
        let listener = TermListener {
            reply_tx,
            control_tx,
            drop_counter: Arc::new(DropCounter::new()),
        };
        let size = crate::vt::bridge::test_dim(cols, rows);
        Term::new(alacritty_terminal::term::Config::default(), &size, listener)
    }

    #[test]
    fn collect_full_on_fresh_term() {
        let mut term = make_term(10, 3);
        // First damage query returns Full per alacritty contract.
        assert_eq!(collect_dirty_rows(&mut term), DirtyRows::Full);
    }

    #[test]
    fn collect_partial_after_reset() {
        let mut term = make_term(10, 3);
        let _ = collect_dirty_rows(&mut term);
        term.reset_damage();
        // After reset with no input, alacritty returns Partial{cursor row}.
        match collect_dirty_rows(&mut term) {
            DirtyRows::Full => panic!("expected Partial after reset"),
            DirtyRows::Rows(rows) => {
                // line_count is 1 (cursor blink dirties cursor row only).
                assert_eq!(rows.len(), 1);
            }
        }
    }
}
