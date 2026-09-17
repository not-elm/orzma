//! Tests for `REP`, which prints the preceding graphic character again.

use super::*;
use crate::screen::cell::CellWidth;

/// Asserts that `REP` prints the preceding character as many more times
/// as its parameter asks.
///
/// Case: an ncurses program in a non-UTF-8 locale draws a run of four
/// identical letters by printing one letter and repeating it three times.
#[test]
fn a_repeat_prints_the_preceding_character_the_requested_number_of_times() {
    let device = interpret_wide(b"A\x1b[3b");
    for column in 0..4 {
        assert_eq!(glyph_at(&device, 0, column), 'A');
    }
    assert_eq!(glyph_at(&device, 0, 4), ' ');
}

/// Asserts that `REP` with its parameter omitted prints the preceding
/// character once more.
///
/// Case: a program prints one character and sends `CSI b` with no count.
#[test]
fn a_repeat_without_a_parameter_prints_the_preceding_character_once() {
    let device = interpret_wide(b"A\x1b[b");
    assert_eq!(glyph_at(&device, 0, 0), 'A');
    assert_eq!(glyph_at(&device, 0, 1), 'A');
    assert_eq!(glyph_at(&device, 0, 2), ' ');
}

/// Asserts that `REP` with an explicit count of one prints the preceding
/// character once more.
///
/// Case: vttest's REP screen prints a plus sign and follows it with an
/// explicit count of one.
#[test]
fn a_repeat_with_a_parameter_of_one_prints_the_preceding_character_once() {
    let device = interpret_wide(b"A\x1b[1b");
    assert_eq!(glyph_at(&device, 0, 0), 'A');
    assert_eq!(glyph_at(&device, 0, 1), 'A');
    assert_eq!(glyph_at(&device, 0, 2), ' ');
}

/// Asserts that `REP` repeats a space like any other graphic character,
/// overwriting the cells it lands on.
///
/// Case: a program blanks the start of a line of text by printing one
/// space over it and repeating that space.
#[test]
fn a_repeat_of_a_space_prints_spaces() {
    let device = interpret_wide(b"xxxxxx\x1b[1G \x1b[2b");
    for column in 0..3 {
        assert_eq!(glyph_at(&device, 0, column), ' ');
    }
    assert_eq!(glyph_at(&device, 0, 3), 'x');
}

/// Asserts that `REP` after a single-shifted character repeats the glyph
/// the single shift selected.
///
/// Case: a program designates line drawing into G2, shows one
/// line-drawing character through `SS2`, and repeats it to extend a rule.
#[test]
fn a_repeat_counts_a_single_shifted_character_as_the_character_to_repeat() {
    let device = interpret_wide(b"\x1b*0\x1bNq\x1b[2b");
    let shifted = glyph_at(&device, 0, 0);
    assert_eq!(glyph_at(&device, 0, 1), shifted);
    assert_eq!(glyph_at(&device, 0, 2), shifted);
}

/// Asserts that `REP` before any graphic character has been printed
/// prints nothing and leaves the cursor where it was.
///
/// Case: `CSI 3 b` is the first thing a freshly started terminal
/// receives.
#[test]
fn a_repeat_before_any_graphic_character_prints_nothing() {
    let device = interpret_wide(b"\x1b[3b");
    assert!(first_row_glyphs(&device).iter().all(|&glyph| glyph == ' '));
    assert_eq!(device.active_screen().cursor_column(), GridColumn(0));
}

/// Asserts that `RIS` clears the preceding graphic character, so a `REP`
/// after it prints nothing.
///
/// Case: a program prints a character, the terminal is hard reset, and a
/// stale `REP` arrives afterwards.
#[test]
fn a_reset_clears_the_preceding_character() {
    let device = interpret_wide(b"A\x1bc\x1b[3b");
    assert!(first_row_glyphs(&device).iter().all(|&glyph| glyph == ' '));
}

/// Asserts that `REP` with an explicit count of zero prints the preceding
/// character once more rather than repeating it zero times.
///
/// Case: vttest's REP screen prints a plus sign on its first row and
/// follows it with an explicit count of zero.
#[test]
fn a_repeat_with_a_parameter_of_zero_prints_the_preceding_character_once() {
    let device = interpret_wide(b"A\x1b[0b");
    assert_eq!(glyph_at(&device, 0, 0), 'A');
    assert_eq!(glyph_at(&device, 0, 1), 'A');
    assert_eq!(glyph_at(&device, 0, 2), ' ');
}

