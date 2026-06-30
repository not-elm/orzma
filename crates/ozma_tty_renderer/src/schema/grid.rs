use crate::schema::{CURSOR_VISIBLE_BIT, Cursor, CursorShape, HyperlinkId, HyperlinkUri, ViCursor};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// A structure represents the layout structure of the terminal grid.
/// Each terminal entity owns this component.
#[derive(Component, Default)]
pub struct TerminalGrid {
    /// Visible column count.
    pub cols: u16,
    /// Visible row count.
    pub rows: u16,
    /// Cell grid indexed `[row][col_grapheme_index]`.
    pub cells: Vec<Vec<Cell>>,
    /// Current cursor state, absent until the first frame arrives.
    pub cursor: Option<Cursor>,
    /// Lines scrolled back from the live tail; 0 = at live tail.
    pub display_offset: u32,
    /// Total scrollback history line count.
    pub history_size: u32,
    /// Cumulative trimmed-lines counter mirrored from the latest frame.
    pub history_base: u64,
    /// Monotonic sequence number of the last applied frame.
    pub last_seq: u32,
    /// Active terminal modes from the last snapshot (e.g. "mouse-sgr-1006").
    pub modes: Vec<String>,
    /// Vi-mode cursor when the server is in copy mode; `None` otherwise.
    /// `CopyModePlugin` reads this every frame to drive `CopyModeState::active`.
    pub vi_cursor: Option<ViCursor>,
    /// Active selection range emitted alongside `vi_cursor`. Independent of
    /// `vi_cursor` — survives motion without selection.
    pub selection: Option<SelectionRange>,
    /// App-level cursor visibility override. When `true`,
    /// `current_cursor_pos_and_style()` clears [`CURSOR_VISIBLE_BIT`]
    /// before returning. Independent of `Cursor.visible` (which mirrors
    /// DECTCEM from the wire) — this field is for the UI layer (e.g.,
    /// IME composition) to non-destructively hide the cursor without
    /// clobbering terminal-controlled state.
    pub suppress_cursor: bool,
    /// OSC 8 hyperlinks indexed by wire id. Populated cumulatively from
    /// `FrameSnapshot.hyperlinks` / `FrameDelta.hyperlinks`. Replaced on
    /// snapshot, merged on delta. Linear scan — realistic sessions
    /// carry ≤100 distinct hyperlinks (mirroring the server-side
    /// interner rationale).
    pub hyperlinks: Vec<(HyperlinkId, HyperlinkUri)>,
    /// Terminal default background color from `FrameSnapshot.default_bg`
    /// (sourced from OSC 11). Raw `[r, g, b]` bytes; black when not set. The
    /// material uses it as the base background for default-bg cells and the
    /// padding outside the grid; an unset `[0,0,0]` is mapped to
    /// `TerminalPaddingFallback` (the theme background) by the material system.
    pub default_bg: [u8; 3],
}

impl TerminalGrid {
    /// Resolves `(row, col)` to the hyperlink at that visible cell, if
    /// any. `col` is a column coordinate, not a grapheme index — wide
    /// cells (width=2) match both of their columns, and width-0
    /// trailers are skipped without consuming a column. Returns
    /// `None` for out-of-bounds, unlinked cells, or when the id is
    /// present on the cell but absent from the map.
    //
    // NOTE: `self.cells[row]` is grapheme-indexed (one entry per
    //       cluster from `runs_to_cells`), so a column-to-cell walk
    //       is required — direct `cells[row][col]` indexing would
    //       desynchronize after any wide char or width-0 trailer.
    //       Must mirror the column-advance logic in
    //       `material::rebuild_cells`.
    pub fn hyperlink_at(&self, row: u16, col: u16) -> Option<(HyperlinkId, &HyperlinkUri)> {
        let row_cells = self.cells.get(row as usize)?;
        let mut current_col: u32 = 0;
        let target = u32::from(col);
        for cell in row_cells {
            if cell.width == 0 {
                continue;
            }
            let cell_end = current_col.saturating_add(u32::from(cell.width));
            if target >= current_col && target < cell_end {
                let id = cell.hyperlink_id?;
                return self
                    .hyperlinks
                    .iter()
                    .find(|(stored_id, _)| *stored_id == id)
                    .map(|(stored_id, uri)| (*stored_id, uri));
            }
            current_col = cell_end;
        }
        None
    }

