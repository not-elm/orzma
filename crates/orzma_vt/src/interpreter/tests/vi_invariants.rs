//! Property tests: whatever the user and the program do in vi mode, the
//! vi cursor stays inside the viewport, never names a continuation
//! column, and every move of it is reported.

use super::*;
use crate::screen::cell::CellWidth;
use crate::screen::grid::coords::GridPoint;
use crate::screen::selection::{CellSide, SelectionKind};
use crate::screen::vi::{ViCursor, ViModeSwitch, ViMotion};
use crate::screen::viewport::Scroll;
use proptest::prelude::*;

/// One step of a vi-mode session: a key the user presses, a click or a
/// drag on a visible cell, a resize, or a piece of program output.
#[derive(Debug, Clone)]
enum Step {
    Motion(ViMotion),
    Scroll(Scroll),
    Toggle(SelectionKind),
    Click(u16, u16),
    Drag(u16, u16),
    Resize(u16, u16),
    Output(&'static [u8]),
}

const MOTIONS: [ViMotion; 21] = [
    ViMotion::Up,
    ViMotion::Down,
    ViMotion::Left,
    ViMotion::Right,
    ViMotion::First,
    ViMotion::Last,
    ViMotion::FirstOccupied,
    ViMotion::High,
    ViMotion::Middle,
    ViMotion::Low,
    ViMotion::SemanticLeft,
    ViMotion::SemanticRight,
    ViMotion::SemanticLeftEnd,
    ViMotion::SemanticRightEnd,
    ViMotion::WordLeft,
    ViMotion::WordRight,
    ViMotion::WordLeftEnd,
    ViMotion::WordRightEnd,
    ViMotion::Bracket,
    ViMotion::ParagraphUp,
    ViMotion::ParagraphDown,
];

const OUTPUTS: [&[u8]; 12] = [
    b"ab (c)",
    "あい".as_bytes(),
    b"\r\n",
    b"\x1b[T",
    b"\x1bM",
    b"\x1b[L",
    b"\x1b[M",
    b"\x1b[2;3r",
    b"\x1b[r",
    b"\x1b[?1049h",
    b"\x1b[?1049l",
    b"\x1bc",
];

fn step_strategy() -> impl Strategy<Value = Step> {
    prop_oneof![
        8 => prop::sample::select(MOTIONS.to_vec()).prop_map(Step::Motion),
        3 => prop_oneof![
            (-3i32..4).prop_map(Scroll::Delta),
            Just(Scroll::PageUp),
            Just(Scroll::PageDown),
            Just(Scroll::HalfPageUp),
            Just(Scroll::HalfPageDown),
            Just(Scroll::Top),
            Just(Scroll::Bottom),
        ]
        .prop_map(Step::Scroll),
        2 => prop_oneof![Just(SelectionKind::Simple), Just(SelectionKind::Lines)]
            .prop_map(Step::Toggle),
        1 => (0u16..12, 0u16..8).prop_map(|(column, row)| Step::Click(column, row)),
        1 => (0u16..12, 0u16..8).prop_map(|(column, row)| Step::Drag(column, row)),
        1 => (2u16..12, 1u16..8).prop_map(|(cols, rows)| Step::Resize(cols, rows)),
        6 => prop::sample::select(OUTPUTS.to_vec()).prop_map(Step::Output),
    ]
}

/// The grid point the visible cell (`column`, `row`) shows, folded into
/// the current grid.
fn visible_point(vt: &OrzmaVt, column: u16, row: u16) -> GridPoint {
    let size = vt.grid_size();
    GridPoint {
        line: ViewportLine(row % size.rows).to_grid(vt.display_offset()),
        column: GridColumn(column % size.cols),
    }
}

/// Applies `step`, returning whether the terminal reported a change.
fn apply(vt: &mut OrzmaVt, step: &Step) -> bool {
    match step {
        Step::Motion(motion) => vt.vi_motion(*motion),
        Step::Scroll(scroll) => vt.scroll(*scroll),
        Step::Toggle(kind) => vt.toggle_vi_selection(*kind),
        Step::Click(column, row) => {
            let cell = visible_point(vt, *column, *row);
            vt.start_selection(cell, CellSide::Left, SelectionKind::Simple)
        }
        Step::Drag(column, row) => {
            let cell = visible_point(vt, *column, *row);
            vt.extend_selection(cell, CellSide::Right)
        }
        Step::Resize(cols, rows) => vt
            .resize(GridSize {
                cols: *cols,
                rows: *rows,
            })
            .is_some(),
        Step::Output(bytes) => vt.interpret(bytes).damaged,
    }
}

/// Checks that vi mode is on and that the vi cursor lies inside the
/// viewport, inside the row, and on a glyph's first column.
fn check_vi_cursor(vt: &OrzmaVt) -> Result<ViCursor, TestCaseError> {
    let Some(cursor) = vt.vi_cursor() else {
        return Err(TestCaseError::fail("vi mode ended"));
    };
    let size = vt.grid_size();
    let offset = vt.display_offset();
    prop_assert!(
        cursor.point.line.to_viewport(offset, size.rows).is_some(),
        "line {} outside the viewport at offset {}",
        cursor.point.line.0,
        offset.0
    );
    prop_assert!(
        cursor.point.column.0 < size.cols,
        "column {} past the row",
        cursor.point.column.0
    );
    let cells: &[Cell] = vt.device.active_screen().grid().row(cursor.point.line);
    prop_assert_ne!(
        cells[usize::from(cursor.point.column.0)].width,
        CellWidth::Spacer,
        "the vi cursor names a continuation column"
    );
    Ok(cursor)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Asserts that after every step of a vi-mode session the vi cursor
    /// lies inside the viewport on a glyph's first column, and that every
    /// step that moves it reports a change and emits a frame carrying it.
    ///
    /// Case: the user moves, scrolls, selects, clicks, drags, and resizes
    /// in vi mode while a program prints, scrolls regions, flips screens,
    /// and resets, on terminals whose scrollback ranges from none to plenty.
    #[test]
    fn the_vi_cursor_stays_in_view_and_every_move_is_reported(
        cap in prop_oneof![Just(0usize), Just(2usize), Just(100usize)],
        steps in prop::collection::vec(step_strategy(), 1..80),
    ) {
        let mut vt = OrzmaVt::new(GridSize { cols: 8, rows: 4 }, cap);
        vt.interpret("one two\r\nthree (4)\r\nあい five\r\nsix\r\nseven\r\n".as_bytes());
        vt.switch_vi_mode(ViModeSwitch::Enter);
        vt.frame();
        let mut before = check_vi_cursor(&vt)?;
        for step in &steps {
            let reported = apply(&mut vt, step);
            let after = check_vi_cursor(&vt)?;
            let frame = vt.frame();
            if after != before {
                prop_assert!(reported, "{:?} moved the vi cursor silently", step);
                prop_assert_eq!(
                    frame.and_then(|frame| frame.vi_cursor),
                    Some(after),
                    "{:?} emitted no frame carrying the moved vi cursor",
                    step
                );
            }
            before = after;
        }
    }
}
