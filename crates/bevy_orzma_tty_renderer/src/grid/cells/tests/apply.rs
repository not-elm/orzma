//! Tests for applying frames to the cells, and for detecting when a frame
//! would change them.

use super::*;
use crate::error::RendererError;
use crate::grid::test_support::{dirty_row, quiet_frame};
use orzma_vt::prelude::{
    Color, DirtyRow, GridSize, Hyperlink, Rgb, Row, RunError, ViewportLine, VtError,
};

fn one_row_frame(cols: u16, run: Run) -> Frame {
    Frame {
        size: GridSize { cols, rows: 1 },
        rows: vec![DirtyRow {
            line: ViewportLine(0),
            contents: Row::from(vec![run]),
        }],
        ..quiet_frame()
    }
}

/// Asserts that re-applying a row refills its row buffer in place
/// rather than allocating a new one.
///
/// Case: a status line redraws the same column every frame, first
/// with a heavily accented letter and then with a plain one.
#[test]
fn a_reapplied_row_keeps_its_row_buffer() {
    let mut cells = TerminalCells::default();
    let accented = run_with_widths("a\u{0301}\u{0301}\u{0301}\u{0301}", &[1, 0, 0, 0, 0]);
    cells
        .apply(&one_row_frame(1, accented))
        .expect("a well-formed frame applies");
    let row_ptr = cells.cells[0].as_ptr();

    cells
        .apply(&one_row_frame(1, run_with_widths("b", &[1])))
        .expect("a well-formed frame applies");

    assert_eq!(text_of(&cells.cells[0][0]), "b");
    assert_eq!(cells.cells[0].as_ptr(), row_ptr);
}

/// Asserts that a carried row replaces that row's cells when
/// applied, while a row the frame does not touch is leveled to the
/// full row width instead of keeping its original length.
///
/// Case: a build prints one line of a two-row pane.
#[test]
fn a_carried_row_replaces_its_cells_and_leaves_other_rows_full_width() {
    let mut cells = TerminalCells {
        cells: vec![vec![], vec![]],
        ..Default::default()
    };
    let frame = Frame {
        size: GridSize { cols: 2, rows: 2 },
        rows: vec![dirty_row(1, "x")],
        ..quiet_frame()
    };
    assert!(cells.differs_from(&frame));
    cells.apply(&frame).expect("a valid frame");
    assert_eq!(text_of(&cells.cells[1][0]), "x");
    assert_eq!(cells.cells[0], vec![Cell::default(); 2]);
}

/// Asserts that cells built at the frame's size but without cell
/// rows still differ, and that applying the frame gives every row
/// its cells and fills the carried ones.
///
/// Case: the host pre-sizes the grid to the PTY geometry before the
/// VT's bootstrap repaint arrives at that same size.
#[test]
fn a_pre_sized_grid_without_cells_takes_the_frame_rows() {
    let mut cells = TerminalCells {
        cells: vec![vec![], vec![]],
        ..Default::default()
    };
    let frame = Frame {
        size: GridSize { cols: 2, rows: 2 },
        rows: vec![dirty_row(0, "a"), dirty_row(1, "b")],
        ..quiet_frame()
    };
    assert!(cells.differs_from(&frame));
    cells.apply(&frame).expect("a valid frame");
    assert_eq!(cells.cells.len(), 2);
    assert_eq!(text_of(&cells.cells[1][0]), "b");
    assert!(!cells.differs_from(&Frame {
        size: GridSize { cols: 2, rows: 2 },
        ..quiet_frame()
    }));
}

/// Asserts that a frame with fewer rows than the cells truncates the
/// cell rows to the new height.
///
/// Case: the user drags the window shorter and the VT's repaint at
/// the new height arrives.
#[test]
fn a_shrinking_size_truncates_the_cells() {
    let mut cells = TerminalCells {
        cells: vec![vec![], vec![], vec![]],
        ..Default::default()
    };
    let frame = Frame {
        rows: vec![dirty_row(0, "a")],
        ..quiet_frame()
    };
    assert!(cells.differs_from(&frame));
    cells.apply(&frame).expect("a valid frame");
    assert_eq!(cells.cells.len(), 1);
    assert_eq!(text_of(&cells.cells[0][0]), "a");
}

/// Asserts that a frame changing only the column count is a
/// difference and repaints the rows it carries at the new width.
///
/// Case: the user drags the window wider without changing its
/// height, and the VT's repaint at the new width arrives.
#[test]
fn a_cols_only_size_change_differs_and_repaints() {
    let mut cells = TerminalCells::settled();
    let frame = Frame {
        size: GridSize { cols: 3, rows: 1 },
        rows: vec![dirty_row(0, "abc")],
        ..quiet_frame()
    };
    assert!(cells.differs_from(&frame));
    cells.apply(&frame).expect("a valid frame");
    assert_eq!(cells.cells[0].len(), 3);
    assert!(!cells.differs_from(&Frame {
        size: GridSize { cols: 3, rows: 1 },
        ..quiet_frame()
    }));
}