    pub fn current_cursor_pos_and_style(&self) -> (UVec2, u32) {
        let mut cursor_pos = UVec2::ZERO;
        let mut cursor_style = 0;
        if let Some(vc) = self.vi_cursor {
            if !vc.in_scrollback && vc.row >= 0 {
                cursor_pos = UVec2::new(u32::from(vc.column), vc.row as u32);
                cursor_style = Cursor {
                    x: vc.column,
                    y: vc.row.max(0) as u16,
                    shape: CursorShape::Block,
                    blinking: false,
                    visible: true,
                }
                .pack_cursor_style();
            }
        } else if let Some(c) = self.cursor.as_ref() {
            cursor_pos = UVec2::new(u32::from(c.x), u32::from(c.y));
            cursor_style = c.pack_cursor_style();
        }
        if self.suppress_cursor {
            cursor_style &= !CURSOR_VISIBLE_BIT;
        }
        (cursor_pos, cursor_style)
    }
}

/// One terminal cell after wire decoding + grapheme expansion.
///
/// `width = 0` for combining marks / zero-width trailers (these are
/// folded into the previous cell's `text` so the caller never paints them
/// separately; they appear in `cells` only because the wire keeps them
/// addressable for selection / cursor logic).
#[derive(Clone, Debug)]
pub struct Cell {
    /// The grapheme cluster text for this cell.
    pub text: String,
    /// Display width: 2 for wide CJK, 0 for combining marks, 1 otherwise.
    pub width: u8,
    /// Foreground color.
    pub fg: Color,
    /// Background color.
    pub bg: Color,
    /// Style bitmask (see `ozmux_terminal_protocol::style`).
    pub style: u16,
    /// OSC 8 hyperlink wire id, if any.
    pub hyperlink_id: Option<HyperlinkId>,
}

impl Cell {
    /// Whether this cell paints no glyph: a zero-width cell (combining mark /
    /// wide-char spacer) or one whose text is empty or all whitespace.
    ///
    /// Shared by the renderer's glyph resolution and the host paint-rescue's
    /// blank-grid test so the two notions of "renders nothing" cannot drift.
    #[inline]
    pub fn is_blank(&self) -> bool {
        self.width == 0 || self.text.trim().is_empty()
    }
}

/// A row of runs ordered left-to-right.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Row {
    /// Runs in left-to-right column order.
    pub runs: Vec<Run>,
}

/// A run of cells sharing identical fg/bg/style attributes.
///
/// Wide-char spacers (alacritty internal) are absorbed server-side and do
/// not appear in `text`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Run {
    /// Total column span (sum of grapheme cluster widths in `text`).
    pub cols: u16,
    /// Foreground color.
    pub fg: Color,
    /// Background color.
    pub bg: Color,
    /// Style bitmask (see the `style` module). Widened from u8 to u16 so
    /// HIDDEN (bit 6) and future underline variants fit. Wire-compatible:
    /// rmp-serde picks the smallest msgpack int form per value, so masks
    /// ≤ 127 still serialize to one byte.
    pub style: u16,
    /// UTF-8 text; the client uses Unicode East Asian Width to position each
    /// grapheme cluster within the run.
    pub text: String,
    /// Hyperlink id (OSC 8); always `None` until Phase 3.
    pub hyperlink_id: Option<HyperlinkId>,
}

/// Active selection range in viewport coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectionRange {
    /// Anchor point where the selection started.
    pub start: ViewportPoint,
    /// Cursor point where the selection currently ends.
    pub end: ViewportPoint,
    /// Selection geometry (char-wise or line-wise).
    pub kind: SelectionKind,
}
/// A point in viewport coordinates (server-converted from alacritty `Line(i32)`).
///
/// Endpoints that resolve to scrollback are clamped to `row = -1` (above
/// viewport) or `row = rows` (below viewport) so the client can still draw
/// the portion of the selection that intersects the visible grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewportPoint {
    /// Viewport row; clamped to `-1` (above) or `rows` (below) for scrollback endpoints.
    pub row: i16,
    /// Viewport column (0-based).
    pub column: u16,
}

