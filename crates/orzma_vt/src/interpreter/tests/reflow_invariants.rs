//! Property test: a resize never changes the text of any logical line,
//! and every rewrapped row keeps the grid's structural invariants.

use super::*;
use crate::screen::grid::Grid;
use crate::screen::grid::reflow::ScrollbackOnGrow;
use proptest::prelude::*;

/// One piece of output.
#[derive(Debug, Clone)]
enum Output {
    Ascii(u8),
    Cjk,
    Mark,
    Newline,
}

fn output_strategy() -> impl Strategy<Value = Output> {
    prop_oneof![
        6 => (b'a'..=b'z').prop_map(Output::Ascii),
        2 => Just(Output::Cjk),
        1 => Just(Output::Mark),
        1 => Just(Output::Newline),
    ]
}

fn emit(vt: &mut OrzmaVt, output: &Output) {
    match output {
        Output::Ascii(byte) => {
            vt.interpret(&[*byte]);
        }
        Output::Cjk => {
            vt.interpret("あ".as_bytes());
        }
        Output::Mark => {
            vt.interpret("\u{0301}".as_bytes());
        }
        Output::Newline => {
            vt.interpret(b"\r\n");
        }
    }
}

/// Every logical line in the ring, oldest first: rows joined through
/// their recorded wraps, each line's trailing blanks trimmed, and the
/// empty lines at the end dropped.
fn logical_lines(grid: &Grid) -> Vec<String> {
    let history = i32::try_from(grid.history_len()).expect("history fits an i32");
    let mut lines = Vec::new();
    let mut current = String::new();
    for line in -history..i32::from(grid.size().rows) {
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
            emit(&mut vt, output);
        }
        let before = logical_lines(vt.device.active_screen().grid());
        for (cols, rows) in &sizes {
            let _ = vt.resize(GridSize { cols: *cols, rows: *rows });
            let grid = vt.device.active_screen().grid();
            prop_assert_eq!(logical_lines(grid), before.clone(), "after {}x{}", cols, rows);
            let history = i32::try_from(grid.history_len()).expect("history fits an i32");
            for line in -history..i32::from(grid.size().rows) {
                let row = grid.row(GridLine(line));
                prop_assert_eq!(row.len(), usize::from(*cols));
                prop_assert!(row.wide_pairs_intact(), "row {} broke", line);
            }
            grid.assert_history_index_matches_ring();
        }
    }
}
