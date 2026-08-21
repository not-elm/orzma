//! Damage vocabulary and the ledger that stages it.
//!
//! [`Damage`] is what one source reports for a single damage cycle, and
//! [`DamageVerdict`] classifies it for the coalescer's immediate-flush
//! decision. [`DamageLedger`] accumulates what interpretation,
//! scrolling, resizing, and placement changes each report per call and
//! hands the merged result to the frame emitter. Staging merges rather
//! than replaces: a source reports only what its own call produced, so
//! an overwritten staged value would drop a repaint no later call
//! re-reports.

use crate::schema::ViewportLine;
#[cfg(feature = "alacritty")]
use alacritty_terminal::{Term, term::TermDamage};
use std::{
    iter,
    ops::{BitOrAssign, Deref},
};

/// Viewport damage the VT reported in a single damage cycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Damage {
    /// Entire viewport is dirty (resize, clear, alt-screen swap, reset).
    Full,
    /// Only the carried rows are dirty.
    Delta(DamageRows),
}

impl Damage {
    /// Reads alacritty's accumulated damage for one cycle.
    ///
    /// # Invariants
    ///
    /// `Term::damage()` consumes its own `last_cursor` bookkeeping, so it must
    /// be called exactly once per cycle, and the caller must
    /// `Term::reset_damage()` immediately after the read so the next cycle
    /// reports only its own damage — a skipped reset latches `damage.full`
    /// and every later cycle reports `Full`.
    #[cfg(feature = "alacritty")]
    pub fn from_alacritty_term<T>(term: &mut Term<T>) -> Self {
        match term.damage() {
            TermDamage::Full => Self::Full,
            TermDamage::Partial(iter) => Self::Delta(
                iter.map(|d| {
                    ViewportLine(
                        u16::try_from(d.line).expect("a terminal's viewport rows fit in u16"),
                    )
                })
                .collect(),
            ),
        }
    }
}

/// Merges damage, keeping whichever repaint is the larger of the two.
///
/// [`Damage::Full`] absorbs anything and an empty
/// [`Damage::Delta`] is the identity, so a staged value can start
/// from an empty set and fold every later reading in.
impl BitOrAssign for Damage {
    fn bitor_assign(&mut self, rhs: Self) {
        match (self, rhs) {
            (Self::Full, _) => {}
            (staged, Self::Full) => *staged = Self::Full,
            (Self::Delta(staged), Self::Delta(incoming)) => {
                if incoming.0.is_empty() {
                    return;
                }
                if staged.0.is_empty() {
                    *staged = incoming;
                    return;
                }
                staged.0.extend(incoming.0);
                staged.0.sort_unstable();
                staged.0.dedup();
            }
        }
    }
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

impl From<Vec<ViewportLine>> for DamageRows {
    fn from(mut rows: Vec<ViewportLine>) -> Self {
        rows.sort_unstable();
        rows.dedup();
        Self(rows)
    }
}

impl FromIterator<ViewportLine> for DamageRows {
    fn from_iter<I: IntoIterator<Item = ViewportLine>>(iter: I) -> Self {
        Self::from(iter.into_iter().collect::<Vec<ViewportLine>>())
    }
}

/// Classification of collected damage that drives the immediate-flush decision.
///
/// The owner classifies once per interpreted chunk and keeps the matching
/// [`Damage`] staged for the emit, so the backend's damage tracker is
/// read exactly once per cycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DamageVerdict {
    /// Entire screen damaged (resize, clear, alt-screen swap).
    Full,
    /// At most one row is dirty (interactive echo).
    AtMostOneRow,
    /// Two or more rows dirty. The row count drives the PR-E2b
    /// immediate-flush cap in `Coalescer::should_flush_immediately`.
    ManyRows { rows: usize },
    /// No visible dirty rows.
    Idle,
}

impl DamageVerdict {
    /// Classifies already-collected damage for the coalescer's
    /// immediate-flush decision.
    pub fn classify(damage: &Damage) -> Self {
        match damage {
            Damage::Full => Self::Full,
            Damage::Delta(rows) => match rows.len() {
                0 => Self::Idle,
                1 => Self::AtMostOneRow,
                n => Self::ManyRows { rows: n },
            },
        }
    }
}

/// Damage staged for the next frame emit.
pub(crate) struct DamageLedger {
    staged: Option<Damage>,
}

impl DamageLedger {
    /// Builds a ledger with the bootstrap repaint already staged.
    pub fn new() -> Self {
        Self {
            staged: Some(Damage::Full),
        }
    }

