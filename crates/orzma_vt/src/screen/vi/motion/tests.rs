//! Unit tests for [`MotionGrid`].

use super::*;
use crate::screen::grid::GridSize;
use crate::screen::grid::coords::ScreenLine;

/// A blank 20×20 grid with room for 100 history rows.
fn grid() -> Grid {
    Grid::new(GridSize { cols: 20, rows: 20 }, 100)
}

fn point(line: i32, column: u16) -> GridPoint {
    GridPoint {
        line: GridLine(line),
        column: GridColumn(column),
    }
}

fn put(grid: &mut Grid, line: u16, column: u16, c: char) {
    grid[ScreenLine(line)][column].c = c;
}

/// Writes a wide glyph whose body sits at `column` and whose continuation
/// sits at `column + 1`.
fn put_wide(grid: &mut Grid, line: u16, column: u16, c: char) {
    grid[ScreenLine(line)][column].c = c;
    grid[ScreenLine(line)][column].width = CellWidth::Wide;
    grid[ScreenLine(line)][column + 1].c = ' ';
    grid[ScreenLine(line)][column + 1].width = CellWidth::Spacer;
}

/// Scrolls `count` blank rows off the top into history.
fn push_history(grid: &mut Grid, count: usize) {
    let bottom = ScreenLine(grid.size().rows - 1);
    for _ in 0..count {
        grid.scroll_up_one(ScreenLine(0), bottom, Cell::default());
    }
}

/// Applies each of `motions` in turn from `from`, returning every point
/// visited.
fn walk(grid: &Grid, from: GridPoint, motions: &[ViMotion]) -> Vec<GridPoint> {
    walk_with(grid, &SemanticEscapeChars::default(), from, motions)
}

fn walk_with(
    grid: &Grid,
    chars: &SemanticEscapeChars,
    from: GridPoint,
    motions: &[ViMotion],
) -> Vec<GridPoint> {
    let motion_grid = MotionGrid::new(grid, DisplayOffset(0), chars);
    let mut at = from;
    motions
        .iter()
        .map(|motion| {
            at = motion_grid.apply(at, *motion);
            at
        })
        .collect()
}

/// The row of the semantic-motion fixture: `x xx  : x:x  : x`.
fn semantic_grid() -> Grid {
    let mut grid = grid();
    for (column, c) in "x xx  : x:x  : x".chars().enumerate() {
        put(&mut grid, 0, u16::try_from(column).expect("a short row"), c);
    }
    grid
}

/// Asserts that a step in each direction moves the vi cursor one cell.
///
/// Case: the user presses `l`, `h`, `j`, and `k` on a blank screen.
#[test]
fn a_step_in_each_direction_moves_one_cell() {
    let grid = grid();
    let steps = [
        ViMotion::Right,
        ViMotion::Left,
        ViMotion::Down,
        ViMotion::Up,
    ];
    assert_eq!(
        walk(&grid, point(0, 0), &steps),
        vec![point(0, 1), point(0, 0), point(1, 0), point(0, 0)]
    );
}

/// Asserts that a one-cell step over a wide glyph moves by the whole
/// glyph in either direction.
///
/// Case: the user presses `l` on a CJK character, and `h` from the
/// character after it.
#[test]
fn a_step_over_a_wide_glyph_moves_one_whole_glyph() {
    let mut grid = grid();
    put(&mut grid, 0, 0, 'a');
    put_wide(&mut grid, 0, 1, '汉');
    put(&mut grid, 0, 3, 'a');
    assert_eq!(
        walk(&grid, point(0, 1), &[ViMotion::Right]),
        vec![point(0, 3)]
    );
    assert_eq!(
        walk(&grid, point(0, 2), &[ViMotion::Left]),
        vec![point(0, 0)]
    );
}