/// Asserts that a row beyond the frame's own size is ignored and is
/// not a difference.
///
/// Case: a malformed frame names a row past its own last row.
#[test]
fn a_row_out_of_range_is_ignored() {
    let mut cells = TerminalCells::settled();
    let frame = Frame {
        rows: vec![dirty_row(5, "x")],
        ..quiet_frame()
    };
    assert!(!cells.differs_from(&frame));
    cells.apply(&frame).expect("a valid frame");
    assert_eq!(cells.cells.len(), 1);
}

/// Asserts that a size change resizes the cell rows before the
/// frame's rows are applied.
///
/// Case: the user drags the window taller and the VT's next frame
/// carries every row of the new size.
#[test]
fn a_size_change_resizes_the_cells() {
    let mut cells = TerminalCells::settled();
    let frame = Frame {
        size: GridSize { cols: 3, rows: 2 },
        rows: vec![dirty_row(0, "a"), dirty_row(1, "b")],
        ..quiet_frame()
    };
    assert!(cells.differs_from(&frame));
    cells.apply(&frame).expect("a valid frame");
    assert_eq!(cells.cells.len(), 2);
    assert_eq!(text_of(&cells.cells[1][0]), "b");
}

/// Asserts that a column-count change levels every retained row to
/// the new width, including the rows the frame does not carry.
///
/// Case: the user drags the window wider and the repaint that
/// follows names only the row the cursor sits on.
#[test]
fn a_cols_change_levels_every_row_not_only_the_carried_ones() {
    let mut cells = TerminalCells::default();
    cells
        .apply(&Frame {
            size: GridSize { cols: 2, rows: 2 },
            rows: vec![dirty_row(0, "ab"), dirty_row(1, "cd")],
            ..quiet_frame()
        })
        .expect("a valid frame");
    cells
        .apply(&Frame {
            size: GridSize { cols: 5, rows: 2 },
            rows: vec![dirty_row(0, "abcde")],
            ..quiet_frame()
        })
        .expect("a valid frame");
    assert_eq!(cells.cells.len(), 2);
    assert!(
        cells.cells.iter().all(|row| row.len() == 5),
        "every row is as wide as the grid"
    );
}

/// Asserts that a palette replaces the mirror and `None` keeps it.
///
/// Case: one frame recolors the background once, and every later
/// frame carries no palette.
#[test]
fn a_palette_replaces_the_mirror_and_none_keeps_it() {
    let mut cells = TerminalCells::settled();
    let palette = Palette {
        background: Rgb { r: 9, g: 8, b: 7 },
        ..Palette::default()
    };
    let recolored = Frame {
        palette: Some(palette),
        ..quiet_frame()
    };
    assert!(cells.differs_from(&recolored));
    cells.apply(&recolored).expect("a valid frame");
    assert_eq!(cells.palette.background, Rgb { r: 9, g: 8, b: 7 });
    assert!(!cells.differs_from(&quiet_frame()));
}

/// Asserts that hyperlinks merge without overwriting a known id,
/// and that a known id alone is not a difference.
///
/// Case: a later frame re-sends a definition the mirror already
/// holds alongside a genuinely new one.
#[test]
fn hyperlinks_merge_without_overwrite() {
    let mut cells = TerminalCells {
        hyperlinks: HashMap::from([(id(1), HyperlinkUri::new("https://old"))]),
        ..TerminalCells::settled()
    };
    let repeated = Frame {
        hyperlinks: vec![Hyperlink {
            id: id(1),
            uri: HyperlinkUri::new("https://CHANGED"),
        }],
        ..quiet_frame()
    };
    assert!(!cells.differs_from(&repeated));

    let extended = Frame {
        hyperlinks: vec![
            Hyperlink {
                id: id(1),
                uri: HyperlinkUri::new("https://CHANGED"),
            },
            Hyperlink {
                id: id(2),
                uri: HyperlinkUri::new("https://new"),
            },
        ],
        ..quiet_frame()
    };
    assert!(cells.differs_from(&extended));
    cells.apply(&extended).expect("a valid frame");
    assert_eq!(cells.hyperlinks.len(), 2);
    assert_eq!(cells.hyperlinks[&id(1)].as_str(), "https://old");
    assert_eq!(cells.hyperlinks[&id(2)].as_str(), "https://new");
}

