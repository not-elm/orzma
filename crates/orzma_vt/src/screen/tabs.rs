//! Character tabulation stops: the columns HT, CHT, and CBT navigate.
#![expect(
    dead_code,
    reason = "every stop edit reaches this table once the CSI dispatch lands"
)]

use crate::screen::grid::coords::GridColumn;

/// One character tabulation stop edit, independent of which control
/// function asked for it.
///
/// TBC and CTC number their parameters differently: `TBC 3` and
/// `CTC 5` both mean "clear every character stop".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CharacterTabEdit {
    /// Set a stop at the cursor column (HTS, CTC 0).
    SetColumn,
    /// Clear the stop at the cursor column (TBC 0, CTC 2).
    ClearColumn,
    /// Clear every stop (TBC 2, 3, and 5; CTC 4 and 5).
    ClearAllColumns,
}

impl CharacterTabEdit {
    /// The edit a `TBC` (`CSI Ps g`) parameter selects; `None` for a
    /// value only line tabulation stops answer.
    pub fn from_tbc(ps: u16) -> Option<Self> {
        match ps {
            0 => Some(Self::ClearColumn),
            2 | 3 | 5 => Some(Self::ClearAllColumns),
            _ => None,
        }
    }

    /// The edit a `CTC` (`CSI Ps W`) parameter selects; `None` for a
    /// value only line tabulation stops answer.
    pub fn from_ctc(ps: u16) -> Option<Self> {
        match ps {
            0 => Some(Self::SetColumn),
            2 => Some(Self::ClearColumn),
            4 | 5 => Some(Self::ClearAllColumns),
            _ => None,
        }
    }
}

/// One screen's character tabulation stops.
///
/// Every line shares one set of stops: this type implements only the
/// reset state, MULTIPLE, of ECMA-48's TABULATION STOP MODE (§ 7.2.17).
///
/// # Invariants
///
/// A resize never touches the table: within its `COLUMN_COUNT`
/// columns, the columns a widening grid gains already hold the stops a
/// reset installed, or none after a `TBC 3`.
///
/// A stop an application sets on the alternate screen never reaches the
/// primary.
#[derive(Debug, PartialEq)]
pub(super) struct TabStops([u64; TabStops::WORDS]);

impl TabStops {
    /// Columns between the stops a device reset installs.
    const DEFAULT_INTERVAL: u16 = 8;
    /// How many columns the table addresses, i.e. `0..COLUMN_COUNT`.
    const COLUMN_COUNT: u16 = 4096;
    const BITS_PER_WORD: usize = u64::BITS as usize;
    const WORDS: usize = Self::COLUMN_COUNT as usize / Self::BITS_PER_WORD;

    /// Sets the stop at `column` (HTS, CTC 0).
    pub fn set(&mut self, column: GridColumn) {
        if let Some((word, mask)) = Self::word_and_mask(column) {
            self.0[word] |= mask;
        }
    }

    /// Clears the stop at `column` (TBC 0, CTC 2).
    pub fn clear(&mut self, column: GridColumn) {
        if let Some((word, mask)) = Self::word_and_mask(column) {
            self.0[word] &= !mask;
        }
    }

    /// Clears every stop (TBC 2, 3, and 5; CTC 4 and 5).
    pub fn clear_all(&mut self) {
        self.0 = [0; Self::WORDS];
    }

    /// Reinstalls the default stride across the whole table (DECST8C,
    /// RIS).
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// The first stop past `column`, clamped to `right_edge` when there
    /// is none.
    pub fn ht(&self, column: GridColumn, right_edge: GridColumn) -> GridColumn {
        self.cht(column, 1, right_edge)
    }

