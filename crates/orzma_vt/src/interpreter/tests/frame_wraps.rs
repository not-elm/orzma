//! Tests for the viewport wrap list a frame carries: it follows every
//! change to the rows' recorded wraps, including the ones no row damage
//! reports.

use super::reflow_invariants::{Traffic, bytes_of, traffic_strategy};
use super::*;
use crate::screen::viewport::Scroll;
use proptest::prelude::*;

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

/// One step of a session: traffic from the program, a viewport scroll,
/// a resize, or a full reset.
#[derive(Debug, Clone)]
enum Step {
    Traffic(Traffic),
    Scroll(i32),
    Resize(u16, u16),
    Reset,
}

fn step_strategy() -> impl Strategy<Value = Step> {
    prop_oneof![
        12 => traffic_strategy().prop_map(Step::Traffic),
        2 => (-4i32..=4).prop_map(Step::Scroll),
        1 => (2u16..10, 1u16..6).prop_map(|(cols, rows)| Step::Resize(cols, rows)),
        1 => Just(Step::Reset),
    ]
}

/// What a consumer that applied every emitted frame knows about the
/// viewport's wraps.
#[derive(Default)]
struct Mirror {
    wraps: Vec<Option<u16>>,
    continues_from_above: bool,
    rows: u16,
}

impl Mirror {
    fn apply(&mut self, frame: &Frame) {
        if let Some(wraps) = &frame.wraps {
            self.wraps.clone_from(wraps);
        }
        self.continues_from_above = frame.continues_from_above;
        self.rows = frame.size.rows;
    }
}

/// Checks that `mirror` holds what the grid records for the rows the
/// viewport shows, reading the grid directly rather than through the
/// accessors the tracker uses.
fn check(vt: &OrzmaVt, mirror: &Mirror) -> Result<(), TestCaseError> {
    let screen = vt.device.active_screen();
    let grid = screen.grid();
    let top = -i32::try_from(screen.display_offset().0).expect("scrollback fits an i32");
    let wraps: Vec<Option<u16>> = (0..i32::from(grid.size().rows))
        .map(|line| grid.wrap_at(GridLine(top + line)))
        .collect();
    prop_assert_eq!(&mirror.wraps, &wraps);
    prop_assert_eq!(mirror.wraps.len(), usize::from(mirror.rows));
    prop_assert_eq!(
        mirror.continues_from_above,
        grid.wrap_at(GridLine(top - 1)).is_some()
    );
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// Asserts that after every emitted frame the consumer's wrap list and
    /// top-row flag equal what the grid records, with one wrap entry per
    /// viewport row.
    ///
    /// Case: a shell and full-screen programs print, erase, edit, scroll
    /// and reset across wrapped lines while the user scrolls back and
    /// resizes the window.
    #[test]
    fn the_consumer_wraps_always_match_the_grid(
        steps in prop::collection::vec((step_strategy(), any::<bool>()), 1..160),
    ) {
        let mut vt = OrzmaVt::new(GridSize { cols: 6, rows: 3 }, 20);
        let mut mirror = Mirror::default();
        for (step, emit) in &steps {
            match step {
                Step::Traffic(traffic) => {
                    vt.interpret(&bytes_of(traffic));
                }
                Step::Scroll(delta) => {
                    vt.scroll(Scroll::Delta(*delta));
                }
                Step::Resize(cols, rows) => {
                    let _ = vt.resize(GridSize { cols: *cols, rows: *rows });
                }
                Step::Reset => {
                    vt.interpret(b"\x1bc");
                }
            }
            if *emit {
                if let Some(frame) = vt.frame() {
                    mirror.apply(&frame);
                }
                check(&vt, &mirror)?;
            }
        }
        if let Some(frame) = vt.frame() {
            mirror.apply(&frame);
        }
        check(&vt, &mirror)?;
    }
}
