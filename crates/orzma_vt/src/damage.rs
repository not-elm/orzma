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
    pub fn classify(dirty: &TermDamage, cursor_changed: bool) -> Self {
        match dirty {
            TermDamage::Full => DamageVerdict::Full,
            TermDamage::Partial(rows) if rows. => {
                if cursor_changed {
                    DamageVerdict::AtMostOneRow
                } else {
                    DamageVerdict::Idle
                }
            }
            TermDamage::Partial(rows) if rows.len() <= 1 => DamageVerdict::AtMostOneRow,
            TermDamage::Partial(rows) => DamageVerdict::ManyRows { rows: rows.len() },
        }
    }
}
