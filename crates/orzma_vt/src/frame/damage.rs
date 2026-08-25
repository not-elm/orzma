//! Damage vocabulary: the span one operation reports and the
//! accumulator that merges spans toward the next emit.
//!
//! [`DamageSpan`] is the allocation-free value a single screen
//! operation reports: a `Copy` span rather than a `Vec`, because this
//! value crosses the per-printed-character path where a `Vec` per
//! character would dominate the interpreter's cost.
//!
//! [`Damage`] accumulates what interpretation, scrolling, and resizing
//! each report per call and hands the merged rows to the frame emitter.
//! Staging merges rather than replaces: a source reports only what its
//! own call produced, so an overwritten value would drop a repaint no
//! later call re-reports.

use crate::schema::ViewportLine;
use std::iter;

/// Viewport rows one operation damaged.
///
/// `Copy` and allocation-free: this value crosses the per-character path
/// between a screen operation and the accumulator, where a `Vec` per
/// printed character would dominate the interpreter's cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DamageSpan {
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
    /// # Invariants
    ///
    /// `first <= last`; the accumulator's span writer sets wrong bits
    /// without panicking on a reversed pair.
    pub fn rows(first: ViewportLine, last: ViewportLine) -> Self {
        debug_assert!(first <= last, "a damage span runs top to bottom");
        Self::Rows { first, last }
    }
}

/// Damage accumulated toward the next frame emit.
///
/// # Invariants
///
/// While `full` is set the row bits are empty: staging `Full` clears
/// them and a span staged under a pending `Full` is discarded, which is
/// what lets [`Self::dirty_rows`] chain both sources unconditionally.
pub(crate) struct Damage {
    /// Whether the entire viewport is dirty. Height-independent: the
    /// flag expands against the emit-time viewport height, so a resize
    /// between staging and emitting cannot under- or over-cover.
    full: bool,
    /// Dirty-row bits, reused across frames so staging never allocates
    /// on the per-character path.
    rows: RowBits,
}

impl Damage {
    /// Builds the accumulator with the bootstrap repaint already staged.
    ///
    /// # Invariants
    ///
    /// Its seeded full damage is what makes the first emitted frame
    /// carry every viewport row; an accumulator that starts clean
    /// paints nothing until the first PTY output arrives.
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

    /// Returns whether nothing is staged, so the emit gate can treat
    /// the accumulator like the other unchanged sections.
    pub fn is_clean(&self) -> bool {
        !self.full && self.rows.is_empty()
    }

    /// The staged rows against the emit-time viewport height, ascending
    /// and without duplicates: every row below `height` when full,
    /// otherwise the set bits.
    ///
    /// The clip on the bit path is release-build defense; staged bits
    /// at or above the emit-time height are unreachable while every
    /// basis change stages `Full`.
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
/// from the least significant bit. Walking elements in order and bits by
/// `trailing_zeros` therefore yields rows ascending, which is what lets
/// the emitter build dirty rows without sorting.
///
/// The accumulator keeps one for the terminal's lifetime: staging a
/// span sets bits and an emit turns them back into rows, so nothing is
/// allocated once the buffer has grown to the viewport's height.
#[derive(Debug, Default)]
struct RowBits(Vec<u64>);

impl RowBits {
    /// Sets every row in the inclusive span.
    ///
    /// # Invariants
    ///
    /// `first <= last`. The `debug_assert!` catches a reversed span in a
    /// debug build; in release the masks and the buffer sizing both
    /// assume the ordering, so the result is meaningless.
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
        /// The property is kept inside the accumulator rather than resting
        /// on `Vt::resize` always staging `Full`, so a later change to the
        /// resize path cannot silently break it.
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
        /// The boundary cases are the reason the last-word mask is written
        /// as `u64::MAX >> (63 - last % 64)`: the arithmetically natural
        /// `!(u64::MAX << (last % 64 + 1))` shifts by 64 exactly when
        /// `last % 64 == 63`, which panics in a debug build.
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

        /// Asserts that clearing drops every set row, so a later span yields
        /// exactly its own rows and none of the ones it replaced.
        ///
        /// The buffer itself is deliberately kept: `clear` is `fill(0)`, not
        /// `Vec::clear`, so the element for every row ever staged stays
        /// allocated and the length remains the high-water mark.
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