/// Asserts that a `REP` crossing the right border continues at the start
/// of the next line while autowrap is set.
///
/// Case: a curses program repeats a character across the right border of
/// a ten-column terminal with autowrap on.
#[test]
fn a_repeat_at_the_right_border_wraps_onto_the_next_line() {
    let (device, _) = interpret_sized(10, b"\x1b[9GA\x1b[3b");
    assert_eq!(glyph_at(&device, 0, 8), 'A');
    assert_eq!(glyph_at(&device, 0, 9), 'A');
    assert_eq!(glyph_at(&device, 1, 0), 'A');
    assert_eq!(glyph_at(&device, 1, 1), 'A');
}

/// Asserts that a `REP` wrapping on the bottom row scrolls the page up
/// while autowrap is set.
///
/// Case: the repetition crosses the right border while the cursor is on
/// the bottom row of the screen.
#[test]
fn a_repeat_that_wraps_on_the_last_row_scrolls_the_page_up() {
    let (device, _) = interpret_sized(10, b"\x1b[999;9HA\x1b[3b");
    assert_eq!(glyph_at(&device, 1, 8), 'A');
    assert_eq!(glyph_at(&device, 1, 9), 'A');
    assert_eq!(glyph_at(&device, 2, 0), 'A');
    assert_eq!(glyph_at(&device, 2, 1), 'A');
}

/// Asserts that a `REP` reaching the right border replaces the last
/// column instead of wrapping while autowrap is reset.
///
/// Case: a status-line program with autowrap turned off repeats a
/// character past the right border.
#[test]
fn a_repeat_at_the_right_border_without_autowrap_overwrites_the_last_column() {
    let (device, _) = interpret_sized(10, b"xxxxxxxxxx\x1b[?7l\x1b[9GA\x1b[3b");
    assert_eq!(glyph_at(&device, 0, 8), 'A');
    assert_eq!(glyph_at(&device, 0, 9), 'A');
    for column in 0..10 {
        assert_eq!(glyph_at(&device, 1, column), ' ');
    }
}

/// Asserts that each character a `REP` prints in insert mode shifts the
/// rest of the row right.
///
/// Case: an editor in insert mode inserts a run of identical characters
/// before existing text.
#[test]
fn a_repeat_in_insert_mode_shifts_the_rest_of_the_row_for_each_repetition() {
    let (device, _) = interpret_sized(10, b"abcdef\x1b[1G\x1b[4hX\x1b[2b");
    let row: String = first_row_glyphs(&device).into_iter().take(9).collect();
    assert_eq!(row, "XXXabcdef");
}

/// Asserts that the characters a `REP` prints take the rendition selected
/// after the preceding character, not the one it was printed with.
///
/// Case: a program prints a character, switches to bold, and then repeats
/// the character.
#[test]
fn a_repeat_uses_the_rendition_selected_after_the_preceding_character() {
    let device = interpret_wide(b"A\x1b[1m\x1b[2b");
    assert!(!cell_at(&device, 0, 0).style.contains(Style::BOLD));
    assert!(cell_at(&device, 0, 1).style.contains(Style::BOLD));
    assert!(cell_at(&device, 0, 2).style.contains(Style::BOLD));
}

/// Asserts that a `REP` leaves a pending single shift for the next
/// character received rather than spending it on a repetition.
///
/// Case: a program leaves a single shift pending, sends `REP`, and then
/// prints the character the shift was meant for.
#[test]
fn a_repeat_leaves_a_pending_single_shift_for_the_next_character() {
    let device = interpret_wide(b"\x1b*0\x1bNqq\x1bN\x1b[2bq");
    assert_eq!(glyph_at(&device, 0, 2), 'q');
    assert_eq!(glyph_at(&device, 0, 3), 'q');
    assert_eq!(glyph_at(&device, 0, 4), glyph_at(&device, 0, 0));
}

/// Asserts that a `REP` after a line break still repeats the character
/// printed before it, rather than printing nothing.
///
/// Case: a program prints a character, moves to the next line with CR
/// LF, and sends `REP` there.
#[test]
fn a_repeat_after_a_line_feed_repeats_the_character_before_it() {
    let device = interpret_wide(b"A\r\n\x1b[2b");
    assert_eq!(glyph_at(&device, 1, 0), 'A');
    assert_eq!(glyph_at(&device, 1, 1), 'A');
}

