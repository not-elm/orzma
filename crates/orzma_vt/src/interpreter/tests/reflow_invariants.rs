//! Property tests: a resize never changes the text of any logical line,
//! a narrowing undone restores every cell, and every reflow keeps the
//! grid's structural invariants.

use super::*;
use crate::device::modes::ScreenKind;
use crate::screen::grid::reflow::ScrollbackOnGrow;
use crate::screen::grid::{Grid, LineId};
use proptest::prelude::*;
use std::collections::HashSet;
use std::ops::Range;

/// One piece of the traffic a program sends: output, cursor motion,
/// erasure, editing, margins, scrolling, pen changes, cursor saves, and
/// screen flips.
#[derive(Debug, Clone)]
enum Traffic {
    Ascii(u8),
    Cjk,
    Mark,
    Newline,
    Cup(u8, u8),
    El(u8),
    Ed(u8),
    Ich(u8),
    Dch(u8),
    Il(u8),
    Dl(u8),
    Ech(u8),
    Stbm(u8, u8),
    StbmReset,
    Ri,
    Ind,
    Sgr(u8),
    Sc,
    Rc,
    Alternate(bool),
    Cr,
    Bs,
    OriginMode(bool),
    Cuu(u8),
    Cud(u8),
}

/// Plain output: ASCII letters, a Japanese glyph, a combining mark, and
/// newlines.
fn output_strategy() -> impl Strategy<Value = Traffic> {
    prop_oneof![
        6 => (b'a'..=b'z').prop_map(Traffic::Ascii),
        2 => Just(Traffic::Cjk),
        1 => Just(Traffic::Mark),
        1 => Just(Traffic::Newline),
    ]
}

fn traffic_strategy() -> impl Strategy<Value = Traffic> {
    prop_oneof![
        20 => (b'a'..=b'z').prop_map(Traffic::Ascii),
        4 => Just(Traffic::Cjk),
        1 => Just(Traffic::Mark),
        4 => Just(Traffic::Newline),
        2 => (0u8..8, 0u8..14).prop_map(|(row, column)| Traffic::Cup(row, column)),
        1 => (0u8..3).prop_map(Traffic::El),
        1 => (0u8..3).prop_map(Traffic::Ed),
        1 => (1u8..4).prop_map(Traffic::Ich),
        1 => (1u8..4).prop_map(Traffic::Dch),
        1 => (1u8..3).prop_map(Traffic::Il),
        1 => (1u8..3).prop_map(Traffic::Dl),
        1 => (1u8..4).prop_map(Traffic::Ech),
        1 => (1u8..5, 1u8..8).prop_map(|(top, bottom)| Traffic::Stbm(top, bottom)),
        1 => Just(Traffic::StbmReset),
        1 => Just(Traffic::Ri),
        1 => Just(Traffic::Ind),
        2 => (0u8..4).prop_map(Traffic::Sgr),
        1 => Just(Traffic::Sc),
        1 => Just(Traffic::Rc),
        1 => any::<bool>().prop_map(Traffic::Alternate),
        1 => Just(Traffic::Cr),
        1 => Just(Traffic::Bs),
        1 => any::<bool>().prop_map(Traffic::OriginMode),
        1 => (1u8..4).prop_map(Traffic::Cuu),
        1 => (1u8..4).prop_map(Traffic::Cud),
    ]
}

fn bytes_of(traffic: &Traffic) -> Vec<u8> {
    match traffic {
        Traffic::Ascii(byte) => vec![*byte],
        Traffic::Cjk => "あ".as_bytes().to_vec(),
        Traffic::Mark => "\u{0301}".as_bytes().to_vec(),
        Traffic::Newline => b"\r\n".to_vec(),
        Traffic::Cup(row, column) => format!("\x1b[{};{}H", row + 1, column + 1).into_bytes(),
        Traffic::El(mode) => format!("\x1b[{mode}K").into_bytes(),
        Traffic::Ed(mode) => format!("\x1b[{mode}J").into_bytes(),
        Traffic::Ich(count) => format!("\x1b[{count}@").into_bytes(),
        Traffic::Dch(count) => format!("\x1b[{count}P").into_bytes(),
        Traffic::Il(count) => format!("\x1b[{count}L").into_bytes(),
        Traffic::Dl(count) => format!("\x1b[{count}M").into_bytes(),
        Traffic::Ech(count) => format!("\x1b[{count}X").into_bytes(),
        Traffic::Stbm(top, bottom) => format!("\x1b[{top};{bottom}r").into_bytes(),
        Traffic::StbmReset => b"\x1b[r".to_vec(),
        Traffic::Ri => b"\x1bM".to_vec(),
        Traffic::Ind => b"\x1bD".to_vec(),
        Traffic::Sgr(0) => b"\x1b[0m".to_vec(),
        Traffic::Sgr(color) => format!("\x1b[4{color}m").into_bytes(),
        Traffic::Sc => b"\x1b7".to_vec(),
        Traffic::Rc => b"\x1b8".to_vec(),
        Traffic::Alternate(true) => b"\x1b[?1049h".to_vec(),
        Traffic::Alternate(false) => b"\x1b[?1049l".to_vec(),
        Traffic::Cr => b"\r".to_vec(),
        Traffic::Bs => b"\x08".to_vec(),
        Traffic::OriginMode(true) => b"\x1b[?6h".to_vec(),
        Traffic::OriginMode(false) => b"\x1b[?6l".to_vec(),
        Traffic::Cuu(count) => format!("\x1b[{count}A").into_bytes(),
        Traffic::Cud(count) => format!("\x1b[{count}B").into_bytes(),
    }
}

