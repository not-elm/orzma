//! Tests for the soft terminal reset.

use super::*;
use crate::device::modes::{AutoWrap, KeypadMode};

/// Asserts that a soft reset shows a cursor an application hid.
///
/// Case: a full-screen program dies with the caret hidden and the user
/// runs `tput init` to get the terminal back.
#[test]
fn a_soft_reset_shows_a_hidden_cursor() {
    let device = interpret(b"\x1b[?25l\x1b[!p");
    assert!(device.cursor().visible);
}

/// Asserts that a soft reset returns the terminal to replace mode.
///
/// Case: a program turns insert mode on to shift a row right and exits
/// before turning it back off.
#[test]
fn a_soft_reset_returns_the_terminal_to_replace_mode() {
    let device = interpret(b"\x1b[4h\x1b[!p");
    assert_eq!(device.modes().insert_replace, InsertReplaceMode::Replace);
}

/// Asserts that a soft reset returns autowrap to enabled rather than to
/// the `No autowrap` of vt510.pdf p.277 Table 5-9.
///
/// Case: a status-bar program turns autowrap off to draw a full-width
/// label, and the shell runs `tput init` behind it.
#[test]
fn a_soft_reset_re_enables_autowrap() {
    let device = interpret(b"\x1b[?7l\x1b[!p");
    assert_eq!(device.modes().auto_wrap, AutoWrap::Enabled);
}

/// Asserts that a soft reset returns the arrow keys to their normal
/// encoding.
///
/// Case: a full-screen editor puts the cursor keys in application mode
/// and the shell resets the terminal after it exits.
#[test]
fn a_soft_reset_returns_the_cursor_keys_to_normal() {
    let device = interpret(b"\x1b[?1h\x1b[!p");
    assert!(!device.modes().app_cursor);
}

/// Asserts that a soft reset returns the keypad to numeric.
///
/// Case: a program puts the keypad in application mode with `ESC =` and
/// the shell resets the terminal after it exits.
#[test]
fn a_soft_reset_returns_the_keypad_to_numeric() {
    let device = interpret(b"\x1b=\x1b[!p");
    assert_eq!(device.modes().keypad_mode, KeypadMode::Numeric);
}

/// Asserts that a soft reset which shows a hidden cursor raises the
/// chunk liveness.
///
/// Case: the terminal sits idle with the caret hidden when `tput init`
/// arrives in a read of its own.
#[test]
fn a_soft_reset_that_shows_the_cursor_raises_the_liveness() {
    assert!(liveness_after(b"\x1b[?25l", b"\x1b[!p"));
}

/// Asserts that a soft reset which only returns the pen stages no
/// damage.
///
/// Case: a program leaves a coloured pen set and `tput init` arrives
/// with nothing on screen that needs repainting.
#[test]
fn a_soft_reset_that_only_returns_the_pen_stages_no_damage() {
    assert!(!liveness_after(b"\x1b[1;31m", b"\x1b[!p"));
}

/// Asserts that a soft reset returns a recoloured indexed palette slot
/// to its built-in default.
///
/// Case: a colour-scheme script recolours the palette with `OSC 4` and
/// the shell runs `tput init` afterwards.
#[test]
fn a_soft_reset_returns_the_indexed_palette_to_its_default() {
    let recoloured = replies_of(b"\x1b]4;1;rgb:0102/0304/0506\x1b\\\x1b]4;1;?\x1b\\");
    let after_reset = replies_of(b"\x1b]4;1;rgb:0102/0304/0506\x1b\\\x1b[!p\x1b]4;1;?\x1b\\");
    let untouched = replies_of(b"\x1b]4;1;?\x1b\\");
    assert_ne!(recoloured, untouched);
    assert_eq!(after_reset, untouched);
}

/// Asserts that a soft reset which returns a recoloured palette stages
/// the repaint that owes.
///
/// Case: a colour-scheme script recolours the palette and `tput init`
/// arrives in a read of its own, with the old colours still on screen.
#[test]
fn a_soft_reset_that_returns_the_palette_stages_a_repaint() {
    assert!(liveness_after(
        b"\x1b]4;1;rgb:0102/0304/0506\x1b\\",
        b"\x1b[!p"
    ));
}

/// Asserts that a soft reset returns the cursor origin to the upper
/// left corner.
///
/// Case: a full-screen program sets a scrolling region with origin mode
/// on, and the shell resets the terminal and sets its own region
/// afterwards.
#[test]
fn a_soft_reset_returns_the_origin_to_the_upper_left_corner() {
    let device = interpret(b"\x1b[2;3r\x1b[?6h\x1b[!p\x1b[2;3rx");
    assert_eq!(glyph_at(&device, 0, 0), 'x');
}