/// Asserts that a second `REP` right after a first one repeats the same
/// character again rather than being ignored.
///
/// Case: vttest's REP screen sends a second `REP` right after the first
/// one.
#[test]
fn a_second_repeat_repeats_the_same_character_again() {
    let device = interpret_wide(b"A\x1b[b\x1b[b");
    for column in 0..3 {
        assert_eq!(glyph_at(&device, 0, column), 'A');
    }
}

/// Asserts that a `REP` after a combining mark repeats the base character
/// without the mark.
///
/// Case: a program prints an e with a combining acute accent and then
/// repeats it.
#[test]
fn a_repeat_after_a_combining_mark_repeats_the_base_character_alone() {
    let device = interpret_wide("e\u{301}\x1b[2b".as_bytes());
    let base = cell_at(&device, 0, 0);
    assert_eq!(base.c, 'e');
    assert_eq!(base.marks(), ['\u{301}']);
    for column in 1..3 {
        let repeated = cell_at(&device, 0, column);
        assert_eq!(repeated.c, 'e');
        assert!(repeated.marks().is_empty());
    }
}

/// Asserts that a `REP` after a character set change repeats the glyph
/// first shown, not the byte mapped through the new set.
///
/// Case: a program prints one line-drawing character, designates ASCII
/// back into G0, and then repeats.
#[test]
fn a_repeat_after_a_character_set_change_repeats_the_glyph_first_printed() {
    let device = interpret_wide(b"\x1b(0q\x1b(B\x1b[2b");
    let first = glyph_at(&device, 0, 0);
    assert_eq!(glyph_at(&device, 0, 1), first);
    assert_eq!(glyph_at(&device, 0, 2), first);
}

/// Asserts that a wide character the screen drops still becomes the
/// character a later `REP` repeats.
///
/// Case: with autowrap off, a wide character arrives on the last column
/// of a ten-column screen and is dropped, and the program moves to the
/// first column and sends `REP`.
#[test]
fn a_repeat_after_a_dropped_wide_character_repeats_that_character() {
    let (device, _) = interpret_sized(10, "\x1b[?7l\x1b[10G中\x1b[1G\x1b[b".as_bytes());
    assert_eq!(glyph_at(&device, 0, 9), ' ');
    let body = cell_at(&device, 0, 0);
    assert_eq!(body.c, '中');
    assert_eq!(body.width, CellWidth::Wide);
    assert_eq!(cell_at(&device, 0, 1).width, CellWidth::Spacer);
}

/// Asserts that a soft reset leaves the preceding graphic character, so a
/// `REP` after it still repeats.
///
/// Case: a program prints a character, performs a soft reset, and then
/// repeats.
#[test]
fn a_repeat_after_a_soft_reset_repeats_the_character_before_it() {
    let device = interpret_wide(b"A\x1b[!p\x1b[b");
    assert_eq!(glyph_at(&device, 0, 0), 'A');
    assert_eq!(glyph_at(&device, 0, 1), 'A');
}

/// Asserts that the preceding graphic character is shared by both
/// screens, so a `REP` on the alternate screen repeats what the primary
/// screen printed.
///
/// Case: a shell prints a character, and a full-screen program enters the
/// alternate screen and sends `REP` before printing anything there.
#[test]
fn a_repeat_after_switching_screens_repeats_the_character_printed_on_the_other_screen() {
    let device = interpret_wide(b"x\x1b[?1049h\x1b[H\x1b[2b");
    assert_eq!(device.modes().active_screen, ScreenKind::Alternate);
    assert_eq!(glyph_at(&device, 0, 0), 'x');
    assert_eq!(glyph_at(&device, 0, 1), 'x');
}

/// Asserts that the characters a `REP` prints join the hyperlink open at
/// the time of the `REP`.
///
/// Case: a build tool prints a character, opens a hyperlink, and repeats
/// the character inside the link.
#[test]
fn a_repeat_prints_inside_the_open_hyperlink() {
    let device = interpret_wide(b"A\x1b]8;;https://a.example\x1b\\\x1b[2b");
    let row = device.active_screen().viewport_row(ViewportLine(0));
    assert_eq!(row[0].hyperlink_id, None);
    assert!(row[1].hyperlink_id.is_some());
    assert!(row[2].hyperlink_id.is_some());
}