/// Every logical line in the ring, oldest first: rows joined through
/// their recorded wraps, each line's trailing blanks trimmed, and the
/// empty lines at the end dropped.
fn logical_lines(grid: &Grid) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for line in ring_lines(grid) {
        let row = grid.row(GridLine(line));
        let cells: &[Cell] = row;
        let wrap = grid.wrap_at(GridLine(line));
        let end = wrap.map_or(cells.len(), |n| usize::from(n).min(cells.len()));
        current.extend(cells[..end].iter().flat_map(Cell::chars));
        if wrap.is_none() {
            lines.push(current.trim_end().to_string());
            current.clear();
        }
    }
    if !current.is_empty() {
        lines.push(current.trim_end().to_string());
    }
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

/// The grid lines of the ring, oldest history row first.
fn ring_lines(grid: &Grid) -> Range<i32> {
    let history = i32::try_from(grid.history_len()).expect("history fits an i32");
    -history..i32::from(grid.size().rows)
}

/// Every row of the ring, oldest first, as its cells and recorded wrap.
fn ring_cells(grid: &Grid) -> Vec<(Vec<Cell>, Option<u16>)> {
    ring_lines(grid)
        .map(|line| {
            let row: &[Cell] = grid.row(GridLine(line));
            (row.to_vec(), grid.wrap_at(GridLine(line)))
        })
        .collect()
}

/// The ids of every row of the ring, oldest first.
fn ring_ids(grid: &Grid) -> Vec<LineId> {
    ring_lines(grid)
        .filter_map(|line| grid.line_id_at(GridLine(line)))
        .collect()
}

/// Checks that every row of the ring is `cols` wide with its wide pairs
/// intact, and that the history index is in step.
fn check_rows(grid: &Grid, cols: u16) -> Result<(), TestCaseError> {
    for line in ring_lines(grid) {
        let row = grid.row(GridLine(line));
        prop_assert_eq!(row.len(), usize::from(cols), "row {} width", line);
        prop_assert!(row.wide_pairs_intact(), "row {} broke a wide pair", line);
    }
    grid.assert_history_index_matches_ring();
    Ok(())
}

/// Checks the structure a reflow or a truncating resize leaves behind:
/// every row as wide as the grid with its wide pairs intact and its wrap
/// inside the row, no id twice in the ring, the history index in step,
/// the bottom row ending its line, and both cursors on the screen.
fn check_structure(vt: &OrzmaVt) -> Result<(), TestCaseError> {
    let screen = vt.device.active_screen();
    let grid = screen.grid();
    let size = grid.size();
    check_rows(grid, size.cols)?;
    for line in ring_lines(grid) {
        if let Some(cells) = grid.wrap_at(GridLine(line)) {
            prop_assert!(cells <= size.cols, "row {} wraps at {}", line, cells);
        }
    }
    let ids = ring_ids(grid);
    let unique: HashSet<LineId> = ids.iter().copied().collect();
    prop_assert_eq!(unique.len(), ids.len(), "an id repeats in the ring");
    prop_assert_eq!(
        grid.wrap_at(GridLine(i32::from(size.rows) - 1)),
        None,
        "the bottom row continues"
    );
    for (line, column, _) in screen.cursors() {
        prop_assert!(
            line.0 < size.rows,
            "a cursor on row {} of {}",
            line.0,
            size.rows
        );
        prop_assert!(
            column.0 < size.cols,
            "a cursor on column {} of {}",
            column.0,
            size.cols
        );
    }
    Ok(())
}

/// The primary screen's row ids seen so far, and those that have left its
/// ring for good.
#[derive(Default)]
struct IdLedger {
    live: HashSet<LineId>,
    gone: HashSet<LineId>,
}

