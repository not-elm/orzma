use alacritty_terminal::{
    Term,
    term::{TermDamage, cell::Cell},
};

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
    pub fn classify(term: &mut Term<Cell>, cursor_changed: bool) -> Self {
        match term.damage() {
            TermDamage::Full => DamageVerdict::Full,
            TermDamage::Partial(rows) => {
                let rows = rows.collect::<Vec<_>>();
                match rows {
                    _ if rows.is_empty() => {
                        if cursor_changed {
                            DamageVerdict::AtMostOneRow
                        } else {
                            DamageVerdict::Idle
                        }
                    }
                    _ if rows.len() <= 1 => DamageVerdict::AtMostOneRow,
                    _ => DamageVerdict::ManyRows { rows: rows.len() },
                }
            }
        }
    }
}