    /// The column `count` stops forward of `column`, clamped to
    /// `right_edge` when the stops run out.
    pub fn cht(&self, column: GridColumn, count: u16, right_edge: GridColumn) -> GridColumn {
        let mut current = column.0;
        for _ in 0..count {
            match self.next_stop(current) {
                Some(stop) if stop <= right_edge.0 => current = stop,
                _ => return GridColumn(current.max(right_edge.0)),
            }
        }
        GridColumn(current)
    }

    /// The column `count` stops back from `column`, clamped to
    /// `left_edge` when the stops run out.
    pub fn cbt(&self, column: GridColumn, count: u16, left_edge: GridColumn) -> GridColumn {
        let mut current = column.0;
        for _ in 0..count {
            match self.previous_stop(current) {
                Some(stop) if stop >= left_edge.0 => current = stop,
                _ => return GridColumn(current.min(left_edge.0)),
            }
        }
        GridColumn(current)
    }

    /// Whether `column` carries a stop.
    fn is_set(&self, column: GridColumn) -> bool {
        Self::word_and_mask(column).is_some_and(|(word, mask)| self.0[word] & mask != 0)
    }

    /// The lowest column above `column` that carries a stop.
    fn next_stop(&self, column: u16) -> Option<u16> {
        let start = usize::from(column.checked_add(1)?);
        if usize::from(Self::COLUMN_COUNT) <= start {
            return None;
        }
        let mut word = start / Self::BITS_PER_WORD;
        let mut bits = self.0[word] & (!0u64 << (start % Self::BITS_PER_WORD));
        while bits == 0 {
            word += 1;
            if Self::WORDS <= word {
                return None;
            }
            bits = self.0[word];
        }
        let index = word * Self::BITS_PER_WORD + bits.trailing_zeros() as usize;
        Some(u16::try_from(index).expect("the table addresses fewer columns than a u16 holds"))
    }

    /// The highest column below `column` that carries a stop.
    fn previous_stop(&self, column: u16) -> Option<u16> {
        let last = usize::from(column)
            .min(usize::from(Self::COLUMN_COUNT))
            .checked_sub(1)?;
        let mut word = last / Self::BITS_PER_WORD;
        let top = last % Self::BITS_PER_WORD;
        let keep = if top == Self::BITS_PER_WORD - 1 {
            !0u64
        } else {
            !(!0u64 << (top + 1))
        };
        let mut bits = self.0[word] & keep;
        while bits == 0 {
            word = word.checked_sub(1)?;
            bits = self.0[word];
        }
        let index =
            word * Self::BITS_PER_WORD + (Self::BITS_PER_WORD - 1 - bits.leading_zeros() as usize);
        Some(u16::try_from(index).expect("the table addresses fewer columns than a u16 holds"))
    }

    /// The word holding `column`'s bit and the mask selecting it, or
    /// `None` when the table does not address the column.
    fn word_and_mask(column: GridColumn) -> Option<(usize, u64)> {
        if column.0 >= Self::COLUMN_COUNT {
            return None;
        }
        let index = usize::from(column.0);
        Some((
            index / Self::BITS_PER_WORD,
            1u64 << (index % Self::BITS_PER_WORD),
        ))
    }
}