/// Asserts that a soft reset leaves the cursor where it stands.
///
/// Case: the shell runs `tput init` at a prompt part-way across the
/// screen and goes on printing from there.
#[test]
fn a_soft_reset_leaves_the_cursor_where_it_stands() {
    let device = interpret(b"ab\x1b[!pc");
    assert_eq!(glyph_at(&device, 0, 0), 'a');
    assert_eq!(glyph_at(&device, 0, 2), 'c');
}

/// Asserts that a soft reset returns the scrolling margins to the whole
/// page.
///
/// Case: a full-screen program leaves a scrolling region behind and the
/// shell resets the terminal before scrolling its own output.
#[test]
fn a_soft_reset_returns_the_margins_to_the_page() {
    let device = interpret(b"x\x1b[2;3r\x1b[!p\x1b[1S");
    assert_eq!(glyph_at(&device, 0, 0), ' ');
}

/// Asserts that a soft reset restores the default designation of G0.
///
/// Case: a program designates the line-drawing set into G0 to draw a
/// box and exits without restoring ASCII.
#[test]
fn a_soft_reset_restores_the_default_g0_designation() {
    let device = interpret(b"\x1b(0\x1b[!pq");
    assert_eq!(glyph_at(&device, 0, 0), 'q');
}

/// Asserts that a soft reset returns a locking shift into G1 to G0.
///
/// Case: a program designates line drawing into G1, locks it into GL
/// with `SO`, and exits without shifting back.
#[test]
fn a_soft_reset_returns_the_g1_locking_shift() {
    let device = interpret(b"\x1b)0\x0e\x1b[!pq");
    assert_eq!(glyph_at(&device, 0, 0), 'q');
}

/// Asserts that a soft reset restores the default designation of G1.
///
/// Case: a program designates line drawing into G1 and locks it into
/// GL with `SO`, and after the shell resets the terminal a later
/// program invokes G1 again assuming the default character set.
#[test]
fn a_soft_reset_restores_the_default_g1_designation() {
    let device = interpret(b"\x1b)0\x0e\x1b[!p\x0eq");
    assert_eq!(glyph_at(&device, 0, 0), 'q');
}

/// Asserts that a soft reset returns a locking shift into G2 to G0.
///
/// Case: a program designates line drawing into G2, locks it into GL
/// with `LS2`, and exits without shifting back.
#[test]
fn a_soft_reset_returns_the_g2_locking_shift() {
    let device = interpret(b"\x1b*0\x1bn\x1b[!pq");
    assert_eq!(glyph_at(&device, 0, 0), 'q');
}

/// Asserts that a soft reset returns a locking shift into G3 to G0.
///
/// Case: a program designates line drawing into G3, locks it into GL
/// with `LS3`, and exits without shifting back.
#[test]
fn a_soft_reset_returns_the_g3_locking_shift() {
    let device = interpret(b"\x1b+0\x1bo\x1b[!pq");
    assert_eq!(glyph_at(&device, 0, 0), 'q');
}

/// Asserts that a soft reset drops a single shift an application armed.
///
/// Case: a program sends `SS2` and is killed before the graphic
/// character that would have consumed it, and a later program
/// designates its own line-drawing set into G2 without invoking it.
#[test]
fn a_soft_reset_drops_a_pending_single_shift() {
    let device = interpret(b"\x1b*0\x1bN\x1b[!p\x1b*0q");
    assert_eq!(glyph_at(&device, 0, 0), 'q');
}

/// Asserts that a soft reset returns the pen to its default.
///
/// Case: a program dies with a bold red pen set and the shell resets
/// the terminal before printing its prompt.
#[test]
fn a_soft_reset_returns_the_pen_to_its_default() {
    let device = interpret(b"\x1b[1;31m\x1b[!px");
    let cell = device.active_screen().viewport_row(ViewportLine(0))[0];
    assert_eq!(cell.fg, Color::DefaultForeground);
    assert_eq!(cell.style, Style::empty());
}

/// Asserts that a soft reset returns the saved cursor to the home
/// position with a default pen.
///
/// Case: a program saves its cursor mid-screen with a coloured pen, and
/// the shell resets the terminal.
#[test]
fn a_soft_reset_returns_the_saved_cursor_to_home() {
    let device = interpret(b"\x1b[2;3H\x1b[31m\x1b7\x1b[!p\x1b8x");
    let cell = device.active_screen().viewport_row(ViewportLine(0))[0];
    assert_eq!(cell.c, 'x');
    assert_eq!(cell.fg, Color::DefaultForeground);
}