/// Asserts that `Last` and `First` reach the last and first columns of a
/// blank row.
///
/// Case: the user presses `$` and then `0` on an empty line.
#[test]
fn first_and_last_reach_the_row_edges() {
    let grid = grid();
    assert_eq!(
        walk(&grid, point(0, 0), &[ViMotion::Last, ViMotion::First]),
        vec![point(0, 19), point(0, 0)]
    );
}

/// Asserts that `FirstOccupied` goes to the row's first non-blank cell,
/// and from there to the first non-blank cell of the logical line the
/// row continues.
///
/// Case: the user presses `^` twice on the last row of a long wrapped
/// command.
#[test]
fn first_occupied_crosses_to_the_wrapped_start() {
    let mut grid = grid();
    put(&mut grid, 0, 1, 'x');
    put(&mut grid, 0, 3, 'y');
    grid.set_wrap_at(GridLine(0), 20);
    grid.set_wrap_at(GridLine(1), 20);
    put(&mut grid, 2, 0, 'z');
    assert_eq!(
        walk(
            &grid,
            point(2, 1),
            &[ViMotion::FirstOccupied, ViMotion::FirstOccupied]
        ),
        vec![point(2, 0), point(0, 1)]
    );
}

/// Asserts that `High`, `Middle`, and `Low` land on the viewport's top,
/// middle, and bottom rows.
///
/// Case: the user presses `H`, `M`, and `L` on a 20-row terminal.
#[test]
fn high_middle_and_low_land_on_the_viewport_rows() {
    let grid = grid();
    assert_eq!(
        walk(
            &grid,
            point(0, 0),
            &[ViMotion::High, ViMotion::Middle, ViMotion::Low]
        ),
        vec![point(0, 0), point(9, 0), point(19, 0)]
    );
}

/// Asserts that `Middle` on a one-row grid stays on that row rather than
/// naming the row above it.
///
/// Case: the user shrinks the window to a single row and presses `M`.
#[test]
fn middle_on_a_one_row_grid_stays_on_that_row() {
    let grid = Grid::new(GridSize { cols: 20, rows: 1 }, 100);
    assert_eq!(
        walk(&grid, point(0, 5), &[ViMotion::Middle]),
        vec![point(0, 0)]
    );
}

/// Asserts that `Bracket` jumps to the matching bracket and back.
///
/// Case: the user presses `%` on an opening parenthesis and again on the
/// closing one.
#[test]
fn a_bracket_jumps_to_its_partner_and_back() {
    let mut grid = grid();
    put(&mut grid, 0, 0, '(');
    put(&mut grid, 0, 1, 'x');
    put(&mut grid, 0, 2, ')');
    assert_eq!(
        walk(&grid, point(0, 0), &[ViMotion::Bracket, ViMotion::Bracket]),
        vec![point(0, 2), point(0, 0)]
    );
}

/// Asserts that `SemanticRightEnd` stops at the end of each semantic word
/// and on each separator.
///
/// Case: the user presses `e` repeatedly along a line mixing words and
/// colons.
#[test]
fn semantic_right_end_stops_at_each_word_end() {
    let grid = semantic_grid();
    let steps = [ViMotion::SemanticRightEnd; 7];
    assert_eq!(
        walk(&grid, point(0, 0), &steps),
        [3, 6, 8, 9, 10, 13, 15]
            .map(|column| point(0, column))
            .to_vec()
    );
}

/// Asserts that `SemanticLeft` stops at the start of each semantic word
/// and on each separator.
///
/// Case: the user presses `b` repeatedly from the end of a line mixing
/// words and colons.
#[test]
fn semantic_left_stops_at_each_word_start() {
    let grid = semantic_grid();
    let steps = [ViMotion::SemanticLeft; 7];
    assert_eq!(
        walk(&grid, point(0, 15), &steps),
        [13, 10, 9, 8, 6, 2, 0]
            .map(|column| point(0, column))
            .to_vec()
    );
}