impl IdLedger {
    /// Records the primary screen's ids when it is on show, failing when
    /// an id that left its ring is back.
    fn observe(&mut self, vt: &OrzmaVt) -> Result<(), TestCaseError> {
        if vt.device.modes().active_screen != ScreenKind::Primary {
            return Ok(());
        }
        let live: HashSet<LineId> = ring_ids(vt.device.active_screen().grid())
            .into_iter()
            .collect();
        self.gone.extend(self.live.difference(&live).copied());
        let back: Vec<&LineId> = live.intersection(&self.gone).collect();
        prop_assert!(back.is_empty(), "ids came back: {:?}", back);
        self.live = live;
        Ok(())
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// Asserts that any series of resizes leaves the text of every logical
    /// line, history included, as it was, with every row as wide as the
    /// grid, every wide pair intact, and the history index in step.
    ///
    /// Case: a shell prints Japanese and ASCII output across wrapped
    /// lines, then the user drags the window through several widths and
    /// heights under either scrollback policy.
    #[test]
    fn resizes_keep_every_logical_line(
        outputs in prop::collection::vec(output_strategy(), 1..120),
        sizes in prop::collection::vec((2u16..12, 1u16..6), 1..8),
        keep in any::<bool>(),
    ) {
        let policy = if keep { ScrollbackOnGrow::Keep } else { ScrollbackOnGrow::Reclaim };
        let mut vt = OrzmaVt::new(GridSize { cols: 6, rows: 3 }, 10_000)
            .with_scrollback_on_grow(policy);
        for output in &outputs {
            vt.interpret(&bytes_of(output));
        }
        let before = logical_lines(vt.device.active_screen().grid());
        for (cols, rows) in &sizes {
            let _ = vt.resize(GridSize { cols: *cols, rows: *rows });
            let grid = vt.device.active_screen().grid();
            prop_assert_eq!(logical_lines(grid), before.clone(), "after {}x{}", cols, rows);
            check_rows(grid, *cols)?;
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        max_global_rejects: 1 << 16,
        ..ProptestConfig::with_cases(256)
    })]

    /// Asserts that under `Reclaim` narrowing the window and widening it
    /// back restores every cell and recorded wrap of every row, history
    /// included, and the cursor, unless the prompt fills its row and parks
    /// the cursor on the right edge.
    ///
    /// Case: a shell prints Japanese and ASCII output with combining marks
    /// across wrapped lines and shows its prompt, then the user narrows the
    /// window and widens it back on macOS.
    #[test]
    fn narrowing_and_widening_back_restores_every_cell_under_reclaim(
        outputs in prop::collection::vec(output_strategy(), 0..120),
        start in (2u16..12, 1u16..7),
        narrow in 2u16..12,
    ) {
        let size = GridSize { cols: start.0, rows: start.1 };
        let mut vt = OrzmaVt::new(size, 10_000);
        for output in &outputs {
            vt.interpret(&bytes_of(output));
        }
        vt.interpret(b"\r\nPS>");
        let screen = vt.device.active_screen();
        let before = (ring_cells(screen.grid()), screen.cursors()[0]);
        let (_, _, parked) = before.1;
        prop_assume!(!parked);
        let _ = vt.resize(GridSize { cols: narrow.min(start.0), rows: start.1 });
        let _ = vt.resize(size);
        let screen = vt.device.active_screen();
        let after = (ring_cells(screen.grid()), screen.cursors()[0]);
        prop_assert_eq!(after, before);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Asserts that after every resize, whatever the history cap and the
    /// policy, each row is as wide as the grid with its wide pairs intact,
    /// no row id repeats or comes back once gone, the history index stays
    /// in step, the bottom row ends its line, and the cursor and the saved
    /// cursor sit on the screen.
    ///
    /// Case: shells and full-screen programs print, move the cursor, erase,
    /// edit, set margins, scroll, save the cursor, and flip screens between
    /// the user's resizes, on terminals whose scrollback ranges from none to
    /// ten thousand rows.
    #[test]
    fn every_resize_keeps_ids_fresh_and_the_cursors_on_the_screen(
        cap in prop_oneof![
            Just(0usize), Just(1usize), Just(2usize), Just(3usize), Just(5usize), Just(10_000usize)
        ],
        keep in any::<bool>(),
        start in (2u16..10, 1u16..6),
        rounds in prop::collection::vec(
            (prop::collection::vec(traffic_strategy(), 0..60), (2u16..12, 1u16..7)),
            1..6,
        ),
    ) {
        let policy = if keep { ScrollbackOnGrow::Keep } else { ScrollbackOnGrow::Reclaim };
        let mut vt = OrzmaVt::new(GridSize { cols: start.0, rows: start.1 }, cap)
            .with_scrollback_on_grow(policy);
        let mut ledger = IdLedger::default();
        for (traffic, (cols, rows)) in &rounds {
            for piece in traffic {
                vt.interpret(&bytes_of(piece));
            }
            ledger.observe(&vt)?;
            let _ = vt.resize(GridSize { cols: *cols, rows: *rows });
            ledger.observe(&vt)?;
            check_structure(&vt)?;
        }
        vt.interpret(b"\x1b[?1049l");
        ledger.observe(&vt)?;
        check_structure(&vt)?;
    }
}