impl Default for TabStops {
    /// The stops a freshly reset device has: one every eight columns,
    /// starting at column eight.
    fn default() -> Self {
        let mut stops = Self([0; Self::WORDS]);
        let mut column = Self::DEFAULT_INTERVAL;
        while column < Self::COLUMN_COUNT {
            stops.set(GridColumn(column));
            column += Self::DEFAULT_INTERVAL;
        }
        stops
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tabs() -> TabStops {
        TabStops::default()
    }

    /// The right edge of an eighty-column grid.
    const EDGE_80: GridColumn = GridColumn(79);
    /// The right edge of the same grid widened to a hundred and twenty
    /// columns.
    const EDGE_120: GridColumn = GridColumn(119);

    mod default {
        use super::*;

        /// Asserts that a reset device carries a stop every eight
        /// columns.
        ///
        /// Case: a terminal spawns and the shell emits tab-aligned
        /// output such as a bare `ls`, before any application has
        /// touched the tab table.
        #[test]
        fn a_reset_device_has_a_stop_every_eight_columns() {
            let tabs = tabs();
            for column in [8, 16, 24, 32] {
                assert!(tabs.is_set(GridColumn(column)));
            }
            for column in [1, 7, 9, 15, 17] {
                assert!(!tabs.is_set(GridColumn(column)));
            }
        }

        /// Asserts that the reset stride starts at column eight, leaving
        /// column zero without a stop.
        ///
        /// Case: the user presses Shift-Tab with the cursor at the
        /// start of a line.
        #[test]
        fn a_reset_device_has_no_stop_at_column_zero() {
            assert!(!tabs().is_set(GridColumn(0)));
        }

        /// Asserts that the reset stride covers the whole table, reaching
        /// columns no ordinary grid shows, rather than stopping at the
        /// grid's right edge.
        ///
        /// Case: the user starts an eighty-column terminal and later
        /// drags the window out to two hundred columns.
        #[test]
        fn the_default_stride_covers_columns_past_the_initial_grid_width() {
            let tabs = tabs();
            assert!(tabs.is_set(GridColumn(80)));
            assert!(tabs.is_set(GridColumn(1000)));
            assert!(tabs.is_set(GridColumn(
                TabStops::COLUMN_COUNT - TabStops::DEFAULT_INTERVAL
            )));
        }
    }

    mod set {
        use super::*;

        /// Asserts that a set stop becomes the column a forward search
        /// finds.
        ///
        /// Case: an application lays out a report by installing its own
        /// tab position and then tabbing to it.
        #[test]
        fn set_adds_a_stop_the_forward_search_finds() {
            let mut tabs = tabs();
            tabs.set(GridColumn(5));
            assert_eq!(tabs.cht(GridColumn(0), 1, EDGE_80), GridColumn(5));
        }

        /// Asserts that setting a column that already carries a stop
        /// leaves the rest of the table alone.
        ///
        /// Case: a shell startup script re-sends HTS at the positions
        /// it expects, over a table that already holds them.
        #[test]
        fn setting_an_existing_stop_leaves_the_set_alone() {
            let mut tabs = tabs();
            tabs.set(GridColumn(8));
            assert!(tabs.is_set(GridColumn(8)));
            assert!(tabs.is_set(GridColumn(16)));
            assert!(!tabs.is_set(GridColumn(9)));
        }

        /// Asserts that a set past the table's last column is dropped
        /// silently rather than growing the table or panicking.
        ///
        /// Case: a grid wider than the table puts the cursor past its
        /// last column when HTS arrives.
        #[test]
        fn a_set_past_the_table_is_dropped() {
            let mut tabs = tabs();
            tabs.set(GridColumn(TabStops::COLUMN_COUNT));
            assert!(!tabs.is_set(GridColumn(TabStops::COLUMN_COUNT)));
            assert!(tabs.is_set(GridColumn(8)));
        }

        /// Asserts that the table's last column can carry a stop.
        ///
        /// Case: an application installs a tab position at the highest
        /// column the table addresses.
        #[test]
        fn the_last_addressable_column_carries_a_stop() {
            let mut tabs = tabs();
            tabs.set(GridColumn(TabStops::COLUMN_COUNT - 1));
            assert!(tabs.is_set(GridColumn(TabStops::COLUMN_COUNT - 1)));
        }

        /// Asserts that a stop at column zero is never the answer to a
        /// forward search.
        ///
        /// Case: an application clears the table and sends HTS with the
        /// cursor still at the start of the line.
        #[test]
        fn a_stop_set_at_column_zero_is_never_reached_by_a_forward_search() {
            let mut tabs = tabs();
            tabs.clear_all();
            tabs.set(GridColumn(0));
            assert_eq!(tabs.cht(GridColumn(0), 1, EDGE_80), EDGE_80);
        }
    }

    mod clear {
        use super::*;

        /// Asserts that clearing a column removes the stop a forward
        /// search would have found there.
        ///
        /// Case: an application thins the default stride out by one
        /// position.
        #[test]
        fn clear_removes_the_stop_at_the_column() {
            let mut tabs = tabs();
            tabs.clear(GridColumn(8));
            assert_eq!(tabs.cht(GridColumn(0), 1, EDGE_80), GridColumn(16));
        }

        /// Asserts that clearing a column with no stop leaves the rest
        /// of the table alone.
        ///
        /// Case: TBC arrives while the cursor sits between two tab
        /// positions.
        #[test]
        fn clearing_a_column_with_no_stop_leaves_the_set_alone() {
            let mut tabs = tabs();
            tabs.clear(GridColumn(5));
            assert!(tabs.is_set(GridColumn(8)));
            assert!(tabs.is_set(GridColumn(16)));
        }

        /// Asserts that a clear past the table's last column is dropped
        /// rather than panicking.
        ///
        /// Case: a grid wider than the table puts the cursor past its
        /// last column when TBC arrives.
        #[test]
        fn a_clear_past_the_table_is_dropped() {
            let mut tabs = tabs();
            tabs.clear(GridColumn(TabStops::COLUMN_COUNT));
            assert!(tabs.is_set(GridColumn(8)));
        }
    }

    mod clear_all {
        use super::*;

        /// Asserts that clearing every stop empties the columns past
        /// the grid as well as the ones it shows.
        ///
        /// Case: an application sends TBC 3 to take the tab table over,
        /// and the user then widens the window.
        #[test]
        fn clear_all_empties_the_table_past_the_grid_as_well() {
            let mut tabs = tabs();
            tabs.clear_all();
            assert!(!tabs.is_set(GridColumn(8)));
            assert!(!tabs.is_set(GridColumn(1000)));
        }

        /// Asserts that a stop set after a full clear is the only one a
        /// forward search can find.
        ///
        /// Case: an application clears the table and installs a single
        /// tab position of its own.
        #[test]
        fn a_stop_set_after_clear_all_is_the_only_one() {
            let mut tabs = tabs();
            tabs.clear_all();
            tabs.set(GridColumn(10));
            assert_eq!(tabs.cht(GridColumn(0), 1, EDGE_80), GridColumn(10));
            assert_eq!(tabs.cht(GridColumn(10), 1, EDGE_80), EDGE_80);
        }
    }

    mod reset {
        use super::*;

        /// Asserts that a reset reinstalls the stride across the whole
        /// table, including the columns past the grid's right edge.
        ///
        /// Case: an application that cleared the table sends DECST8C to
        /// get the default tab positions back.
        #[test]
        fn reset_reinstalls_the_stride_across_the_whole_table() {
            let mut tabs = tabs();
            tabs.clear_all();
            tabs.reset();
            assert!(tabs.is_set(GridColumn(8)));
            assert!(tabs.is_set(GridColumn(1000)));
        }

        /// Asserts that a reset drops the stops the application added.
        ///
        /// Case: one application leaves custom tab positions behind and
        /// the next one issues a hard reset before drawing.
        #[test]
        fn reset_drops_the_stops_the_application_added() {
            let mut tabs = tabs();
            tabs.set(GridColumn(5));
            tabs.reset();
            assert!(!tabs.is_set(GridColumn(5)));
        }
    }

    mod forward {
        use super::*;

        /// Asserts that a forward search moves to the next stop.
        ///
        /// Case: the shell emits a tab at the start of a line.
        #[test]
        fn forward_moves_to_the_next_stop() {
            assert_eq!(tabs().cht(GridColumn(0), 1, EDGE_80), GridColumn(8));
        }

        /// Asserts that a forward search starting on a stop moves to
        /// the following one.
        ///
        /// Case: a column of tab-aligned output emits a second tab with
        /// the cursor already parked on a tab position.
        #[test]
        fn forward_from_a_stop_moves_to_the_following_one() {
            assert_eq!(tabs().cht(GridColumn(8), 1, EDGE_80), GridColumn(16));
        }

        /// Asserts that a forward search counts the stops it is asked
        /// for.
        ///
        /// Case: an application emits CHT with a parameter of three to
        /// skip two tab positions.
        #[test]
        fn forward_counts_multiple_stops() {
            assert_eq!(tabs().cht(GridColumn(0), 3, EDGE_80), GridColumn(24));
        }

        /// Asserts that a forward search past the last stop the grid
        /// shows lands on the right edge rather than wrapping to the next
        /// line or refusing the move.
        ///
        /// Case: the shell emits a tab with the cursor already past the
        /// last tab position an eighty-column screen shows.
        #[test]
        fn forward_past_the_last_stop_clamps_to_the_right_edge() {
            assert_eq!(tabs().cht(GridColumn(72), 1, EDGE_80), EDGE_80);
        }

        /// Asserts that a forward search with no stop anywhere ahead
        /// lands on the right edge.
        ///
        /// Case: an application clears every stop and then emits a tab.
        #[test]
        fn forward_with_no_stop_ahead_clamps_to_the_right_edge() {
            let mut tabs = tabs();
            tabs.clear_all();
            assert_eq!(tabs.cht(GridColumn(0), 1, EDGE_80), EDGE_80);
        }

        /// Asserts that a stop sitting exactly on the right edge is
        /// reachable.
        ///
        /// Case: the user sizes the window so its last column falls on
        /// a tab position, and the shell emits a tab just before it.
        #[test]
        fn forward_finds_a_stop_exactly_at_the_right_edge() {
            assert_eq!(
                tabs().cht(GridColumn(72), 1, GridColumn(80)),
                GridColumn(80)
            );
        }

        /// Asserts that a forward search from the right edge stays put.
        ///
        /// Case: the shell emits a tab with the cursor already parked
        /// on the last column of the screen.
        #[test]
        fn a_forward_at_the_right_edge_does_not_move() {
            assert_eq!(tabs().cht(EDGE_80, 1, EDGE_80), EDGE_80);
        }

        /// Asserts that a forward search counting no stops leaves the
        /// column alone rather than moving one stop.
        ///
        /// Case: the parameter layer hands down a count it has not
        /// defaulted.
        #[test]
        fn a_zero_count_forward_does_not_move() {
            assert_eq!(tabs().cht(GridColumn(3), 0, EDGE_80), GridColumn(3));
        }
    }

    mod backward {
        use super::*;

        /// Asserts that a backward search moves to the previous stop.
        ///
        /// Case: the user presses Shift-Tab to step back to the
        /// previous column of a form.
        #[test]
        fn backward_moves_to_the_previous_stop() {
            assert_eq!(tabs().cbt(GridColumn(20), 1, GridColumn(0)), GridColumn(16));
        }

        /// Asserts that a backward search starting on a stop moves to
        /// the preceding one.
        ///
        /// Case: the user presses Shift-Tab twice in a row and the
        /// cursor is already parked on a tab position.
        #[test]
        fn backward_from_a_stop_moves_to_the_preceding_one() {
            assert_eq!(tabs().cbt(GridColumn(16), 1, GridColumn(0)), GridColumn(8));
        }

        /// Asserts that a backward search counts the stops it is asked
        /// for.
        ///
        /// Case: an application emits CBT with a parameter of two to
        /// step back over one tab position.
        #[test]
        fn backward_counts_multiple_stops() {
            assert_eq!(tabs().cbt(GridColumn(24), 2, GridColumn(0)), GridColumn(8));
        }

        /// Asserts that a backward search past the first stop falls back
        /// to the left edge.
        ///
        /// Case: the user presses Shift-Tab near the start of a line,
        /// before the first tab position.
        #[test]
        fn backward_past_the_first_stop_clamps_to_the_left_edge() {
            assert_eq!(tabs().cbt(GridColumn(5), 1, GridColumn(0)), GridColumn(0));
        }

        /// Asserts that a backward search from the left edge stays put.
        ///
        /// Case: the user presses Shift-Tab with the cursor at the
        /// start of a line.
        #[test]
        fn a_backward_at_the_left_edge_does_not_move() {
            assert_eq!(tabs().cbt(GridColumn(0), 1, GridColumn(0)), GridColumn(0));
        }

        /// Asserts that a backward search stops at a left edge the
        /// caller places away from column zero.
        ///
        /// Case: an application sets a left margin and the user presses
        /// Shift-Tab inside it.
        #[test]
        fn backward_clamps_to_a_non_zero_left_edge() {
            assert_eq!(
                tabs().cbt(GridColumn(16), 1, GridColumn(10)),
                GridColumn(10)
            );
        }

        /// Asserts that a stop sitting exactly on a non-zero left edge
        /// is reachable.
        ///
        /// Case: an application places a left margin on a tab position
        /// and the user steps back to it.
        #[test]
        fn backward_finds_a_stop_exactly_at_a_non_zero_left_edge() {
            assert_eq!(tabs().cbt(GridColumn(16), 1, GridColumn(8)), GridColumn(8));
        }

        /// Asserts that a backward search starting past the table finds
        /// the highest stop the table holds.
        ///
        /// Case: a grid wider than the table puts the cursor past its
        /// last column when CBT arrives.
        #[test]
        fn backward_from_beyond_the_table_finds_the_last_stored_stop() {
            assert_eq!(
                tabs().cbt(GridColumn(5000), 1, GridColumn(0)),
                GridColumn(TabStops::COLUMN_COUNT - TabStops::DEFAULT_INTERVAL)
            );
        }

        /// Asserts that a backward search counting no stops leaves the
        /// column alone rather than moving one stop.
        ///
        /// Case: the parameter layer hands down a count it has not
        /// defaulted.
        #[test]
        fn a_zero_count_backward_does_not_move() {
            assert_eq!(tabs().cbt(GridColumn(20), 0, GridColumn(0)), GridColumn(20));
        }
    }

    mod width_independence {
        use super::*;

        /// Asserts that widening the grid reaches stops the table
        /// already held, with no resize call touching the table.
        ///
        /// Case: the user drags an eighty-column window out to a
        /// hundred and twenty columns while the shell keeps emitting
        /// tab-aligned output.
        #[test]
        fn a_widened_edge_reaches_the_stops_that_were_already_there() {
            let tabs = tabs();
            assert_eq!(tabs.cht(GridColumn(72), 1, EDGE_80), EDGE_80);
            assert_eq!(tabs.cht(GridColumn(72), 1, EDGE_120), GridColumn(80));
        }

        /// Asserts that widening the grid after a full clear finds no
        /// stop in the columns it gains.
        ///
        /// Case: an application clears every stop to lay out its own
        /// columns, and the user then widens the window.
        #[test]
        fn a_widened_edge_stays_empty_after_every_stop_was_cleared() {
            let mut tabs = tabs();
            tabs.clear_all();
            assert_eq!(tabs.cht(GridColumn(72), 1, EDGE_120), EDGE_120);
        }

        /// Asserts that adding one stop leaves the stride past the
        /// right edge intact.
        ///
        /// Case: an application adds one tab position of its own to the
        /// default stride, and the user later widens the window.
        #[test]
        fn an_added_stop_does_not_disable_the_stride_past_the_edge() {
            let mut tabs = tabs();
            tabs.set(GridColumn(5));
            assert_eq!(tabs.cht(GridColumn(72), 1, EDGE_120), GridColumn(80));
        }
    }

    mod character_tab_edit {
        use super::*;

        /// Asserts that `TBC 0` selects the single-column clear.
        ///
        /// Case: an application drops the tab position the cursor
        /// currently sits on.
        #[test]
        fn tbc_zero_selects_the_column_clear() {
            assert_eq!(
                CharacterTabEdit::from_tbc(0),
                Some(CharacterTabEdit::ClearColumn)
            );
        }

        /// Asserts that `TBC 2` and `TBC 3` both select the full clear,
        /// rather than `TBC 2` doing nothing.
        ///
        /// Case: an application clears the tab table before installing
        /// its own layout.
        #[test]
        fn tbc_two_and_three_both_select_the_full_clear() {
            assert_eq!(
                CharacterTabEdit::from_tbc(2),
                Some(CharacterTabEdit::ClearAllColumns)
            );
            assert_eq!(
                CharacterTabEdit::from_tbc(3),
                Some(CharacterTabEdit::ClearAllColumns)
            );
        }

        /// Asserts that `TBC 5` selects the full clear.
        ///
        /// Case: an application asks for every tabulation stop of
        /// either kind to be dropped.
        #[test]
        fn tbc_five_selects_the_full_clear() {
            assert_eq!(
                CharacterTabEdit::from_tbc(5),
                Some(CharacterTabEdit::ClearAllColumns)
            );
        }

        /// Asserts that the TBC parameters only line tabulation stops
        /// answer select nothing rather than falling back to the
        /// character stops.
        ///
        /// Case: an application written for a printer sends TBC 1 to
        /// drop the line tab stop on the cursor's line.
        #[test]
        fn tbc_values_only_line_stops_answer_are_dropped() {
            assert_eq!(CharacterTabEdit::from_tbc(1), None);
            assert_eq!(CharacterTabEdit::from_tbc(4), None);
        }

        /// Asserts that `CTC 0` selects the column set and `CTC 2` the
        /// column clear.
        ///
        /// Case: an application uses CTC rather than HTS and TBC to
        /// edit the tab position under the cursor.
        #[test]
        fn ctc_zero_selects_the_column_set_and_ctc_two_the_column_clear() {
            assert_eq!(
                CharacterTabEdit::from_ctc(0),
                Some(CharacterTabEdit::SetColumn)
            );
            assert_eq!(
                CharacterTabEdit::from_ctc(2),
                Some(CharacterTabEdit::ClearColumn)
            );
        }

        /// Asserts that `CTC 4` and `CTC 5` both select the full clear.
        ///
        /// Case: an application uses CTC rather than TBC to clear the
        /// tab table.
        #[test]
        fn ctc_four_and_five_both_select_the_full_clear() {
            assert_eq!(
                CharacterTabEdit::from_ctc(4),
                Some(CharacterTabEdit::ClearAllColumns)
            );
            assert_eq!(
                CharacterTabEdit::from_ctc(5),
                Some(CharacterTabEdit::ClearAllColumns)
            );
        }

        /// Asserts that the CTC parameters only line tabulation stops
        /// answer select nothing.
        ///
        /// Case: an application written for a printer sends CTC 1 to
        /// set a line tab stop on the cursor's line.
        #[test]
        fn ctc_values_only_line_stops_answer_are_dropped() {
            assert_eq!(CharacterTabEdit::from_ctc(1), None);
            assert_eq!(CharacterTabEdit::from_ctc(3), None);
            assert_eq!(CharacterTabEdit::from_ctc(6), None);
        }

        /// Asserts that a parameter neither control function defines
        /// selects nothing.
        ///
        /// Case: a malformed sequence reaches the terminal with a
        /// parameter past the range either function assigns.
        #[test]
        fn an_out_of_range_parameter_is_dropped() {
            assert_eq!(CharacterTabEdit::from_tbc(6), None);
            assert_eq!(CharacterTabEdit::from_ctc(7), None);
        }
    }
}