/// Asserts that `SemanticRight` stops at the start of each following
/// semantic word and on each separator.
///
/// Case: the user presses `w` repeatedly along a line mixing words and
/// colons.
#[test]
fn semantic_right_stops_at_each_word_start() {
    let grid = semantic_grid();
    let steps = [ViMotion::SemanticRight; 7];
    assert_eq!(
        walk(&grid, point(0, 0), &steps),
        [2, 6, 8, 9, 10, 13, 15]
            .map(|column| point(0, column))
            .to_vec()
    );
}

/// Asserts that `SemanticLeftEnd` stops at the end of each preceding
/// semantic word and on each separator.
///
/// Case: the user walks backward word end by word end along a line mixing
/// words and colons.
#[test]
fn semantic_left_end_stops_at_each_word_end() {
    let grid = semantic_grid();
    let steps = [ViMotion::SemanticLeftEnd; 7];
    assert_eq!(
        walk(&grid, point(0, 15), &steps),
        [13, 10, 9, 8, 6, 3, 0]
            .map(|column| point(0, column))
            .to_vec()
    );
}

/// Asserts that semantic motions cross blank rows into history and back
/// to the bottom-right cell.
///
/// Case: the screen above the vi cursor is blank, with five blank rows of
/// scrollback, and the user presses `b`, `w`, and `e`.
#[test]
fn a_semantic_motion_crosses_into_history() {
    let mut grid = grid();
    push_history(&mut grid, 5);
    let steps = [
        ViMotion::SemanticLeft,
        ViMotion::SemanticRight,
        ViMotion::SemanticLeftEnd,
        ViMotion::SemanticRightEnd,
    ];
    assert_eq!(
        walk(&grid, point(0, 0), &steps),
        vec![point(-5, 0), point(19, 19), point(-5, 0), point(19, 19)]
    );
}

/// Asserts that a semantic motion steps over a wide glyph as one word.
///
/// Case: the user presses `w` on a CJK word and `b` from its continuation
/// column.
#[test]
fn a_semantic_motion_steps_over_a_wide_glyph() {
    let mut grid = grid();
    put(&mut grid, 0, 0, 'a');
    put_wide(&mut grid, 0, 2, '汉');
    put(&mut grid, 0, 5, 'a');
    assert_eq!(
        walk(&grid, point(0, 2), &[ViMotion::SemanticRight]),
        vec![point(0, 5)]
    );
    assert_eq!(
        walk(&grid, point(0, 3), &[ViMotion::SemanticLeft]),
        vec![point(0, 0)]
    );
}

/// Asserts that a configured wide separator ends a semantic word.
///
/// Case: the user adds the full-width hyphen to the word separators and
/// walks across it with `w` and `b`.
#[test]
fn a_wide_separator_ends_a_semantic_word() {
    let mut grid = grid();
    put(&mut grid, 0, 0, 'x');
    put(&mut grid, 0, 1, 'x');
    put_wide(&mut grid, 0, 2, '－');
    put(&mut grid, 0, 4, 'x');
    put(&mut grid, 0, 5, 'x');
    let chars = SemanticEscapeChars::new("－");
    let right = [ViMotion::SemanticRight];
    let left = [ViMotion::SemanticLeft];
    assert_eq!(
        walk_with(&grid, &chars, point(0, 0), &right),
        vec![point(0, 2)]
    );
    assert_eq!(
        walk_with(&grid, &chars, point(0, 2), &right),
        vec![point(0, 4)]
    );
    assert_eq!(
        walk_with(&grid, &chars, point(0, 5), &left),
        vec![point(0, 4)]
    );
    assert_eq!(
        walk_with(&grid, &chars, point(0, 4), &left),
        vec![point(0, 2)]
    );
    assert_eq!(
        walk_with(&grid, &chars, point(0, 2), &left),
        vec![point(0, 0)]
    );
}