/// Selection geometry. `Block` (rectangle) and `Semantic` are non-goals
/// for v1 — see the design spec's Non-goals section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SelectionKind {
    /// Character-wise selection.
    Char,
    /// Line-wise selection.
    Line,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{Cursor, CursorShape};

    fn visible_block_cursor() -> Cursor {
        Cursor {
            x: 3,
            y: 5,
            shape: CursorShape::Block,
            blinking: false,
            visible: true,
        }
    }

    #[test]
    fn current_cursor_pos_and_style_returns_packed_style_when_not_suppressed() {
        let grid = TerminalGrid {
            cursor: Some(visible_block_cursor()),
            suppress_cursor: false,
            ..Default::default()
        };
        let (pos, style) = grid.current_cursor_pos_and_style();
        assert_eq!(pos, UVec2::new(3, 5));
        assert_eq!(style & CURSOR_VISIBLE_BIT, CURSOR_VISIBLE_BIT);
    }

    #[test]
    fn current_cursor_pos_and_style_clears_visible_bit_when_suppressed() {
        let grid = TerminalGrid {
            cursor: Some(visible_block_cursor()),
            suppress_cursor: true,
            ..Default::default()
        };
        let (_pos, style) = grid.current_cursor_pos_and_style();
        assert_eq!(style & CURSOR_VISIBLE_BIT, 0);
    }

    #[test]
    fn suppress_cursor_does_not_affect_vi_cursor_position() {
        let grid = TerminalGrid {
            vi_cursor: Some(ViCursor {
                row: 2,
                column: 7,
                in_scrollback: false,
            }),
            suppress_cursor: true,
            ..Default::default()
        };
        let (pos, style) = grid.current_cursor_pos_and_style();
        assert_eq!(pos, UVec2::new(7, 2));
        assert_eq!(style & CURSOR_VISIBLE_BIT, 0);
    }

    #[test]
    fn hyperlink_at_returns_none_when_out_of_bounds() {
        let grid = TerminalGrid {
            cols: 4,
            rows: 2,
            cells: vec![vec![], vec![]],
            ..Default::default()
        };
        assert!(grid.hyperlink_at(99, 0).is_none());
        assert!(grid.hyperlink_at(0, 99).is_none());
    }

    #[test]
    fn hyperlink_at_returns_none_for_width_zero_trailer() {
        let cell = Cell {
            text: "\u{0301}".to_string(),
            width: 0,
            fg: Color::WHITE,
            bg: Color::BLACK,
            style: 0,
            hyperlink_id: Some(HyperlinkId(5)),
        };
        let grid = TerminalGrid {
            cols: 4,
            rows: 1,
            cells: vec![vec![cell]],
            hyperlinks: vec![(HyperlinkId(5), HyperlinkUri::new("https://example"))],
            ..Default::default()
        };
        assert!(grid.hyperlink_at(0, 0).is_none());
    }

    #[test]
    fn hyperlink_at_returns_none_when_map_missing_id() {
        let cell = Cell {
            text: "x".to_string(),
            width: 1,
            fg: Color::WHITE,
            bg: Color::BLACK,
            style: 0,
            hyperlink_id: Some(HyperlinkId(7)),
        };
        let grid = TerminalGrid {
            cols: 4,
            rows: 1,
            cells: vec![vec![cell]],
            hyperlinks: vec![],
            ..Default::default()
        };
        assert!(grid.hyperlink_at(0, 0).is_none());
    }

    #[test]
    fn hyperlink_at_returns_id_and_uri_for_linked_cell() {
        let cell = Cell {
            text: "x".to_string(),
            width: 1,
            fg: Color::WHITE,
            bg: Color::BLACK,
            style: 0,
            hyperlink_id: Some(HyperlinkId(7)),
        };
        let grid = TerminalGrid {
            cols: 4,
            rows: 1,
            cells: vec![vec![cell]],
            hyperlinks: vec![(HyperlinkId(7), HyperlinkUri::new("https://example"))],
            ..Default::default()
        };
        let (id, uri) = grid.hyperlink_at(0, 0).expect("hyperlink present");
        assert_eq!(id, HyperlinkId(7));
        assert_eq!(uri.as_str(), "https://example");
    }

    #[test]
    fn hyperlink_at_returns_none_for_unlinked_cell() {
        let cell = Cell {
            text: "x".to_string(),
            width: 1,
            fg: Color::WHITE,
            bg: Color::BLACK,
            style: 0,
            hyperlink_id: None,
        };
        let grid = TerminalGrid {
            cols: 4,
            rows: 1,
            cells: vec![vec![cell]],
            hyperlinks: vec![(HyperlinkId(7), HyperlinkUri::new("https://example"))],
            ..Default::default()
        };
        assert!(grid.hyperlink_at(0, 0).is_none());
    }

    #[test]
    fn hyperlink_at_resolves_both_halves_of_wide_char() {
        // Wide CJK grapheme occupies 2 columns but only 1 cell entry.
        // Both halves must resolve to the same hyperlink.
        let wide_linked = Cell {
            text: "あ".to_string(),
            width: 2,
            fg: Color::WHITE,
            bg: Color::BLACK,
            style: 0,
            hyperlink_id: Some(HyperlinkId(7)),
        };
        let trailing = Cell {
            text: "b".to_string(),
            width: 1,
            fg: Color::WHITE,
            bg: Color::BLACK,
            style: 0,
            hyperlink_id: None,
        };
        let grid = TerminalGrid {
            cols: 3,
            rows: 1,
            cells: vec![vec![wide_linked, trailing]],
            hyperlinks: vec![(HyperlinkId(7), HyperlinkUri::new("https://example"))],
            ..Default::default()
        };
        // Column 0: left half of the wide char → linked.
        let (id, uri) = grid.hyperlink_at(0, 0).expect("left half should resolve");
        assert_eq!(id, HyperlinkId(7));
        assert_eq!(uri.as_str(), "https://example");
        // Column 1: right half of the wide char → SAME link.
        let (id, uri) = grid.hyperlink_at(0, 1).expect("right half should resolve");
        assert_eq!(id, HyperlinkId(7));
        assert_eq!(uri.as_str(), "https://example");
        // Column 2: trailing ASCII char → unlinked.
        assert!(grid.hyperlink_at(0, 2).is_none());
    }

    #[test]
    fn hyperlink_at_skips_width_zero_trailer_in_column_walk() {
        // Width-0 trailer (e.g., a combining mark emitted as its own
        // wire cell) consumes a grapheme slot but no columns. The
        // following cell must still be reachable at its column.
        let base = Cell {
            text: "a".to_string(),
            width: 1,
            fg: Color::WHITE,
            bg: Color::BLACK,
            style: 0,
            hyperlink_id: None,
        };
        let trailer = Cell {
            text: "\u{0301}".to_string(),
            width: 0,
            fg: Color::WHITE,
            bg: Color::BLACK,
            style: 0,
            hyperlink_id: None,
        };
        let linked = Cell {
            text: "x".to_string(),
            width: 1,
            fg: Color::WHITE,
            bg: Color::BLACK,
            style: 0,
            hyperlink_id: Some(HyperlinkId(9)),
        };
        let grid = TerminalGrid {
            cols: 2,
            rows: 1,
            cells: vec![vec![base, trailer, linked]],
            hyperlinks: vec![(HyperlinkId(9), HyperlinkUri::new("https://x"))],
            ..Default::default()
        };
        // Column 0: 'a' (unlinked).
        assert!(grid.hyperlink_at(0, 0).is_none());
        // Column 1: the linked cell — the width-0 trailer must not
        // displace its column.
        let (id, _uri) = grid.hyperlink_at(0, 1).expect("linked cell at col 1");
        assert_eq!(id, HyperlinkId(9));
    }
}
