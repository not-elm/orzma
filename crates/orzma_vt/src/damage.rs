//! Central staging point for the damage every source reports.
//!
//! [`DamageLedger`] accumulates the [`Damage`] that interpretation,
//! scrolling, resizing, and placement changes each report per call, and
//! hands the merged result to the frame emitter. Staging merges rather
//! than replaces: a source reports only what its own call produced, so
//! an overwritten staged value would drop a repaint no later call
//! re-reports.

use crate::schema::{Damage, DamageRows};

// NOTE: `#[expect]` is impractical on this type — the tests below
// construct the ledger and call every method, so `dead_code` fires in
// the lib build but not in the test build, leaving the expectation
// unfulfilled there.
/// Damage staged for the next frame emit.
#[allow(
    dead_code,
    reason = "the executor and the frame emitter reach the ledger once they land"
)]
pub(crate) struct DamageLedger {
    staged: Option<Damage>,
}

#[allow(
    dead_code,
    reason = "the executor and the frame emitter reach the ledger once they land"
)]
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::DamageRows;

    /// A ledger whose seeded bootstrap repaint has been consumed, so a
    /// test observes only the damage it stages itself.
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

    /// Asserts that a second stage unions into the staged value instead
    /// of overwriting it.
    #[test]
    fn staging_merges_rather_than_replacing() {
        let mut ledger = drained();
        ledger.stage(Damage::Delta(vec![1, 2, 3].into()));
        ledger.stage(Damage::Delta(vec![7].into()));
        assert_eq!(ledger.take(), Some(Damage::Delta(vec![1, 2, 3, 7].into())));
    }

    /// Asserts that staging an empty row set leaves real staged damage
    /// for a take to hand back, not an absent one.
    ///
    /// Case: the viewport sits fully scrolled back into history while
    /// the shell keeps writing at the live tail.
    #[test]
    fn an_empty_row_set_stays_staged_damage_rather_than_collapsing_to_nothing() {
        let mut ledger = drained();
        ledger.stage(Damage::Delta(DamageRows::default()));
        assert_eq!(ledger.take(), Some(Damage::Delta(DamageRows::default())));
    }

    /// Asserts that a take hands over the staged damage and leaves the
    /// ledger empty, so nothing it consumed reappears afterwards.
    ///
    /// Case: the coalescer's deadline fires and builds one frame, fires
    /// again with no PTY output in between, and then a later chunk
    /// dirties a different row.
    #[test]
    fn take_hands_over_the_staged_damage_and_then_reports_nothing_staged() {
        let mut ledger = drained();
        ledger.stage(Damage::Delta(vec![4].into()));
        assert_eq!(ledger.take(), Some(Damage::Delta(vec![4].into())));
        assert_eq!(ledger.take(), None);

        ledger.stage(Damage::Delta(vec![9].into()));
        assert_eq!(ledger.take(), Some(Damage::Delta(vec![9].into())));
    }

    /// Asserts that staging an optional damage reports whether one was
    /// present, and leaves the staged value untouched when it was not.
    ///
    /// Case: the host resizes the window to the size it already had
    /// while an earlier chunk's damage still waits for the next emit,
    /// and later the user scrolls the viewport back into scrollback
    /// history.
    #[test]
    fn stage_if_changed_reports_whether_anything_was_staged() {
        let mut ledger = drained();
        ledger.stage(Damage::Delta(vec![2].into()));
        assert!(!ledger.stage_if_changed(None));
        assert_eq!(ledger.take(), Some(Damage::Delta(vec![2].into())));

        assert!(ledger.stage_if_changed(Some(Damage::Full)));
        assert_eq!(ledger.take(), Some(Damage::Full));
    }
}