/// Asserts that whitespace ends a semantic word even when the configured
/// separators leave it out.
///
/// Case: the user has configured only `-` as a word separator and presses
/// `w` at the start of `ab cd-ef`.
#[test]
fn whitespace_ends_a_semantic_word_outside_the_configured_separators() {
    let mut grid = grid();
    for (column, c) in "ab cd-ef".chars().enumerate() {
        put(&mut grid, 0, u16::try_from(column).expect("a short row"), c);
    }
    let chars = SemanticEscapeChars::new("-");
    assert_eq!(
        walk_with(&grid, &chars, point(0, 0), &[ViMotion::SemanticRight]),
        vec![point(0, 3)]
    );
}

/// Asserts that the whitespace-word motions treat punctuation as part of
/// a word.
///
/// Case: the user walks a line of `a;  a;` with `E`, `B`, `W`, and the
/// backward word-end motion.
#[test]
fn word_motions_split_only_at_whitespace() {
    let mut grid = grid();
    for (column, c) in "a;  a;".chars().enumerate() {
        put(&mut grid, 0, u16::try_from(column).expect("a short row"), c);
    }
    let steps = [
        ViMotion::WordRightEnd,
        ViMotion::WordRightEnd,
        ViMotion::WordLeft,
        ViMotion::WordLeft,
        ViMotion::WordRight,
        ViMotion::WordLeftEnd,
    ];
    assert_eq!(
        walk(&grid, point(0, 0), &steps),
        [1, 5, 4, 0, 4, 1].map(|column| point(0, column)).to_vec()
    );
}

/// Asserts that whitespace-word motions cross blank rows into history and
/// back to the bottom-right cell.
///
/// Case: the screen above the vi cursor is blank, with five blank rows of
/// scrollback, and the user presses `B`, `W`, and `E`.
#[test]
fn a_word_motion_crosses_into_history() {
    let mut grid = grid();
    push_history(&mut grid, 5);
    let steps = [
        ViMotion::WordLeft,
        ViMotion::WordRight,
        ViMotion::WordLeftEnd,
        ViMotion::WordRightEnd,
    ];
    assert_eq!(
        walk(&grid, point(0, 0), &steps),
        vec![point(-5, 0), point(19, 19), point(-5, 0), point(19, 19)]
    );
}

/// Asserts that a whitespace-word motion steps over a wide glyph as one
/// word.
///
/// Case: the user presses `W` on a CJK word and `B` from its continuation
/// column.
#[test]
fn a_word_motion_steps_over_a_wide_glyph() {
    let mut grid = grid();
    put(&mut grid, 0, 0, 'a');
    put_wide(&mut grid, 0, 2, '汉');
    put(&mut grid, 0, 5, 'a');
    assert_eq!(
        walk(&grid, point(0, 2), &[ViMotion::WordRight]),
        vec![point(0, 5)]
    );
    assert_eq!(
        walk(&grid, point(0, 3), &[ViMotion::WordLeft]),
        vec![point(0, 0)]
    );
}

/// Asserts that a page target moves by the requested rows and stops at
/// the top of history and the bottom row.
///
/// Case: the user pages up and down through forty rows of scrollback.
#[test]
fn page_targets_stop_at_the_grid_edges() {
    let mut grid = grid();
    push_history(&mut grid, 40);
    let chars = SemanticEscapeChars::default();
    let motion_grid = MotionGrid::new(&grid, DisplayOffset(0), &chars);
    let mut at = point(19, 0);
    let up: Vec<GridPoint> = (0..4)
        .map(|_| {
            at = motion_grid.scroll_target(at, 20);
            at
        })
        .collect();
    assert_eq!(
        up,
        vec![point(-1, 0), point(-21, 0), point(-40, 0), point(-40, 0)]
    );
    let down: Vec<GridPoint> = (0..4)
        .map(|_| {
            at = motion_grid.scroll_target(at, -20);
            at
        })
        .collect();
    assert_eq!(
        down,
        vec![point(-20, 0), point(0, 0), point(19, 0), point(19, 0)]
    );
}

