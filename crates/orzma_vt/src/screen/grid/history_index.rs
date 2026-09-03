//! Constant-time lookup of a history row's ring index by its `LineId`,
//! kept in step with the ring as rows enter, pop, and get reclaimed.

use crate::screen::grid::LineId;
use std::collections::HashMap;

/// Where each history row sits, by id, in constant time.
///
/// Only history is indexed: a row enters it at the newest end, leaves
/// it from the oldest end (the cap) or the newest end (a growth
/// reclaiming it), and is never recycled while inside. Each entry
/// therefore gets a running sequence number, and its ring index is
/// that number minus the count popped so far, so a pop moves nothing.
///
/// The visible rows stay unindexed: a region scroll recycles and
/// reorders them freely, and a scan over them is bounded by the screen
/// height rather than the history cap.
///
/// # Invariants
///
/// The live sequence numbers form the contiguous interval
/// `[popped, next_seq)`, so `seq_of.len() == next_seq - popped`, and
/// that length equals the grid's `history_len`.
#[derive(Debug, Default)]
pub(super) struct HistoryIndex {
    seq_of: HashMap<LineId, u64>,
    next_seq: u64,
    popped: u64,
}

impl HistoryIndex {
    /// The ring index of the history row `id`; `None` when `id` is not a
    /// history row.
    pub fn index_of(&self, id: LineId) -> Option<usize> {
        let seq = *self.seq_of.get(&id)?;
        Some(
            usize::try_from(seq - self.popped)
                .expect("a live entry sits at or past the popped count"),
        )
    }

    /// Records that `id` became the newest history row.
    pub fn enter(&mut self, id: LineId) {
        let replaced = self.seq_of.insert(id, self.next_seq);
        self.next_seq += 1;
        debug_assert!(replaced.is_none(), "a row enters history once");
        self.debug_assert_contiguous();
    }

    /// Records that `id`, the oldest history row, left through the cap.
    pub fn pop_oldest(&mut self, id: LineId) {
        let removed = self.seq_of.remove(&id);
        self.popped += 1;
        debug_assert_eq!(
            removed,
            Some(self.popped - 1),
            "the cap pops the oldest entry"
        );
        self.debug_assert_contiguous();
    }

    /// Records that `id`, the newest history row, went back to being
    /// visible.
    pub fn reclaim_newest(&mut self, id: LineId) {
        let removed = self.seq_of.remove(&id);
        self.next_seq -= 1;
        debug_assert_eq!(
            removed,
            Some(self.next_seq),
            "a growth reclaims the newest entry"
        );
        self.debug_assert_contiguous();
    }

    fn debug_assert_contiguous(&self) {
        debug_assert_eq!(
            u64::try_from(self.seq_of.len()).expect("an entry count fits in u64"),
            self.next_seq - self.popped
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts that an empty index resolves nothing.
    ///
    /// Case: a fresh terminal has printed less than one screenful, so no
    /// row has entered history yet.
    #[test]
    fn an_empty_index_resolves_nothing() {
        let index = HistoryIndex::default();
        assert_eq!(index.index_of(LineId(0)), None);
    }

    /// Asserts that rows entering history take consecutive ring indices
    /// in entry order.
    ///
    /// Case: the shell scrolls three lines of output into history on a
    /// terminal that has not reached its scrollback cap.
    #[test]
    fn entering_rows_take_consecutive_indices() {
        let mut index = HistoryIndex::default();
        index.enter(LineId(10));
        index.enter(LineId(11));
        index.enter(LineId(12));
        assert_eq!(index.index_of(LineId(10)), Some(0));
        assert_eq!(index.index_of(LineId(11)), Some(1));
        assert_eq!(index.index_of(LineId(12)), Some(2));
    }

    /// Asserts that popping the oldest row forgets it and shifts every
    /// survivor down by one.
    ///
    /// Case: output keeps scrolling on a terminal whose scrollback is at
    /// its cap, so each new history row pushes the oldest one out.
    #[test]
    fn popping_the_oldest_shifts_the_survivors_down() {
        let mut index = HistoryIndex::default();
        index.enter(LineId(10));
        index.enter(LineId(11));
        index.enter(LineId(12));
        index.pop_oldest(LineId(10));
        assert_eq!(index.index_of(LineId(10)), None);
        assert_eq!(index.index_of(LineId(11)), Some(0));
        assert_eq!(index.index_of(LineId(12)), Some(1));
    }

    /// Asserts that reclaiming the newest row forgets it and leaves the
    /// older rows where they were.
    ///
    /// Case: the user drags the window taller, so the newest history row
    /// comes back onto the screen.
    #[test]
    fn reclaiming_the_newest_leaves_the_older_rows_in_place() {
        let mut index = HistoryIndex::default();
        index.enter(LineId(10));
        index.enter(LineId(11));
        index.enter(LineId(12));
        index.reclaim_newest(LineId(12));
        assert_eq!(index.index_of(LineId(12)), None);
        assert_eq!(index.index_of(LineId(10)), Some(0));
        assert_eq!(index.index_of(LineId(11)), Some(1));
    }

    /// Asserts that a row entering after a reclaim takes the reclaimed
    /// slot without the reclaimed row resolving there.
    ///
    /// Case: the user drags the window taller, then the shell prints
    /// enough to scroll a new row into history.
    #[test]
    fn a_row_entering_after_a_reclaim_reuses_the_slot() {
        let mut index = HistoryIndex::default();
        index.enter(LineId(10));
        index.enter(LineId(11));
        index.reclaim_newest(LineId(11));
        index.enter(LineId(20));
        assert_eq!(index.index_of(LineId(20)), Some(1));
        assert_eq!(index.index_of(LineId(11)), None);
    }

    /// Asserts that a pop, a reclaim, and an entry compose into the
    /// positions the ring actually holds.
    ///
    /// Case: on a terminal at its scrollback cap, output pushes the
    /// oldest row out, the user drags the window taller, and the shell
    /// then scrolls another row into history.
    #[test]
    fn a_pop_a_reclaim_and_an_entry_compose() {
        let mut index = HistoryIndex::default();
        index.enter(LineId(10));
        index.enter(LineId(11));
        index.enter(LineId(12));
        index.pop_oldest(LineId(10));
        index.reclaim_newest(LineId(12));
        index.enter(LineId(30));
        assert_eq!(index.index_of(LineId(10)), None);
        assert_eq!(index.index_of(LineId(12)), None);
        assert_eq!(index.index_of(LineId(11)), Some(0));
        assert_eq!(index.index_of(LineId(30)), Some(1));
    }
}
