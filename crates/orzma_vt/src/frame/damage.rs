//! Damage tracking: which viewport rows the next frame must repaint.

use crate::screen::viewport::ViewportLine;
use std::iter;

/// Viewport rows one operation damaged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DamageSpan {
    /// Entire viewport is dirty (resize, clear, alt-screen swap, reset).
    Full,
    /// An inclusive viewport-row span; a single row is `first == last`.
    Rows {
        /// Topmost damaged row.
        first: ViewportLine,
        /// Bottommost damaged row, inclusive.
        last: ViewportLine,
    },
}

impl DamageSpan {
    /// Builds an inclusive row span.
    ///
    /// The caller must pass `first <= last`.
    pub fn rows(first: ViewportLine, last: ViewportLine) -> Self {
        debug_assert!(first <= last, "a damage span runs top to bottom");
        Self::Rows { first, last }
    }
}

/// Damage accumulated toward the next frame emit.
///
/// Staging merges rather than replaces. A span staged under a pending
/// `Full` is discarded.
///
/// # Invariants
///
/// While `full` is set the row bits are empty.
pub(crate) struct Damage {
    /// Whether the entire viewport is dirty. The flag is
    /// height-independent: it expands against the emit-time viewport
    /// height, so a resize between staging and emitting cannot under- or
    /// over-cover.
    full: bool,
    /// Dirty-row bits, reused across frames.
    rows: RowBits,
}

impl Damage {
    /// Builds the accumulator with the bootstrap repaint already staged.
    ///
    /// # Invariants
    ///
    /// Its seeded full damage makes the first emitted frame carry every
    /// viewport row.
    pub fn new() -> Self {
        Self {
            full: true,
            rows: RowBits::default(),
        }
    }

    /// Merges `span` into the accumulated damage.
    pub fn stage(&mut self, span: DamageSpan) {
        match span {
            DamageSpan::Full => {
                self.full = true;
                self.rows.clear();
            }
            DamageSpan::Rows { first, last } => {
                if !self.full {
                    self.rows.set_span(first, last);
                    debug_assert!(!self.rows.is_empty(), "a staged span sets at least one bit");
                }
            }
        }
    }

    /// Returns whether nothing is staged.
    pub fn is_clean(&self) -> bool {
        !self.full && self.rows.is_empty()
    }

    /// The staged rows against the emit-time viewport height, ascending
    /// and without duplicates: every row below `height` when full,
    /// otherwise the set bits.
    ///
    /// Every staged row must lie below `height`; a release build drops
    /// any that do not.
    pub fn dirty_rows(&self, height: u16) -> impl Iterator<Item = ViewportLine> + '_ {
        debug_assert!(
            self.rows.rows().all(|line| line.0 < height),
            "staged row damage must fit the emit-time viewport"
        );
        let full_span = if self.full { 0..height } else { 0..0 };
        full_span
            .map(ViewportLine)
            .chain(self.rows.rows().filter(move |line| line.0 < height))
    }

    /// Empties the accumulator after an emit, keeping the bit buffer
    /// for the next frame.
    pub fn clear(&mut self) {
        self.full = false;
        self.rows.clear();
    }
}

/// A reusable set of dirty viewport rows, one bit per row.
///
/// Bit `b` of element `w` is viewport row `w * 64 + b`, with `b` counted
/// from the least significant bit.
#[derive(Debug, Default)]
struct RowBits(Vec<u64>);

impl RowBits {
    /// Sets every row in the inclusive span.
    ///
    /// The caller must pass `first <= last`; a debug build panics on a
    /// reversed span.
    fn set_span(&mut self, first: ViewportLine, last: ViewportLine) {
        debug_assert!(first <= last, "a damage span runs top to bottom");
        let (first, last) = (usize::from(first.0), usize::from(last.0));
        let needed = last / 64 + 1;
        if self.0.len() < needed {
            self.0.resize(needed, 0);
        }
        let head = u64::MAX << (first % 64);
        let tail = u64::MAX >> (63 - last % 64);
        let (first_word, last_word) = (first / 64, last / 64);
        if first_word == last_word {
            self.0[first_word] |= head & tail;
            return;
        }
        self.0[first_word] |= head;
        self.0[first_word + 1..last_word].fill(u64::MAX);
        self.0[last_word] |= tail;
    }

    /// Empties the set, keeping the buffer for the next frame.
    fn clear(&mut self) {
        self.0.fill(0);
    }

    /// Returns whether no row is set.
    fn is_empty(&self) -> bool {
        self.0.iter().all(|word| *word == 0)
    }

    /// The set rows, ascending and without duplicates.
    fn rows(&self) -> impl Iterator<Item = ViewportLine> + '_ {
        self.0.iter().enumerate().flat_map(|(index, &word)| {
            iter::successors((word != 0).then_some(word), |rest| {
                let next = *rest & (*rest - 1);
                (next != 0).then_some(next)
            })
            .map(move |rest| {
                let bit = rest.trailing_zeros() as usize;
                ViewportLine(u16::try_from(index * 64 + bit).expect("a viewport row fits in u16"))
            })
        })
    }
}

#[cfg(test)]
mod tests {
    mod span {
        use super::super::*;

        /// Asserts that the span constructor keeps its endpoints and that a
        /// single row is the degenerate span.
        ///
        /// Case: a printed character damages exactly the row it landed on.
        #[test]
        fn a_single_row_span_carries_the_same_endpoint_twice() {
            assert_eq!(
                DamageSpan::rows(ViewportLine(4), ViewportLine(4)),
                DamageSpan::Rows {
                    first: ViewportLine(4),
                    last: ViewportLine(4),
                }
            );
        }
    }

    mod accumulator {
        use super::super::*;