/// Asserts that `Up` stops at the top of history and `Down` at the bottom
/// row.
///
/// Case: the user holds `k` past the oldest row and `j` past the newest.
#[test]
fn up_and_down_stop_at_the_grid_edges() {
    let mut grid = grid();
    push_history(&mut grid, 1);
    assert_eq!(
        walk(&grid, point(-1, 0), &[ViMotion::Up]),
        vec![point(-1, 0)]
    );
    assert_eq!(
        walk(&grid, point(19, 0), &[ViMotion::Down]),
        vec![point(19, 0)]
    );
}

/// Asserts that the paragraph motions stop on the blank row between two
/// blocks of text.
///
/// Case: the user presses `{` and `}` in output made of two paragraphs.
#[test]
fn paragraph_motions_stop_at_the_blank_row_between_blocks() {
    let mut grid = grid();
    for line in [0, 1, 2, 4, 5] {
        put(&mut grid, line, 0, 'x');
    }
    assert_eq!(
        walk(&grid, point(5, 3), &[ViMotion::ParagraphUp]),
        vec![point(3, 0)]
    );
    assert_eq!(
        walk(&grid, point(0, 3), &[ViMotion::ParagraphDown]),
        vec![point(3, 0)]
    );
}

/// Asserts that a paragraph motion passes over a blank row with a colored
/// background and over a blank row that wraps, stopping on the first
/// plain blank row.
///
/// Case: the user presses `}` in output where a program painted a row of
/// colored spaces and a long line wraps through a row of spaces.
#[test]
fn paragraph_motions_pass_colored_and_wrapped_blank_rows() {
    let mut grid = grid();
    put(&mut grid, 0, 0, 'x');
    for column in 0..20 {
        grid[ScreenLine(1)][column].bg = Color::Indexed(4);
    }
    grid.set_wrap_at(GridLine(2), 20);
    put(&mut grid, 4, 0, 'x');
    assert_eq!(
        walk(&grid, point(0, 0), &[ViMotion::ParagraphDown]),
        vec![point(3, 0)]
    );
}

/// Asserts that a row ending in a leading spacer counts as wrapped at its
/// last column only, and that `h` from the wide glyph it pushed down
/// steps over the spacer.
///
/// Case: a wide glyph that did not fit at the end of a row moved to the
/// next row, and the user presses `h` on it.
#[test]
fn a_row_ending_in_a_leading_spacer_wraps_at_its_last_column() {
    let mut grid = grid();
    grid[ScreenLine(0)][19].width = CellWidth::LeadingSpacer;
    grid.set_wrap_at(GridLine(0), 19);
    put_wide(&mut grid, 1, 0, '汉');
    let chars = SemanticEscapeChars::default();
    let motion_grid = MotionGrid::new(&grid, DisplayOffset(0), &chars);
    assert!(motion_grid.is_wrap(point(0, 19)));
    assert!(!motion_grid.is_wrap(point(0, 18)));
    assert_eq!(
        walk(&grid, point(1, 0), &[ViMotion::Left]),
        vec![point(0, 18)]
    );
}

/// Asserts that word and paragraph motions on a blank grid stop at the
/// grid's edges.
///
/// Case: the user enters vi mode on a freshly opened pane and presses
/// `w`, `b`, and `}`.
#[test]
fn motions_on_a_blank_grid_stop_at_the_grid_edges() {
    let grid = grid();
    assert_eq!(
        walk(&grid, point(0, 0), &[ViMotion::WordRight]),
        vec![point(19, 19)]
    );
    assert_eq!(
        walk(&grid, point(0, 0), &[ViMotion::SemanticLeft]),
        vec![point(0, 0)]
    );
    assert_eq!(
        walk(&grid, point(0, 0), &[ViMotion::ParagraphDown]),
        vec![point(19, 0)]
    );
}
