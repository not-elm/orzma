//! Tests for the viewport wrap list a frame carries: it follows every
//! change to the rows' recorded wraps, including the ones no row damage
//! reports.

use super::*;
use crate::screen::viewport::Scroll;

/// A 4x3 terminal with ten rows of history whose bootstrap frame has
/// already been drained.
fn drained_vt() -> OrzmaVt {
    let mut vt = OrzmaVt::new(GridSize { cols: 4, rows: 3 }, 10);
    vt.frame().expect("the bootstrap repaint emits");
    vt
}

/// Feeds `bytes` and returns the frame they produce.
fn frame_after(vt: &mut OrzmaVt, bytes: &[u8]) -> Frame {
    vt.interpret(bytes);
    vt.frame().expect("the bytes change the screen")
}

/// Asserts that the wrap recorded when a pending autowrap resolves in a
/// later frame reaches the consumer, although no damage covers the row
/// that wrapped.
///
/// Case: the user types a command at the prompt until it reaches the
/// last column, and the next keystroke arrives after that frame went out.
#[test]
fn a_pending_wrap_resolved_in_a_later_frame_is_carried() {
    let mut vt = drained_vt();
    let filled = frame_after(&mut vt, b"abcd");
    assert_eq!(filled.wraps, None);
    let wrapped = frame_after(&mut vt, b"e");
    assert_eq!(wrapped.wraps, Some(vec![Some(4), None, None]));
}

/// Asserts that erasing the row a wrapped row continues on ends the
/// wrapped row's line in the carried list.
///
/// Case: a shell redraws its prompt and clears the second row of a line
/// that wrapped with `EL 2`.
#[test]
fn erasing_the_continuation_row_ends_the_wrapped_line() {
    let mut vt = drained_vt();
    let wrapped = frame_after(&mut vt, b"abcde");
    assert_eq!(wrapped.wraps, Some(vec![Some(4), None, None]));
    let erased = frame_after(&mut vt, b"\x1b[2K");
    assert_eq!(erased.wraps, Some(vec![None, None, None]));
}

/// Asserts that erasing below from the first column ends the line of
/// the row above in the carried list.
///
/// Case: a full-screen program clears everything from the start of the
/// second row down with `ED 0`.
#[test]
fn erasing_below_from_the_first_column_ends_the_line_above() {
    let mut vt = drained_vt();
    frame_after(&mut vt, b"abcde");
    let erased = frame_after(&mut vt, b"\x1b[2;1H\x1b[J");
    assert_eq!(erased.wraps, Some(vec![None, None, None]));
}

/// Asserts that a full reset of a screen whose wrapped rows hold only
/// blanks ends every line in the carried list.
///
/// Case: a program prints a run of spaces that wraps, then sends `RIS`.
#[test]
fn a_reset_of_blank_wrapped_rows_ends_every_line() {
    let mut vt = drained_vt();
    let wrapped = frame_after(&mut vt, b"     ");
    assert_eq!(wrapped.wraps, Some(vec![Some(4), None, None]));
    let reset = frame_after(&mut vt, b"\x1bc");
    assert_eq!(reset.wraps, Some(vec![None, None, None]));
}

/// Asserts that the frame reports when the top viewport row continues a
/// line from the row above it, and stops once that line is cut.
///
/// Case: long output scrolls the first part of a wrapped line into
/// history, and a program then erases the screen above the cursor with
/// `ED 1`.
#[test]
fn the_top_row_continuing_from_history_is_reported_until_its_line_is_cut() {
    let mut vt = drained_vt();
    let scrolled = frame_after(&mut vt, &[b'x'; 16]);
    assert!(scrolled.continues_from_above);
    let cut = frame_after(&mut vt, b"\x1b[1;4H\x1b[1J");
    assert!(!cut.continues_from_above);
}

/// Asserts that the oldest history row reports no row above it once the
/// viewport scrolls back onto it, and that the list follows the rows the
/// viewport now shows.
///
/// Case: the user scrolls to the top of a short scrollback.
#[test]
fn the_oldest_history_row_continues_from_nothing() {
    let mut vt = drained_vt();
    frame_after(&mut vt, &[b'x'; 16]);
    assert!(vt.scroll(Scroll::Top));
    let top = vt.frame().expect("a moved viewport emits");
    assert!(!top.continues_from_above);
    assert_eq!(top.wraps, Some(vec![Some(4), Some(4), Some(4)]));
}
