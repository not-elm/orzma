//! Damage collection and classification driving the coalescer's
//! immediate-flush decision.

/// Rows the VT reported dirty in a single damage cycle.
///
/// Backend-agnostic: the alacritty-specific reader lives with the
/// backend as `DirtyRows::from_term`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirtyRows {
    /// Entire viewport is dirty (resize, clear, alt-screen swap, reset).
    Full,
    /// Viewport row indices that changed, ascending.
    Rows(Vec<u16>),
}

impl DirtyRows {
    /// Reads alacritty's accumulated damage for one cycle.
    ///
    /// # Invariants
    ///
    /// `Term::damage()` consumes its own `last_cursor` bookkeeping, so it must
    /// be called exactly once per cycle. The owner must call
    /// `Term::reset_damage()` after the matching emit — without it
    /// `damage.full` latches and every later cycle reports `Full`.
    #[cfg(feature = "alacritty")]
    pub fn from_alacritty_term<T>(term: &mut alacritty_terminal::Term<T>) -> Self {
        use alacritty_terminal::term::TermDamage;
        match term.damage() {
            TermDamage::Full => Self::Full,
            TermDamage::Partial(iter) => Self::Rows(iter.map(|d| d.line as u16).collect()),
        }
    }
}

/// Classification of collected damage that drives the immediate-flush decision.
///
/// The owner classifies once per interpreted chunk and keeps the matching
/// [`DirtyRows`] staged for the emit, so the backend's damage tracker is
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
    pub fn classify(dirty: &DirtyRows) -> Self {
        match dirty {
            DirtyRows::Full => Self::Full,
            DirtyRows::Rows(rows) => match rows.len() {
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
        assert_eq!(
            DamageVerdict::classify(&DirtyRows::Full),
            DamageVerdict::Full
        );
    }

    #[test]
    fn no_dirty_rows_classifies_as_idle() {
        assert_eq!(
            DamageVerdict::classify(&DirtyRows::Rows(Vec::new())),
            DamageVerdict::Idle
        );
    }

    #[test]
    fn one_dirty_row_classifies_as_at_most_one_row() {
        assert_eq!(
            DamageVerdict::classify(&DirtyRows::Rows(vec![7])),
            DamageVerdict::AtMostOneRow
        );
    }

    #[test]
    fn many_dirty_rows_carry_the_row_count() {
        assert_eq!(
            DamageVerdict::classify(&DirtyRows::Rows(vec![0, 3, 9])),
            DamageVerdict::ManyRows { rows: 3 }
        );
    }
}

#[cfg(all(test, feature = "alacritty"))]
mod alacritty_tests {
    use super::*;
    use alacritty_terminal::Term;
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
        assert_eq!(
            DirtyRows::from_alacritty_term(&mut fresh_term()),
            DirtyRows::Full
        );
    }

    #[test]
    fn printing_text_damages_the_cursor_row() {
        let mut term = term_after(b"hi");
        assert_eq!(
            DirtyRows::from_alacritty_term(&mut term),
            DirtyRows::Rows(vec![0])
        );
    }

    #[test]
    fn each_written_line_is_reported_dirty() {
        let mut term = term_after(b"one\r\ntwo\r\nthree");
        assert_eq!(
            DirtyRows::from_alacritty_term(&mut term),
            DirtyRows::Rows(vec![0, 1, 2])
        );
    }

    #[test]
    fn insert_mode_reports_full_damage() {
        let mut term = term_after(b"\x1b[4h");
        assert_eq!(DirtyRows::from_alacritty_term(&mut term), DirtyRows::Full);
    }

    #[test]
    fn reset_damage_clears_the_accumulator() {
        let mut term = term_after(b"one\r\ntwo\r\nthree");
        assert_eq!(
            DirtyRows::from_alacritty_term(&mut term),
            DirtyRows::Rows(vec![0, 1, 2])
        );
        term.reset_damage();
        assert_eq!(
            DirtyRows::from_alacritty_term(&mut term),
            DirtyRows::Rows(vec![2])
        );
    }
}
