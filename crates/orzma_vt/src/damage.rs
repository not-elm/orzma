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
