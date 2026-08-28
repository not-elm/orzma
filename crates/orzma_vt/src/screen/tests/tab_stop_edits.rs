//! Tests for setting, clearing, and resetting tabulation stops.

use super::*;

/// Asserts that a stop set at the cursor is where the next tab
/// lands.
///
/// Case: an application walks to the column it wants, sets a tab
/// position there, and returns to the start of the line.
#[test]
fn hts_adds_a_stop_the_next_ht_finds() {
    let mut screen = wide_screen();
    screen.state.column = GridColumn(3);
    screen.set_horizontal_tab_stop();
    screen.state.column = GridColumn(0);
    screen.move_forward_tabs(1);
    assert_eq!(screen.state.column, GridColumn(3));
}

/// Asserts that setting a stop leaves the cursor where it was.
///
/// HTS edits the stop table and nothing else; the neighbouring
/// name HT is the one that moves. Nothing on screen changes
/// either, which is why `set_horizontal_tab_stop` reports no
/// damage to stage.
///
/// Case: an application installs a tab position at the column it
/// is already writing at, then keeps printing on the same line.
#[test]
fn hts_does_not_move_the_cursor() {
    let mut screen = wide_screen();
    screen.state.column = GridColumn(3);
    screen.set_horizontal_tab_stop();
    assert_eq!(screen.state.column, GridColumn(3));
}

/// Asserts that HTS and `CTC 0` install the same stop.
///
/// The two are one edit in the vocabulary rather than two
/// parallel implementations, so that a later TABULATION STOP
/// MODE cannot scope one of them and miss the other.
///
/// Case: an application uses CTC rather than HTS to install its
/// tab positions, having found the CSI form easier to generate.
#[test]
fn hts_and_ctc_zero_install_the_same_stop() {
    let mut by_hts = wide_screen();
    by_hts.state.column = GridColumn(3);
    by_hts.set_horizontal_tab_stop();

    let mut by_ctc = wide_screen();
    by_ctc.state.column = GridColumn(3);
    by_ctc.edit_tab_stop(CharacterTabEdit::from_ctc(0).unwrap());

    for screen in [&mut by_hts, &mut by_ctc] {
        screen.state.column = GridColumn(0);
        screen.move_forward_tabs(1);
    }
    assert_eq!(by_hts.state.column, GridColumn(3));
    assert_eq!(by_ctc.state.column, by_hts.state.column);
}

/// Asserts that clearing the stop under the cursor makes the
/// next tab reach the one after it.
///
/// Case: an application parks on a default tab position and
/// drops it so its own layout is one column wider.
#[test]
fn tbc_zero_clears_the_stop_under_the_cursor() {
    let mut screen = wide_screen();
    screen.state.column = GridColumn(8);
    screen.edit_tab_stop(CharacterTabEdit::from_tbc(0).unwrap());
    screen.state.column = GridColumn(0);
    screen.move_forward_tabs(1);
    assert_eq!(screen.state.column, GridColumn(16));
}

/// Asserts that clearing every stop leaves a tab nothing to
/// find.
///
/// Case: a full-screen application clears the tab table before
/// installing a layout of its own.
#[test]
fn tbc_three_clears_every_stop() {
    let mut screen = wide_screen();
    screen.edit_tab_stop(CharacterTabEdit::from_tbc(3).unwrap());
    screen.move_forward_tabs(1);
    assert_eq!(screen.state.column, GridColumn(19));
}

/// Asserts that CTC sets and clears the stop under the cursor.
///
/// Case: an application uses CTC rather than HTS and TBC to edit
/// the tab position it is parked on.
#[test]
fn ctc_zero_sets_and_ctc_two_clears_at_the_cursor() {
    let mut screen = wide_screen();
    screen.state.column = GridColumn(3);
    screen.edit_tab_stop(CharacterTabEdit::from_ctc(0).unwrap());
    screen.state.column = GridColumn(0);
    screen.move_forward_tabs(1);
    assert_eq!(screen.state.column, GridColumn(3));

    screen.edit_tab_stop(CharacterTabEdit::from_ctc(2).unwrap());
    screen.state.column = GridColumn(0);
    screen.move_forward_tabs(1);
    assert_eq!(screen.state.column, GridColumn(8));
}

/// Asserts that DECST8C brings the default stride back after a
/// full clear.
///
/// Case: an application that cleared the tab table asks for the
/// default tab positions again before exiting.
#[test]
fn decst8c_reinstalls_the_stride_after_a_full_clear() {
    let mut screen = wide_screen();
    screen.edit_tab_stop(CharacterTabEdit::from_tbc(3).unwrap());
    screen.reset_tab_stops();
    screen.move_forward_tabs(1);
    assert_eq!(screen.state.column, GridColumn(8));
}
