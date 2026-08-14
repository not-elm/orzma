//! Write-cursor extraction tests: active-grid position, raw wide-char
//! semantics, DECSCUSR shape and blink, DECTCEM intent, and vi mode.

use super::*;
use crate::schema::{Cursor, CursorShape};

/// Asserts that a fresh grid reports the origin with the default
/// style and the cursor shown.
///
/// Case: the terminal has just been created and no PTY byte has
/// arrived yet, so the shell prompt is about to render at the top
/// left corner.
#[test]
fn a_fresh_grid_starts_at_the_origin() {
    let vt = AlacrittyVtBackend::new(GRID_COLS, GRID_ROWS);
    assert_eq!(
        vt.cursor(),
        Cursor {
            point: point(0, 0),
            shape: CursorShape::Block,
            blinking: false,
            visible: true,
        }
    );
}

/// Asserts that printing text advances the cursor column past the
/// last written cell.
///
/// Case: the user types three characters at the prompt and the caret
/// follows the echoed text.
#[test]
fn printing_advances_the_column() {
    assert_eq!(vt_after(b"abc").cursor().point, point(0, 3));
}

/// Asserts that CRLF pairs advance the line and return the column to
/// the left edge.
///
/// Case: a command prints two short lines of output and leaves the
/// cursor at the start of the third line.
#[test]
fn line_feeds_advance_the_line() {
    assert_eq!(vt_after(b"a\r\nb\r\n").cursor().point, point(2, 0));
}

/// Asserts that CUP addresses the cursor to the requested cell in
/// active-grid coordinates.
///
/// Case: a full-screen application places its caret at an absolute
/// screen position before drawing a status field.
#[test]
fn cup_moves_to_the_addressed_cell() {
    assert_eq!(vt_after(b"\x1b[10;20H").cursor().point, point(9, 19));
}

/// Asserts that the cursor holds the last column while a wrap is
/// pending and only moves once the next character commits it.
///
/// Case: output fills a terminal row exactly, then continues with one
/// more character on the next row.
#[test]
fn the_last_column_holds_until_the_wrap_commits() {
    let mut vt = vt_after(&[b'a'; 80]);
    assert_eq!(vt.cursor().point, point(0, 79));
    vt.interpret(b"a");
    assert_eq!(vt.cursor().point, point(1, 1));
}

/// Asserts that user scrolling leaves the reported cursor untouched
/// while the viewport projection of its line becomes `None`.
///
/// Case: the user scrolls back through history while the shell sits
/// idle at its prompt; the cursor state is unchanged, and only the
/// projection decides that there is no caret cell to paint.
#[test]
fn scrolling_leaves_the_cursor_untouched() {
    let mut vt = vt_with_history(SEEDED_HISTORY_ROWS);
    let before = vt.cursor();
    assert!(before.visible);
    vt.scroll(Scroll::Delta(5));
    assert_eq!(vt.cursor(), before);
    assert_eq!(vt.display_offset(), DisplayOffset(5));
    assert_eq!(
        vt.cursor()
            .point
            .line
            .to_viewport(vt.display_offset(), GRID_ROWS),
        None
    );
    vt.scroll(Scroll::Top);
    assert_eq!(vt.cursor(), before);
}

/// Asserts that widening the grid reflows a wrapped line and carries
/// the cursor with its text.
///
/// Case: the user widens the window after output wrapped at the old
/// width, and the caret follows its character onto the rejoined line.
#[test]
fn reflow_moves_the_cursor_with_its_text() {
    let mut vt = vt_after(&[b'a'; 81]);
    vt.resize(100, GRID_ROWS);
    assert_eq!(vt.cursor().point, point(0, 81));
}

/// Asserts that entering the alternate screen carries the cursor in
/// and leaving it restores the primary position.
///
/// Case: the user opens a full-screen application from the shell,
/// moves around inside it, and quits back to the prompt they left.
#[test]
fn the_alt_screen_saves_and_restores_the_cursor() {
    let mut vt = vt_after(b"abc");
    assert_eq!(vt.cursor().point, point(0, 3));
    vt.interpret(b"\x1b[?1049h");
    assert_eq!(vt.cursor().point, point(0, 3));
    vt.interpret(b"\x1b[10;20H");
    assert_eq!(vt.cursor().point, point(9, 19));
    vt.interpret(b"\x1b[?1049l");
    assert_eq!(vt.cursor().point, point(0, 3));
}

/// Asserts that origin-mode CUP addresses within the scrolling region
/// while the reported point stays in active-grid coordinates.
///
/// Case: a full-screen application confines itself to a scrolling
/// region with DECOM enabled, and its region-relative cursor moves
/// still land on absolute grid cells.
#[test]
fn origin_mode_addresses_within_the_scroll_region() {
    assert_eq!(
        vt_after(b"\x1b[5;20r\x1b[?6h\x1b[2;3H").cursor().point,
        point(5, 2)
    );
}

/// Asserts that a wide character advances the cursor two columns and
/// that backing up lands on the trailing spacer cell uncorrected.
///
/// Case: the user types a full-width CJK character and then presses
/// the left arrow once; the raw cursor rests on the spacer half of
/// the glyph, and the schema reports that position as-is.
#[test]
fn a_wide_char_advances_two_columns_and_backing_up_lands_on_the_spacer() {
    let mut vt = vt_after("あ".as_bytes());
    assert_eq!(vt.cursor().point, point(0, 2));
    vt.interpret(b"\x1b[D");
    assert_eq!(vt.cursor().point, point(0, 1));
}