    /// Merges `damage` into the staged value.
    ///
    /// Seeding an absent staged value with an empty row set is safe
    /// because that set is the merge identity.
    pub fn stage(&mut self, damage: Damage) {
        *self
            .staged
            .get_or_insert(Damage::Delta(DamageRows::default())) |= damage;
    }

    /// Stages the reported damage, if any; returns whether there was
    /// any to stage.
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
    pub fn take(&mut self) -> Option<Damage> {
        self.staged.take()
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
#[allow(
    dead_code,
    reason = "DamageLedger reaches this in Task 6; cfg(test) makes the lint conditional, so #[expect] would fire unfulfilled under cargo test"
)]
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
        // NOTE: both shift amounts stay within `0..=63` by construction.
        // The natural last-word mask `!(u64::MAX << (last % 64 + 1))`
        // shifts by 64 when `last % 64 == 63`, which Rust treats as
        // arithmetic overflow and panics in a debug build.
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

        #[test]
        fn full_damage_classifies_as_full() {
            assert_eq!(DamageVerdict::classify(&Damage::Full), DamageVerdict::Full);
        }

        #[test]
        fn no_dirty_rows_classifies_as_idle() {
            assert_eq!(
                DamageVerdict::classify(&Damage::Delta(DamageRows::default())),
                DamageVerdict::Idle
            );
        }

        #[test]
        fn one_dirty_row_classifies_as_at_most_one_row() {
            assert_eq!(
                DamageVerdict::classify(&Damage::Delta(vec![ViewportLine(7)].into())),
                DamageVerdict::AtMostOneRow
            );
        }

        #[test]
        fn many_dirty_rows_carry_the_row_count() {
            assert_eq!(
                DamageVerdict::classify(&Damage::Delta(
                    vec![ViewportLine(0), ViewportLine(3), ViewportLine(9)].into()
                )),
                DamageVerdict::ManyRows { rows: 3 }
            );
        }

        /// Asserts that merging partial damage yields the ascending,
        /// duplicate-free union.
        ///
        /// Case: damage from an interpreted chunk and from a selection
        /// change meets in the staged value before one emit. A single
        /// backend read is already normalized, so this exists only for
        /// that cross-read merge; keeping append order would leave a
        /// duplicate that `classify` reports as `ManyRows` instead of
        /// `AtMostOneRow`.
        #[test]
        fn merging_partial_damage_unions_sorts_and_dedups_the_rows() {
            let mut interleaved =
                Damage::Delta(vec![ViewportLine(1), ViewportLine(3), ViewportLine(5)].into());
            interleaved |=
                Damage::Delta(vec![ViewportLine(2), ViewportLine(3), ViewportLine(5)].into());
            assert_eq!(
                interleaved,
                Damage::Delta(
                    vec![
                        ViewportLine(1),
                        ViewportLine(2),
                        ViewportLine(3),
                        ViewportLine(5)
                    ]
                    .into()
                )
            );

            let mut descending = Damage::Delta(vec![ViewportLine(5)].into());
            descending |= Damage::Delta(vec![ViewportLine(3)].into());
            assert_eq!(
                descending,
                Damage::Delta(vec![ViewportLine(3), ViewportLine(5)].into())
            );

            let mut repeated = Damage::Delta(vec![ViewportLine(0), ViewportLine(1)].into());
            repeated |= Damage::Delta(vec![ViewportLine(0), ViewportLine(1)].into());
            assert_eq!(
                repeated,
                Damage::Delta(vec![ViewportLine(0), ViewportLine(1)].into())
            );
        }

        /// Asserts that `Full` absorbs partial damage from either side.
        ///
        /// Case: a selection change demands a whole repaint, then a
        /// one-row echo arrives before the emit. Both orders are pinned
        /// because the two arms are asymmetric; letting the newest
        /// damage win would leave the screen stale.
        #[test]
        fn full_damage_absorbs_partial_damage_from_either_side() {
            let mut staged_full = Damage::Full;
            staged_full |= Damage::Delta(vec![ViewportLine(0)].into());
            assert_eq!(staged_full, Damage::Full);

            let mut incoming_full = Damage::Delta(vec![ViewportLine(0), ViewportLine(1)].into());
            incoming_full |= Damage::Full;
            assert_eq!(incoming_full, Damage::Full);

            let mut both_full = Damage::Full;
            both_full |= Damage::Full;
            assert_eq!(both_full, Damage::Full);

            let mut full_then_empty = Damage::Full;
            full_then_empty |= Damage::Delta(DamageRows::default());
            assert_eq!(full_then_empty, Damage::Full);
        }

        /// Asserts that an empty row set is the merge identity on both
        /// sides.
        ///
        /// Case: the staging site folds an absent staged value in by
        /// merging onto an empty `Delta`, so the identity is
        /// load-bearing. An empty operand is a real reading — a viewport
        /// scrolled fully into history — not a sentinel to discard the
        /// other side for.
        #[test]
        fn an_empty_row_set_is_the_merge_identity() {
            let mut empty_incoming = Damage::Delta(vec![ViewportLine(0), ViewportLine(2)].into());
            empty_incoming |= Damage::Delta(DamageRows::default());
            assert_eq!(
                empty_incoming,
                Damage::Delta(vec![ViewportLine(0), ViewportLine(2)].into())
            );

            let mut empty_staged = Damage::Delta(DamageRows::default());
            empty_staged |= Damage::Delta(vec![ViewportLine(0), ViewportLine(2)].into());
            assert_eq!(
                empty_staged,
                Damage::Delta(vec![ViewportLine(0), ViewportLine(2)].into())
            );

            let mut both_empty = Damage::Delta(DamageRows::default());
            both_empty |= Damage::Delta(DamageRows::default());
            assert_eq!(both_empty, Damage::Delta(DamageRows::default()));
        }
    }

    mod ledger {
        use super::super::*;

        /// A ledger whose seeded bootstrap repaint has been consumed, so
        /// a test observes only the damage it stages itself.
        fn drained() -> DamageLedger {
            let mut ledger = DamageLedger::new();
            ledger.take();
            ledger
        }

        /// Asserts that a freshly built ledger already has full damage
        /// staged.
        #[test]
        fn a_new_ledger_starts_with_full_damage_staged() {
            let mut ledger = DamageLedger::new();
            assert_eq!(ledger.take(), Some(Damage::Full));
        }

        /// Asserts that a second stage unions into the staged value
        /// instead of overwriting it.
        #[test]
        fn staging_merges_rather_than_replacing() {
            let mut ledger = drained();
            ledger.stage(Damage::Delta(
                vec![ViewportLine(1), ViewportLine(2), ViewportLine(3)].into(),
            ));
            ledger.stage(Damage::Delta(vec![ViewportLine(7)].into()));
            assert_eq!(
                ledger.take(),
                Some(Damage::Delta(
                    vec![
                        ViewportLine(1),
                        ViewportLine(2),
                        ViewportLine(3),
                        ViewportLine(7)
                    ]
                    .into()
                ))
            );
        }

        /// Asserts that staging an empty row set leaves real staged
        /// damage for a take to hand back, not an absent one.
        ///
        /// Case: the viewport sits fully scrolled back into history
        /// while the shell keeps writing at the live tail.
        #[test]
        fn an_empty_row_set_stays_staged_damage_rather_than_collapsing_to_nothing() {
            let mut ledger = drained();
            ledger.stage(Damage::Delta(DamageRows::default()));
            assert_eq!(ledger.take(), Some(Damage::Delta(DamageRows::default())));
        }

        /// Asserts that a take hands over the staged damage and leaves
        /// the ledger empty, so nothing it consumed reappears
        /// afterwards.
        ///
        /// Case: the coalescer's deadline fires and builds one frame,
        /// fires again with no PTY output in between, and then a later
        /// chunk dirties a different row.
        #[test]
        fn take_hands_over_the_staged_damage_and_then_reports_nothing_staged() {
            let mut ledger = drained();
            ledger.stage(Damage::Delta(vec![ViewportLine(4)].into()));
            assert_eq!(
                ledger.take(),
                Some(Damage::Delta(vec![ViewportLine(4)].into()))
            );
            assert_eq!(ledger.take(), None);

            ledger.stage(Damage::Delta(vec![ViewportLine(9)].into()));
            assert_eq!(
                ledger.take(),
                Some(Damage::Delta(vec![ViewportLine(9)].into()))
            );
        }

        /// Asserts that staging an optional damage reports whether one
        /// was present, and leaves the staged value untouched when it
        /// was not.
        ///
        /// Case: the host resizes the window to the size it already had
        /// while an earlier chunk's damage still waits for the next
        /// emit, and later the user scrolls the viewport back into
        /// scrollback history.
        #[test]
        fn stage_if_changed_reports_whether_anything_was_staged() {
            let mut ledger = drained();
            ledger.stage(Damage::Delta(vec![ViewportLine(2)].into()));
            assert!(!ledger.stage_if_changed(None));
            assert_eq!(
                ledger.take(),
                Some(Damage::Delta(vec![ViewportLine(2)].into()))
            );

            assert!(ledger.stage_if_changed(Some(Damage::Full)));
            assert_eq!(ledger.take(), Some(Damage::Full));
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
