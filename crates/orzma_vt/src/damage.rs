//! Damage vocabulary and the ledger that stages it.
//!
//! [`DamageLedger`] accumulates what interpretation, scrolling, and
//! resizing each report per call and hands the merged result to the
//! frame emitter as a [`StagedDamage`]. Staging merges rather than
//! replaces: a source reports only what its own call produced, so an
//! overwritten staged value would drop a repaint no later call
//! re-reports.
//!
//! [`Damage`] is the allocation-free counterpart a single screen
//! operation reports: a `Copy` span rather than a `Vec`, because this
//! value crosses the per-printed-character path where a `Vec` per
//! character would dominate the interpreter's cost.

use crate::schema::ViewportLine;
use std::{iter, ops::Deref};

/// Viewport damage the VT reported in a single damage cycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StagedDamage {
    /// Entire viewport is dirty (resize, clear, alt-screen swap, reset).
    Full,
    /// Only the carried rows are dirty.
    Delta(DamageRows),
}

/// Dirty viewport row indices, ascending and without duplicates.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DamageRows(Vec<ViewportLine>);

impl Deref for DamageRows {
    type Target = [ViewportLine];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl FromIterator<ViewportLine> for DamageRows {
    fn from_iter<I: IntoIterator<Item = ViewportLine>>(iter: I) -> Self {
        let mut rows: Vec<ViewportLine> = iter.into_iter().collect();
        rows.sort_unstable();
        rows.dedup();
        Self(rows)
    }
}

impl DamageRows {
    /// Collects already-ascending, already-unique rows.
    ///
    /// # Invariants
    ///
    /// The iterator must yield rows ascending without repeats; the
    /// ledger's bit walk does by construction. Feeding an unordered
    /// iterator here silently violates this type's ordering contract.
    fn from_ascending(count: usize, rows: impl Iterator<Item = ViewportLine>) -> Self {
        let mut collected = Vec::with_capacity(count);
        collected.extend(rows);
        debug_assert!(
            collected.is_sorted_by(|a, b| a < b),
            "the ledger's bit walk yields rows ascending and unique"
        );
        Self(collected)
    }
}

/// Damage staged for the next frame emit.
pub(crate) struct DamageLedger {
    /// Damage staged for the next emit; `None` when nothing is.
    staged: Option<Staged>,
    /// Dirty-row bits, reused across frames so staging never allocates
    /// on the per-character path. Meaningful only while `staged` is
    /// `Some(Staged::Rows)`.
    rows: RowBits,
}

impl DamageLedger {
    /// Builds a ledger with the bootstrap repaint already staged.
    ///
    /// # Invariants
    ///
    /// Its seeded full damage is what makes the first emitted frame
    /// carry every viewport row; a ledger that starts empty paints
    /// nothing until the first PTY output arrives.
    pub fn new() -> Self {
        Self {
            staged: Some(Staged::Full),
            rows: RowBits::default(),
        }
    }

    /// Merges `damage` into the staged value.
    pub fn stage(&mut self, damage: Damage) {
        match (&self.staged, damage) {
            (Some(Staged::Full), _) => {}
            (_, Damage::Full) => {
                self.staged = Some(Staged::Full);
                self.rows.clear();
            }
            (_, Damage::Rows { first, last }) => {
                self.staged = Some(Staged::Rows);
                self.rows.set_span(first, last);
            }
        }
    }

    /// Stages the reported damage, if any; returns whether there was any
    /// to stage.
    pub fn stage_if_changed(&mut self, damage: Option<Damage>) -> bool {
        match damage {
            Some(damage) => {
                self.stage(damage);
                true
            }
            None => false,
        }
    }

    /// Hands over the staged damage, leaving the ledger empty; `None`
    /// when nothing is staged.
    pub fn take(&mut self) -> Option<StagedDamage> {
        let staged = self.staged.take()?;
        let drained = match staged {
            Staged::Full => StagedDamage::Full,
            Staged::Rows => {
                debug_assert!(
                    self.rows.count_ones() > 0,
                    "Rows is staged only by a span, so the set cannot be empty"
                );
                StagedDamage::Delta(DamageRows::from_ascending(
                    self.rows.count_ones(),
                    self.rows.rows(),
                ))
            }
        };
        self.rows.clear();
        Some(drained)
    }
}

/// Which kind of damage the ledger holds.
enum Staged {
    /// Every viewport row.
    Full,
    /// The rows the ledger's bit set names.
    ///
    /// # Invariants
    ///
    /// At least one bit is set: `Rows` is staged only by a span, so an
    /// empty set is unreachable.
    Rows,
}

/// Viewport rows one operation damaged.
///
/// `Copy` and allocation-free: this value crosses the per-character path
/// between a screen operation and the ledger, where a `Vec` per printed
/// character would dominate the interpreter's cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Damage {
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

impl Damage {
    /// Builds an inclusive row span.
    ///
    /// # Invariants
    ///
    /// `first <= last`; the ledger's span writer sets wrong bits without
    /// panicking on a reversed pair.
    pub fn rows(first: ViewportLine, last: ViewportLine) -> Self {
        debug_assert!(first <= last, "a damage span runs top to bottom");
        Self::Rows { first, last }
    }
}

/// A reusable set of dirty viewport rows, one bit per row.
///
/// Bit `b` of element `w` is viewport row `w * 64 + b`, with `b` counted
/// from the least significant bit. Walking elements in order and bits by
/// `trailing_zeros` therefore yields rows ascending, which is what lets
/// a drain build [`DamageRows`] without sorting.
///
/// The ledger keeps one for the terminal's lifetime: staging a span sets
/// bits and draining turns them back into rows, so nothing is allocated
/// once the buffer has grown to the viewport's height.
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

