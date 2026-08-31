//! Tests for saving the `DECSC` checkpoint.

use super::*;

/// Asserts that a save copies aside every item `DECSC` lists.
///
/// Case: a full-screen application saves its cursor before
/// drawing a status line in another color and character set.
#[test]
fn a_save_copies_every_item_decsc_lists() {
    let mut screen = dirty_screen();
    screen.save_checkpoint();
    assert_eq!(screen.checkpoint.line, ScreenLine(2));
    assert_eq!(screen.checkpoint.column, GridColumn(3));
    assert_eq!(screen.checkpoint.pen.bg, Color::Indexed(4));
    assert!(screen.checkpoint.pending_wrap);
    assert_eq!(screen.checkpoint.origin_mode, OriginMode::WithinMargins);
    assert_eq!(screen.checkpoint.character_set_mapping.gl, GCode::G1);
}

/// Asserts that work done after a save leaves the saved copy
/// alone.
///
/// Case: an application saves its cursor and then keeps printing,
/// expecting the save to still describe where it was.
#[test]
fn later_work_does_not_reach_the_saved_copy() {
    let mut screen = dirty_screen();
    screen.save_checkpoint();
    screen.state.line = ScreenLine(0);
    screen.pen_mut().bg = Color::DefaultBackground;
    screen.invoke_character_set(GCode::G0);
    assert_eq!(screen.checkpoint.line, ScreenLine(2));
    assert_eq!(screen.checkpoint.pen.bg, Color::Indexed(4));
    assert_eq!(screen.checkpoint.character_set_mapping.gl, GCode::G1);
}
