//! Tests for reverse index, in each of the spellings that request it.

use super::*;

/// Asserts that the raw C1 byte for RI reaches the screen.
///
/// Case: a program emits an eight-bit reverse index on a terminal not
/// running in UTF-8 mode.
#[test]
fn the_raw_c1_byte_reverse_indexes() {
    let device = interpret(b"a\r\x8d");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(1))[0].c,
        'a'
    );
}

/// Asserts that the UTF-8 encoding of U+008D reaches the same arm.
///
/// Case: a program running on a UTF-8 stream emits the reverse index.
#[test]
fn the_utf8_form_reverse_indexes() {
    let device = interpret(b"a\r\xc2\x8d");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(1))[0].c,
        'a'
    );
}

/// Asserts that `ESC M` scrolls the region down when the cursor
/// already sits on the top margin.
///
/// Case: a pager walks backwards through a document with the
/// seven-bit reverse index while the cursor rests on the first row.
#[test]
fn the_seven_bit_reverse_index_scrolls_at_the_top_margin() {
    let device = interpret(b"a\r\x1bM");
    assert_eq!(
        device.active_screen().viewport_row(ViewportLine(1))[0].c,
        'a'
    );
}