/// Asserts that the steady DECSCUSR variants map to their shapes with
/// blinking off.
///
/// Case: a terminal application selects each steady caret style in
/// turn — block, underline, and bar.
#[test]
fn steady_decscusr_variants_map_to_shapes() {
    let cases = [
        (&b"\x1b[2 q"[..], CursorShape::Block),
        (&b"\x1b[4 q"[..], CursorShape::Underline),
        (&b"\x1b[6 q"[..], CursorShape::Bar),
    ];
    for (bytes, shape) in cases {
        let cursor = vt_after(bytes).cursor();
        assert_eq!(cursor.shape, shape);
        assert!(!cursor.blinking);
    }
}

/// Asserts that the blinking DECSCUSR variants set the blinking flag
/// for every shape.
///
/// Case: a terminal application selects each blinking caret style in
/// turn — block, underline, and bar.
#[test]
fn blinking_decscusr_variants_set_blinking() {
    let cases = [
        (&b"\x1b[1 q"[..], CursorShape::Block),
        (&b"\x1b[3 q"[..], CursorShape::Underline),
        (&b"\x1b[5 q"[..], CursorShape::Bar),
    ];
    for (bytes, shape) in cases {
        let cursor = vt_after(bytes).cursor();
        assert_eq!(cursor.shape, shape);
        assert!(cursor.blinking);
    }
}

/// Asserts that DECSCUSR 0 resets the style to the configured
/// default.
///
/// Case: an application that customized its caret restores the
/// terminal default on exit with the reset variant.
#[test]
fn decscusr_zero_resets_to_the_config_default() {
    let cursor = vt_after(b"\x1b[5 q\x1b[0 q").cursor();
    assert_eq!(cursor.shape, CursorShape::Block);
    assert!(!cursor.blinking);
}

/// Asserts that a cursor-style escape split across two interpret
/// calls still takes effect.
///
/// Case: the PTY reader hands the VT a chunk boundary that falls in
/// the middle of a DECSCUSR sequence.
#[test]
fn a_split_escape_survives_chunked_interpretation() {
    let mut vt = AlacrittyVtBackend::new(GRID_COLS, GRID_ROWS);
    vt.interpret(b"\x1b[5 ");
    vt.interpret(b"q");
    let cursor = vt.cursor();
    assert_eq!(cursor.shape, CursorShape::Bar);
    assert!(cursor.blinking);
}

/// Asserts that OSC 50 changes the shape while preserving the
/// blinking flag.
///
/// Case: an application that already selected a blinking caret
/// switches only its shape through the legacy OSC 50 path.
#[test]
fn osc_50_changes_shape_preserving_blinking() {
    let cursor = vt_after(b"\x1b[5 q\x1b]50;CursorShape=2\x07").cursor();
    assert_eq!(cursor.shape, CursorShape::Underline);
    assert!(cursor.blinking);
}

/// Asserts that DEC private mode 12 toggles blinking independently of
/// DECSCUSR.
///
/// Case: an application turns cursor blinking off and back on through
/// the mode-12 path while keeping its selected shape.
#[test]
fn dec_mode_12_toggles_blinking() {
    let mut vt = vt_after(b"\x1b[3 q");
    assert!(vt.cursor().blinking);
    vt.interpret(b"\x1b[?12l");
    assert!(!vt.cursor().blinking);
    assert_eq!(vt.cursor().shape, CursorShape::Underline);
    vt.interpret(b"\x1b[?12h");
    assert!(vt.cursor().blinking);
}

/// Asserts that DECTCEM hides and shows the cursor while the position
/// keeps being reported.
///
/// Case: vim hides the caret during a redraw and shows it again when
/// the frame is complete; overlays that anchor to the cursor still
/// need its position while it is hidden.
#[test]
fn dectcem_hides_and_shows_the_cursor() {
    let mut vt = vt_after(b"abc\x1b[?25l");
    let hidden = vt.cursor();
    assert!(!hidden.visible);
    assert_eq!(hidden.point, point(0, 3));
    vt.interpret(b"\x1b[?25h");
    assert!(vt.cursor().visible);
}

/// Asserts that entering vi mode does not override the DECTCEM
/// intent.
///
/// Case: an application hides the cursor, then the user enters vi
/// mode to inspect the screen; the application's hide request stays
/// in force for the write cursor.
#[test]
fn vi_mode_does_not_mask_the_dectcem_intent() {
    let mut vt = vt_after(b"\x1b[?25l");
    vt.switch_vi_mode(ViModeSwitch::Enter).unwrap();
    assert!(!vt.cursor().visible);
}

/// Asserts that `cursor` keeps reporting the write cursor while vi
/// mode is active.
///
/// Case: the user enters vi mode and the shell keeps printing; the
/// write cursor follows the new output while the vi cursor stays
/// where the user left it.
#[test]
fn vi_mode_reports_the_write_cursor() {
    let mut vt = vt_after(b"abc");
    vt.switch_vi_mode(ViModeSwitch::Enter).unwrap();
    vt.interpret(b"\x1b[10;20H");
    assert_eq!(vt.cursor().point, point(9, 19));
}