/// Asserts that a row resolves a hyperlink id defined by an earlier
/// frame's table.
///
/// Case: a link's definition arrived in one frame and the row that
/// references it is repainted in a later one.
#[test]
fn a_row_resolves_a_hyperlink_from_an_earlier_frame() {
    let mut cells = TerminalCells::settled();
    cells
        .apply(&Frame {
            hyperlinks: vec![Hyperlink {
                id: id(4),
                uri: HyperlinkUri::new("https://earlier"),
            }],
            ..quiet_frame()
        })
        .expect("a valid frame");
    cells
        .apply(&Frame {
            rows: vec![DirtyRow {
                line: ViewportLine(0),
                contents: Row::from(vec![run_with_link("a", Some(id(4)))]),
            }],
            ..quiet_frame()
        })
        .expect("a valid frame");
    assert_eq!(
        cells.hyperlink_at(0, 0).map(|(_, uri)| uri.as_str()),
        Some("https://earlier")
    );
}

/// Asserts that applying the runs a stored row emits reproduces that
/// row's cells exactly.
///
/// Case: the VT repaints a row holding an accented letter, a CJK
/// character inside an OSC 8 link, and a colored letter.
#[test]
fn a_row_survives_the_round_trip_through_its_runs() {
    let mut accented = linked_cell('e', None);
    assert!(accented.push_mark('\u{0301}'));
    let wide = Cell {
        width: CellWidth::Wide,
        ..linked_cell('あ', Some(7))
    };
    let colored = Cell {
        fg: Color::Indexed(1),
        ..linked_cell('z', None)
    };
    let stored = vec![accented, wide.clone(), wide.continuation(), colored];
    let mut cells = TerminalCells::default();
    cells
        .apply(&Frame {
            size: GridSize { cols: 4, rows: 1 },
            rows: vec![DirtyRow {
                line: ViewportLine(0),
                contents: Row::from(stored.clone()).to_runs(),
            }],
            hyperlinks: vec![Hyperlink {
                id: id(7),
                uri: HyperlinkUri::new("https://example"),
            }],
            ..quiet_frame()
        })
        .expect("a stored row's runs apply");
    assert_eq!(cells.cells[0], stored);
}

/// Asserts that a size change alone is a difference for the cells,
/// and that applying the frame resizes the cell rows to match even
/// though the frame carries no rows of its own.
///
/// Case: a frame that only changes the size reaches settled cells.
#[test]
fn a_new_size_alone_resizes_the_cell_rows() {
    let mut cells = TerminalCells::settled();
    let frame = Frame {
        size: GridSize { cols: 3, rows: 2 },
        ..quiet_frame()
    };
    assert!(cells.differs_from(&frame));
    cells.apply(&frame).expect("a valid frame");
    assert_eq!(cells.cells.len(), 2);
    assert!(!cells.differs_from(&frame));
}

/// Asserts that a frame the cells report no difference for leaves
/// the cells, hyperlinks and palette untouched.
///
/// Case: a frame repeats the content state the cells already hold,
/// with a linked cell, its hyperlink table entry, and a
/// non-default palette already in place.
#[test]
fn a_cells_that_reports_no_difference_is_not_mutated_by_apply() {
    let linked = linked_cell('x', Some(7));
    let mut cells = TerminalCells {
        cells: vec![vec![linked]],
        hyperlinks: HashMap::from([(id(7), HyperlinkUri::new("https://example"))]),
        palette: Palette {
            background: Rgb { r: 9, g: 8, b: 7 },
            ..Palette::default()
        },
    };
    let frame = quiet_frame();
    assert!(!cells.differs_from(&frame));
    let before = (
        cells.cells.clone(),
        cells.hyperlinks.clone(),
        cells.palette.clone(),
    );
    cells.apply(&frame).expect("a valid frame");
    assert_eq!(
        (
            cells.cells.clone(),
            cells.hyperlinks.clone(),
            cells.palette.clone(),
        ),
        before
    );
}

/// Asserts that a frame carrying a run whose widths fail
/// [`Run::check`] is rejected with that error and leaves the cells
/// untouched, hyperlink table included.
///
/// Case: a producer emits a run with a width of three alongside a
/// new hyperlink definition.
#[test]
fn a_frame_with_a_malformed_run_is_rejected_and_leaves_the_cells_untouched() {
    let mut cells = TerminalCells::settled();
    let frame = Frame {
        rows: vec![DirtyRow {
            line: ViewportLine(0),
            contents: Row::from(vec![run_with_widths("\u{3042}", &[3])]),
        }],
        hyperlinks: vec![Hyperlink {
            id: id(4),
            uri: HyperlinkUri::new("https://rejected"),
        }],
        ..quiet_frame()
    };
    assert!(cells.differs_from(&frame));
    assert!(matches!(
        cells.apply(&frame),
        Err(RendererError::Vt(VtError::Run(RunError::InvalidWidth)))
    ));
    assert_eq!(cells.cells, vec![vec![Cell::default()]]);
    assert!(cells.hyperlinks.is_empty());
}
