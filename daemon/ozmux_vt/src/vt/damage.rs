//! Damage classification and dirty-row collection for the VT emit pipeline.

use alacritty_terminal::{Term, term::TermDamage};

/// Classification of accumulated damage that drives the immediate-flush decision.
/// The bridge constructs this once per pre-emit decision (via `Term::damage()`)
/// and reuses it for the actual emit so `Term::damage()` is never called twice
/// without an intervening `reset_damage()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DamageVerdict {
    /// Entire screen damaged (resize, clear, alt-screen swap).
    Full,
    /// At most one row is dirty (interactive echo / cursor-only motion).
    AtMostOneRow,
    /// Two or more rows dirty. The row count drives the PR-E2b
    /// immediate-flush cap in `Coalescer::should_flush_immediately`.
    ManyRows { rows: usize },
    /// No rows dirty and cursor unchanged.
    Idle,
}

impl DamageVerdict {
    /// Classifies the bridge's accumulated damage for the Coalescer's
    /// immediate-flush decision. The cursor delta is folded in so that
    /// cursor-only motion (no dirty rows) counts as `AtMostOneRow`.
    pub fn classify_damage(dirty: &DirtyRows, cursor_changed: bool) -> Self {
        match dirty {
            DirtyRows::Full => DamageVerdict::Full,
            DirtyRows::Rows(rows) if rows.is_empty() => {
                if cursor_changed {
                    DamageVerdict::AtMostOneRow
                } else {
                    DamageVerdict::Idle
                }
            }
            DirtyRows::Rows(rows) if rows.len() <= 1 => DamageVerdict::AtMostOneRow,
            DirtyRows::Rows(rows) => DamageVerdict::ManyRows { rows: rows.len() },
        }
    }
}

/// Outcome of damage inspection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirtyRows {
    /// Entire screen is dirty (resize / alt-screen swap / clear / reset).
    Full,
    /// Specific row indices are dirty.
    Rows(Vec<u16>),
}

impl DirtyRows {
    /// Reads the damage tracker and returns row indices that changed.
    ///
    /// `&mut Term` is required because `Term::damage()` takes `&mut self`.
    /// `scratch_dirty` is cleared, filled with the dirty row indices, then
    /// moved into the returned `DirtyRows::Rows` variant via `mem::take`.
    /// The caller should reclaim the consumed `Vec` back into the scratch field
    /// after the emit completes so capacity persists across calls.
    pub fn collect<T>(term: &mut Term<T>, scratch_dirty: &mut Vec<u16>) -> DirtyRows {
        match term.damage() {
            TermDamage::Full => DirtyRows::Full,
            TermDamage::Partial(iter) => {
                scratch_dirty.clear();
                scratch_dirty.extend(iter.map(|d| d.line as u16));
                DirtyRows::Rows(std::mem::take(scratch_dirty))
            }
        }
    }
}
