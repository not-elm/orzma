//! Unit tests for [`Screen`], one module per method under test.

use super::*;
use crate::device::color::Color;
use crate::screen::margins::Margins;

fn screen() -> Screen {
    Screen::new(GridSize { cols: 4, rows: 3 }, 10)
}

/// Twenty columns put the right edge at 19, so the default stride's
/// stops at 8 and 16 are reachable and the one at 24 is not.
fn wide_screen() -> Screen {
    Screen::new(GridSize { cols: 20, rows: 3 }, 10)
}

/// Four rows leave two rows below a bottom margin at row 1, so a
/// cursor outside the region has somewhere left to move down to.
fn tall_screen() -> Screen {
    Screen::new(GridSize { cols: 4, rows: 4 }, 10)
}

/// Moves every item `DECSC` saves off its default, so a later
/// assertion that the state came back cannot pass by accident.
fn dirty_screen() -> Screen {
    let mut screen = screen();
    screen.state.line = ScreenLine(2);
    screen.state.column = GridColumn(3);
    screen.state.pending_wrap = true;
    screen.pen_mut().bg = Color::Indexed(4);
    screen
        .scroll_region
        .set_origin_mode(OriginMode::WithinMargins);
    screen.invoke_character_set(GCode::G1);
    screen.designate_character_set(GCode::G1, CharacterSet::DecSpecialGraphics);
    screen
}

mod backspace;
mod carriage_return;
mod cursor;
mod display_offset;
mod erase_in_display;
mod erase_in_line;
mod fill_alignment_pattern;
mod line_feed;
mod move_backward_tabs;
mod move_cursor_to;
mod move_forward_tabs;
mod new;
mod placements;
mod print;
mod reset;
mod resize;
mod restore_checkpoint;
mod reverse_index;
mod save_checkpoint;
mod scroll;
mod seat_cursor;
mod set_origin_mode;
mod set_scroll_region;
mod tab_stop_edits;
mod tab_to;
mod viewport_row;
