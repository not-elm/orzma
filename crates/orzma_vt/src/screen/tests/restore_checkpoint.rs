//! Tests for restoring the `DECSC` checkpoint.

use super::*;

/// Asserts that a restore puts back every item the save copied
/// aside.
///
/// Case: an application finishes drawing its status line and
/// returns to where it was working.
#[test]
fn a_restore_puts_back_every_saved_item() {
    let mut screen = dirty_screen();
    screen.save_checkpoint();
    screen.state.line = ScreenLine(0);
    screen.state.column = GridColumn(0);
    screen.state.pending_wrap = false;
    screen.pen_mut().bg = Color::DefaultBackground;
    screen
        .scroll_region
        .set_origin_mode(OriginMode::UpperLeftCorner);
    screen.invoke_character_set(GCode::G0);
    screen.restore_checkpoint();
    assert_eq!(screen.state.line, ScreenLine(2));
    assert_eq!(screen.state.column, GridColumn(3));
    assert_eq!(screen.state.pen.bg, Color::Indexed(4));
    assert!(screen.state.pending_wrap);
    assert_eq!(
        screen.scroll_region.origin_mode(),
        OriginMode::WithinMargins
    );
    assert_eq!(screen.character_set_mapping.gl, GCode::G1);
}

/// Asserts that a restore with nothing ever saved returns the
/// screen to its power-up state rather than being ignored.
///
/// Case: an application emits a restore during start-up, before
/// it has ever saved anything.
#[test]
fn an_unsaved_restore_returns_the_power_up_state() {
    let mut screen = dirty_screen();
    screen.restore_checkpoint();
    assert_eq!(screen.state.line, ScreenLine(0));
    assert_eq!(screen.state.column, GridColumn(0));
    assert_eq!(screen.state.pen, Pen::default());
    assert!(!screen.state.pending_wrap);
    assert_eq!(
        screen.scroll_region.origin_mode(),
        OriginMode::UpperLeftCorner
    );
    assert_eq!(screen.character_set_mapping, CharacterSetMapping::default());
}

/// Asserts that a restored deferred wrap really wraps the next
/// character.
///
/// The flag is pinned through behaviour rather than by reading it
/// back, because only the wrap it produces is observable to the
/// application that saved it.
///
/// Case: an application fills a row to its last column, saves,
/// goes away to draw elsewhere, restores, and prints one more
/// character.
#[test]
fn a_restored_deferred_wrap_still_wraps_the_next_character() {
    let mut screen = screen();
    for c in ['a', 'b', 'c', 'd'] {
        screen.print(c, InsertReplaceMode::Replace, AutoWrap::Enabled);
    }
    screen.save_checkpoint();
    screen.state.line = ScreenLine(2);
    screen.state.column = GridColumn(0);
    screen.state.pending_wrap = false;
    screen.restore_checkpoint();
    screen.print('e', InsertReplaceMode::Replace, AutoWrap::Enabled);
    assert_eq!(screen.grid[ScreenLine(1)][0].c, 'e');
    assert_eq!(screen.state.line, ScreenLine(1));
}
