//! Damage collection and classification driving the coalescer's
//! immediate-flush decision.

#[cfg(feature = "alacritty")]
use alacritty_terminal::{Term, term::TermDamage};
use std::ops::{BitOrAssign, Deref};

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
    /// be called exactly once per cycle. The owner must call
    /// `Term::reset_damage()` after the matching emit — without it
    /// `damage.full` latches and every later cycle reports `Full`.
    #[cfg(feature = "alacritty")]
    pub fn from_alacritty_term<T>(term: &mut Term<T>) -> Self {
        match term.damage() {
            TermDamage::Full => Self::Full,
            TermDamage::Partial(iter) => Self::Delta(iter.map(|d| d.line as u16).collect()),
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
                staged.0.extend(incoming.0);
                staged.0.sort_unstable();
                staged.0.dedup();
            }
        }
    }
}

/// Dirty viewport row indices, ascending and without duplicates.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DamageRows(Vec<u16>);

impl Deref for DamageRows {
    type Target = Vec<u16>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<Vec<u16>> for DamageRows {
    fn from(mut rows: Vec<u16>) -> Self {
        rows.sort_unstable();
        rows.dedup();
        Self(rows)
    }
}

impl FromIterator<u16> for DamageRows {
    fn from_iter<I: IntoIterator<Item = u16>>(iter: I) -> Self {
        Self::from(iter.into_iter().collect::<Vec<u16>>())
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

#[cfg(test)]
mod tests {
    use super::*;

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
            DamageVerdict::classify(&Damage::Delta(vec![7].into())),
            DamageVerdict::AtMostOneRow
        );
    }

    #[test]
    fn many_dirty_rows_carry_the_row_count() {
        assert_eq!(
            DamageVerdict::classify(&Damage::Delta(vec![0, 3, 9].into())),
            DamageVerdict::ManyRows { rows: 3 }
        );
    }

    /// Asserts that merging partial damage yields the ascending,
    /// duplicate-free union.
    ///
    /// Case: damage from an interpreted chunk and from a selection
    /// change meets in the staged value before one emit. A single
    /// backend read is already normalized, so this exists only for that
    /// cross-read merge; keeping append order would leave a duplicate
    /// that `classify` reports as `ManyRows` instead of `AtMostOneRow`.
    #[test]
    fn merging_partial_damage_unions_sorts_and_dedups_the_rows() {
        let mut interleaved = Damage::Delta(vec![1, 3, 5].into());
        interleaved |= Damage::Delta(vec![2, 3, 5].into());
        assert_eq!(interleaved, Damage::Delta(vec![1, 2, 3, 5].into()));

        let mut descending = Damage::Delta(vec![5].into());
        descending |= Damage::Delta(vec![3].into());
        assert_eq!(descending, Damage::Delta(vec![3, 5].into()));

        let mut repeated = Damage::Delta(vec![0, 1].into());
        repeated |= Damage::Delta(vec![0, 1].into());
        assert_eq!(repeated, Damage::Delta(vec![0, 1].into()));
    }

    /// Asserts that `Full` absorbs partial damage from either side.
    ///
    /// Case: a selection change demands a whole repaint, then a one-row
    /// echo arrives before the emit. Both orders are pinned because the
    /// two arms are asymmetric; letting the newest damage win would
    /// leave the screen stale.
    #[test]
    fn full_damage_absorbs_partial_damage_from_either_side() {
        let mut staged_full = Damage::Full;
        staged_full |= Damage::Delta(vec![0].into());
        assert_eq!(staged_full, Damage::Full);

        let mut incoming_full = Damage::Delta(vec![0, 1].into());
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
    /// Case: the staging site folds an absent staged value in by merging
    /// onto an empty `Delta`, so the identity is load-bearing. An empty
    /// operand is a real reading — a viewport scrolled fully into
    /// history — not a sentinel to discard the other side for.
    #[test]
    fn an_empty_row_set_is_the_merge_identity() {
        let mut empty_incoming = Damage::Delta(vec![0, 2].into());
        empty_incoming |= Damage::Delta(DamageRows::default());
        assert_eq!(empty_incoming, Damage::Delta(vec![0, 2].into()));

        let mut empty_staged = Damage::Delta(DamageRows::default());
        empty_staged |= Damage::Delta(vec![0, 2].into());
        assert_eq!(empty_staged, Damage::Delta(vec![0, 2].into()));

        let mut both_empty = Damage::Delta(DamageRows::default());
        both_empty |= Damage::Delta(DamageRows::default());
        assert_eq!(both_empty, Damage::Delta(DamageRows::default()));
    }
}

#[cfg(all(test, feature = "alacritty"))]
mod alacritty_tests {
    use super::*;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::grid::Dimensions;
    use alacritty_terminal::term::Config;
    use alacritty_terminal::vte::ansi::Processor;

    /// Grid size for the fixtures below.
    ///
    /// `total_lines == screen_lines` on purpose: scrollback capacity comes
    /// from `Config::scrolling_history`, not from the size type.
    struct TestDim;

    impl Dimensions for TestDim {
        fn columns(&self) -> usize {
            80
        }

        fn screen_lines(&self) -> usize {
            24
        }

        fn total_lines(&self) -> usize {
            24
        }
    }

    fn fresh_term() -> Term<VoidListener> {
        Term::new(Config::default(), &TestDim, VoidListener)
    }

    // NOTE: a fresh `Term` starts fully damaged (`TermDamageState::new` sets
    // `full: true` for the bootstrap paint). The reset clears it so each test
    // observes only the damage its own bytes produced.
    fn term_after(bytes: &[u8]) -> Term<VoidListener> {
        let mut term = fresh_term();
        term.reset_damage();
        let mut processor: Processor = Processor::new();
        processor.advance(&mut term, bytes);
        term
    }

    #[test]
    fn a_fresh_terminal_reports_full_damage() {
        assert_eq!(Damage::from_alacritty_term(&mut fresh_term()), Damage::Full);
    }

    #[test]
    fn printing_text_damages_the_cursor_row() {
        let mut term = term_after(b"hi");
        assert_eq!(
            Damage::from_alacritty_term(&mut term),
            Damage::Delta(vec![0].into())
        );
    }

    #[test]
    fn each_written_line_is_reported_dirty() {
        let mut term = term_after(b"one\r\ntwo\r\nthree");
        assert_eq!(
            Damage::from_alacritty_term(&mut term),
            Damage::Delta(vec![0, 1, 2].into())
        );
    }

    #[test]
    fn insert_mode_reports_full_damage() {
        let mut term = term_after(b"\x1b[4h");
        assert_eq!(Damage::from_alacritty_term(&mut term), Damage::Full);
    }

    #[test]
    fn reset_damage_clears_the_accumulator() {
        let mut term = term_after(b"one\r\ntwo\r\nthree");
        assert_eq!(
            Damage::from_alacritty_term(&mut term),
            Damage::Delta(vec![0, 1, 2].into())
        );
        term.reset_damage();
        assert_eq!(
            Damage::from_alacritty_term(&mut term),
            Damage::Delta(vec![2].into())
        );
    }
}