        fn drain(damage: &mut Damage, height: u16) -> Vec<u16> {
            let rows = damage.dirty_rows(height).map(|line| line.0).collect();
            damage.clear();
            rows
        }

        /// Asserts that a fresh accumulator drains as a full repaint and
        /// is clean afterwards.
        ///
        /// Case: a terminal is spawned and paints its first frame before
        /// any PTY output has arrived.
        #[test]
        fn a_fresh_accumulator_drains_as_a_full_repaint() {
            let mut damage = Damage::new();
            assert!(!damage.is_clean());
            assert_eq!(drain(&mut damage, 3), [0, 1, 2]);
            assert!(damage.is_clean());
        }

        /// Asserts that row damage staged while a full repaint is pending
        /// is discarded rather than retained.
        ///
        /// Case: a resize stages a full repaint and the shell keeps
        /// printing before the frame is emitted.
        #[test]
        fn rows_staged_under_a_pending_full_repaint_are_discarded() {
            let mut damage = Damage::new();
            damage.clear();
            damage.stage(DamageSpan::Full);
            damage.stage(DamageSpan::rows(ViewportLine(5), ViewportLine(5)));
            assert_eq!(drain(&mut damage, 3), [0, 1, 2]);
            damage.stage(DamageSpan::rows(ViewportLine(1), ViewportLine(1)));
            assert_eq!(drain(&mut damage, 3), [1]);
        }

        /// Asserts that spans accumulate across calls and drain ascending
        /// without duplicates.
        ///
        /// Case: one PTY chunk prints on several rows before the
        /// coalescer's window closes.
        #[test]
        fn staged_spans_accumulate_and_drain_ascending() {
            let mut damage = Damage::new();
            damage.clear();
            damage.stage(DamageSpan::rows(ViewportLine(3), ViewportLine(4)));
            damage.stage(DamageSpan::rows(ViewportLine(0), ViewportLine(0)));
            damage.stage(DamageSpan::rows(ViewportLine(3), ViewportLine(3)));
            assert_eq!(drain(&mut damage, 5), [0, 3, 4]);
        }

        /// Asserts that a full repaint clears the bits it supersedes, so a
        /// later shrink cannot surface a row past the new viewport.
        ///
        /// Case: the window shrinks after output damaged a row that the
        /// smaller viewport no longer has.
        #[test]
        fn a_full_repaint_clears_the_rows_it_supersedes() {
            let mut damage = Damage::new();
            damage.clear();
            damage.stage(DamageSpan::rows(ViewportLine(90), ViewportLine(90)));
            damage.stage(DamageSpan::Full);
            assert_eq!(drain(&mut damage, 3), [0, 1, 2]);
            damage.stage(DamageSpan::rows(ViewportLine(1), ViewportLine(1)));
            assert_eq!(drain(&mut damage, 3), [1]);
        }
    }

    mod row_bits {
        use super::super::*;

        fn rows_of(bits: &RowBits) -> Vec<u16> {
            bits.rows().map(|line| line.0).collect()
        }

        fn span(first: u16, last: u16) -> RowBits {
            let mut bits = RowBits::default();
            bits.set_span(ViewportLine(first), ViewportLine(last));
            bits
        }

        /// Asserts that every span endpoint around a 64-row word boundary
        /// sets exactly the rows it names.
        ///
        /// Case: a viewport whose height is a multiple of 64 erases from the
        /// cursor to the last row.
        #[test]
        fn a_span_sets_exactly_its_rows_across_word_boundaries() {
            assert_eq!(rows_of(&span(0, 0)), [0]);
            assert_eq!(rows_of(&span(63, 63)), [63]);
            assert_eq!(rows_of(&span(64, 64)), [64]);
            assert_eq!(rows_of(&span(0, 63)), (0..=63).collect::<Vec<_>>());
            assert_eq!(rows_of(&span(63, 64)), [63, 64]);
            assert_eq!(rows_of(&span(64, 127)), (64..=127).collect::<Vec<_>>());
            assert_eq!(rows_of(&span(1, 130)), (1..=130).collect::<Vec<_>>());
        }

        /// Asserts that overlapping spans union without duplicating a row.
        ///
        /// Case: a chunk prints on row 2, then a linefeed damages rows 2
        /// and 3 before the frame is emitted.
        #[test]
        fn overlapping_spans_union_without_duplicates() {
            let mut bits = RowBits::default();
            bits.set_span(ViewportLine(2), ViewportLine(2));
            bits.set_span(ViewportLine(2), ViewportLine(3));
            assert_eq!(rows_of(&bits), [2, 3]);
        }

        /// Asserts that disjoint spans are both retained, ascending.
        ///
        /// Case: a full-screen application repaints its top status line and
        /// its bottom mode line in one chunk.
        #[test]
        fn disjoint_spans_are_both_retained_in_ascending_order() {
            let mut bits = RowBits::default();
            bits.set_span(ViewportLine(20), ViewportLine(20));
            bits.set_span(ViewportLine(0), ViewportLine(0));
            assert_eq!(rows_of(&bits), [0, 20]);
        }

        /// Asserts that clearing drops every set row without shortening the
        /// buffer, so a later span yields exactly its own rows and none of
        /// the ones it replaced.
        ///
        /// Case: a frame is emitted and the next chunk starts staging into
        /// the same terminal's accumulator.
        #[test]
        fn clearing_drops_every_set_row() {
            let mut bits = span(0, 200);
            let len = bits.0.len();
            bits.clear();
            assert_eq!(bits.0.len(), len, "clear must not shorten the buffer");
            assert!(bits.is_empty());
            assert_eq!(rows_of(&bits), Vec::<u16>::new());

            bits.set_span(ViewportLine(200), ViewportLine(200));
            assert_eq!(rows_of(&bits), [200], "a cleared bit came back");
        }
    }
}
