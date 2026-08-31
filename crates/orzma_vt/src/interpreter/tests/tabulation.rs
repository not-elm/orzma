//! Tests for tab stops: planting them, clearing them, and moving
//! between them.

use super::*;

/// Asserts that `ESC H` plants a tabulation stop at the cursor
/// column.
///
/// Case: a program sizes a column by walking the cursor to the width
/// it wants, setting a stop there, and tabbing to it on later rows.
#[test]
fn the_seven_bit_tab_set_plants_a_stop_at_the_cursor() {
    let device = interpret(b"ab\x1bH\r\t");
    assert_eq!(device.active_screen().cursor_column(), GridColumn(2));
}

/// Asserts that `CSI I` advances the cursor by whole tab stops.
///
/// Case: a program lays out a table by asking for two tab stops
/// rather than emitting two horizontal tabs.
#[test]
fn the_forward_tabulation_sequence_advances_by_stops() {
    let device = interpret_wide(b"\x1b[2Ix");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(0))[16].c,
        'x'
    );
}

/// Asserts that `CSI Z` walks the cursor back by whole tab stops.
///
/// Case: a program aligning a column overshoots and steps back one
/// stop to line up with the header above it.
#[test]
fn the_backward_tabulation_sequence_retreats_by_stops() {
    let device = interpret_wide(b"\x1b[1;20H\x1b[Zx");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(0))[16].c,
        'x'
    );
}

/// Asserts that an omitted tabulation count moves one stop, the
/// default every `Pn` carries.
///
/// Case: a program emits the bare `CSI I` spelling for a single
/// tab.
#[test]
fn an_omitted_tabulation_count_moves_one_stop() {
    let device = interpret_wide(b"\x1b[Ix");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(0))[8].c,
        'x'
    );
}

/// Asserts that a zero tabulation count moves one stop rather than
/// standing still.
///
/// Case: a program computes its tab count and emits `CSI 0 I` when
/// the computation yields nothing to skip.
#[test]
fn a_zero_tabulation_count_moves_one_stop() {
    let device = interpret_wide(b"\x1b[0Ix");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(0))[8].c,
        'x'
    );
}

/// Asserts that `CSI 3 g` clears every tab stop, so a later tab
/// runs to the right edge.
///
/// Case: a program installs its own column layout and clears the
/// default eight-column stride first.
#[test]
fn the_tabulation_clear_sequence_clears_every_stop() {
    let device = interpret_wide(b"\x1b[3g\tx");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(0))[19].c,
        'x'
    );
}

/// Asserts that `CSI 0 W` sets a tab stop at the cursor column.
///
/// Case: a program installs a stop with the cursor-tabulation
/// spelling rather than `HTS`.
#[test]
fn the_cursor_tabulation_control_sequence_sets_a_stop() {
    let device = interpret_wide(b"\x1b[3g\x1b[1;4H\x1b[0W\x1b[1;1H\tx");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(0))[3].c,
        'x'
    );
}

/// Asserts that `CSI ? 5 W` reinstalls the default eight-column
/// stride.
///
/// Case: a program clears every stop, lays out its own table, and
/// restores the defaults before handing the terminal back.
#[test]
fn the_tab_stop_reset_sequence_reinstalls_the_default_stride() {
    let device = interpret_wide(b"\x1b[3g\x1b[?5W\tx");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(0))[8].c,
        'x'
    );
}