    /// Number of rows currently set.
    fn count_ones(&self) -> usize {
        self.0.iter().map(|word| word.count_ones() as usize).sum()
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
    mod damage {
        use super::super::*;

        /// Asserts that the span constructor keeps its endpoints and that a
        /// single row is the degenerate span.
        ///
        /// Case: a printed character damages exactly the row it landed on.
        #[test]
        fn a_single_row_span_carries_the_same_endpoint_twice() {
            assert_eq!(
                Damage::rows(ViewportLine(4), ViewportLine(4)),
                Damage::Rows {
                    first: ViewportLine(4),
                    last: ViewportLine(4),
                }
            );
        }
    }

    mod ledger {
        use super::super::*;

        fn rows(ledger: &mut DamageLedger) -> Vec<u16> {
            match ledger.take() {
                Some(StagedDamage::Delta(rows)) => rows.iter().map(|line| line.0).collect(),
                other => panic!("expected a delta, got {other:?}"),
            }
        }

        /// Asserts that a fresh ledger's first drain is a full repaint.
        ///
        /// Case: a terminal is spawned and paints its first frame before any
        /// PTY output has arrived.
        #[test]
        fn a_fresh_ledger_drains_as_a_full_repaint() {
            let mut ledger = DamageLedger::new();
            assert!(matches!(ledger.take(), Some(StagedDamage::Full)));
            assert!(ledger.take().is_none());
        }

        /// Asserts that row damage staged while a full repaint is pending is
        /// discarded rather than retained.
        ///
        /// Case: a resize stages a full repaint and the shell keeps printing
        /// before the frame is emitted.
        #[test]
        fn rows_staged_under_a_pending_full_repaint_are_discarded() {
            let mut ledger = DamageLedger::new();
            ledger.take();
            ledger.stage(Damage::Full);
            ledger.stage(Damage::rows(ViewportLine(5), ViewportLine(5)));
            assert!(matches!(ledger.take(), Some(StagedDamage::Full)));
            ledger.stage(Damage::rows(ViewportLine(1), ViewportLine(1)));
            assert_eq!(rows(&mut ledger), [1]);
        }

        /// Asserts that spans accumulate across calls and drain ascending
        /// without duplicates.
        ///
        /// Case: one PTY chunk prints on several rows before the coalescer's
        /// window closes.
        #[test]
        fn staged_spans_accumulate_and_drain_ascending() {
            let mut ledger = DamageLedger::new();
            ledger.take();
            ledger.stage(Damage::rows(ViewportLine(3), ViewportLine(4)));
            ledger.stage(Damage::rows(ViewportLine(0), ViewportLine(0)));
            ledger.stage(Damage::rows(ViewportLine(3), ViewportLine(3)));
            assert_eq!(rows(&mut ledger), [0, 3, 4]);
        }

        /// Asserts that `stage_if_changed` reports whether it staged
        /// anything.
        ///
        /// Case: a scroll request is clamped to a no-op and its caller must
        /// learn the viewport did not move.
        #[test]
        fn stage_if_changed_reports_whether_anything_was_staged() {
            let mut ledger = DamageLedger::new();
            ledger.take();
            assert!(!ledger.stage_if_changed(None));
            assert!(ledger.take().is_none());
            assert!(ledger.stage_if_changed(Some(Damage::Full)));
            assert!(matches!(ledger.take(), Some(StagedDamage::Full)));
        }

        /// Asserts that a full repaint clears the bits it supersedes, so a
        /// later shrink cannot surface a row past the new viewport.
        ///
        /// The property is kept inside the ledger rather than resting on
        /// `Vt::resize` always staging `Full`, so a later change to the
        /// resize path cannot silently break it.
        ///
        /// Case: the window shrinks after output damaged a row that the
        /// smaller viewport no longer has.
        #[test]
        fn a_full_repaint_clears_the_rows_it_supersedes() {
            let mut ledger = DamageLedger::new();
            ledger.take();
            ledger.stage(Damage::rows(ViewportLine(90), ViewportLine(90)));
            ledger.stage(Damage::Full);
            assert!(matches!(ledger.take(), Some(StagedDamage::Full)));
            ledger.stage(Damage::rows(ViewportLine(1), ViewportLine(1)));
            assert_eq!(rows(&mut ledger), [1]);
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
            assert_eq!(bits.count_ones(), 2);
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
        /// the same terminal's ledger.
        #[test]
        fn clearing_drops_every_set_row() {
            let mut bits = span(0, 200);
            let len = bits.0.len();
            bits.clear();
            assert_eq!(bits.0.len(), len, "clear must not shorten the buffer");
            assert_eq!(bits.count_ones(), 0);
            assert_eq!(rows_of(&bits), Vec::<u16>::new());

            bits.set_span(ViewportLine(200), ViewportLine(200));
            assert_eq!(rows_of(&bits), [200], "a cleared bit came back");
        }
    }
}
