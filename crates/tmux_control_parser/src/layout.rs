//! Parser for tmux window-layout strings (e.g. `b25f,80x24,0,0,2`) into a
//! typed recursive cell tree, mirroring tmux's `layout-custom.c` grammar.

use crate::error::{LayoutError, TmuxError, TmuxResult};

/// Orientation of a split cell, named after tmux's `{`/`[`/`<` delimiters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SplitDir {
    /// `{ ... }` — children side-by-side, left to right.
    LeftRight,
    /// `[ ... ]` — children stacked, top to bottom.
    TopBottom,
    /// `< ... >` — floating/popup group (newer tmux).
    Floating,
}

/// A cell's geometry within the window grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellDims {
    /// Cell width in character cells (tmux `%u`).
    pub width: u32,
    /// Cell height in character cells (tmux `%u`).
    pub height: u32,
    /// Horizontal offset from the window origin (tmux `%d`, may be negative).
    pub xoff: i32,
    /// Vertical offset from the window origin (tmux `%d`, may be negative).
    pub yoff: i32,
}

/// One node in the layout tree: a leaf pane or a split of child cells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cell {
    /// A pane. `pane_id` is `None` when the grammar's `x`-lookahead proved a
    /// trailing comma was a sibling separator rather than a pane id.
    Leaf {
        /// Geometry of this pane.
        dims: CellDims,
        /// The tmux pane id, when present in the layout string.
        pane_id: Option<u32>,
    },
    /// A container split into child cells along one axis (or a floating group).
    Split {
        /// Geometry of the container.
        dims: CellDims,
        /// Whether children run left-right (`{}`), top-bottom (`[]`), or float (`<>`).
        dir: SplitDir,
        /// The child cells, in layout order.
        children: Vec<Cell>,
    },
}

impl Cell {
    /// Returns the geometry of this cell, regardless of leaf/split.
    pub fn dims(&self) -> CellDims {
        match self {
            Cell::Leaf { dims, .. } | Cell::Split { dims, .. } => *dims,
        }
    }
}

/// A parsed window-layout string: the leading checksum plus the root cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowLayout {
    /// The 16-bit checksum tmux prefixed (4 lowercase hex digits). Stored
    /// verbatim; parsing never fails on a mismatch.
    pub checksum: u16,
    /// The root cell of the layout tree.
    pub root: Cell,
}

impl WindowLayout {
    /// Parses a `CHECKSUM,CELL` window-layout string into a typed tree.
    pub fn parse(input: &[u8]) -> TmuxResult<Self> {
        let comma = input
            .iter()
            .position(|&b| b == b',')
            .ok_or(TmuxError::InvalidLayout {
                reason: LayoutError::MissingChecksumComma,
            })?;
        let checksum = parse_checksum(&input[..comma])?;
        let body = &input[comma + 1..];
        let mut cur = Cursor::new(body);
        let root = parse_cell(&mut cur)?;
        if !cur.at_end() {
            return Err(TmuxError::InvalidLayout {
                reason: LayoutError::TrailingData,
            });
        }
        Ok(WindowLayout { checksum, root })
    }

    /// Recomputes tmux's 16-bit rolling checksum over a layout body (the bytes
    /// after the first comma of the original string).
    pub fn recompute_checksum(body: &[u8]) -> u16 {
        let mut csum: u16 = 0;
        for &b in body {
            csum = (csum >> 1).wrapping_add((csum & 1) << 15);
            csum = csum.wrapping_add(b as u16);
        }
        csum
    }

    /// Returns `true` if the stored checksum matches a recomputation over `body`.
    pub fn verify(&self, body: &[u8]) -> bool {
        self.checksum == Self::recompute_checksum(body)
    }
}

struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn bump(&mut self) {
        self.pos += 1;
    }

    fn at_end(&self) -> bool {
        self.pos >= self.bytes.len()
    }

    fn mark(&self) -> usize {
        self.pos
    }

    fn reset(&mut self, pos: usize) {
        self.pos = pos;
    }

    fn slice(&self, start: usize, end: usize) -> &'a [u8] {
        &self.bytes[start..end]
    }

    fn expect(&mut self, byte: u8) -> TmuxResult<()> {
        if self.peek() == Some(byte) {
            self.bump();
            Ok(())
        } else {
            Err(TmuxError::InvalidLayout {
                reason: LayoutError::Expected { expected: byte },
            })
        }
    }

    fn skip_digits(&mut self) {
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.bump();
        }
    }

    fn take_u32(&mut self, field: &'static str) -> TmuxResult<u32> {
        let start = self.pos;
        self.skip_digits();
        parse_u32(self.slice(start, self.pos), field)
    }

    fn take_i32(&mut self, field: &'static str) -> TmuxResult<i32> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.bump();
        }
        self.skip_digits();
        let raw = self.slice(start, self.pos);
        core::str::from_utf8(raw)
            .ok()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| TmuxError::InvalidInt {
                field,
                raw: String::from_utf8_lossy(raw).into_owned(),
            })
    }
}

fn parse_checksum(bytes: &[u8]) -> TmuxResult<u16> {
    let bad = || TmuxError::InvalidLayout {
        reason: LayoutError::BadChecksum,
    };
    if bytes.len() != 4 {
        return Err(bad());
    }
    let mut value: u16 = 0;
    for &b in bytes {
        let nibble = match b {
            b'0'..=b'9' => b - b'0',
            b'a'..=b'f' => b - b'a' + 10,
            _ => return Err(bad()),
        };
        value = (value << 4) | u16::from(nibble);
    }
    Ok(value)
}

fn parse_cell(cur: &mut Cursor) -> TmuxResult<Cell> {
    let dims = parse_dims(cur)?;
    match cur.peek() {
        None => Ok(Cell::Leaf {
            dims,
            pane_id: None,
        }),
        Some(b'{') => {
            cur.bump();
            let children = parse_children(cur, b'}')?;
            Ok(Cell::Split {
                dims,
                dir: SplitDir::LeftRight,
                children,
            })
        }
        Some(b'[') => {
            cur.bump();
            let children = parse_children(cur, b']')?;
            Ok(Cell::Split {
                dims,
                dir: SplitDir::TopBottom,
                children,
            })
        }
        Some(b'<') => {
            cur.bump();
            let children = parse_children(cur, b'>')?;
            Ok(Cell::Split {
                dims,
                dir: SplitDir::Floating,
                children,
            })
        }
        Some(b',') => parse_after_comma(cur, dims),
        Some(byte) => Err(TmuxError::InvalidLayout {
            reason: LayoutError::UnexpectedByte { byte },
        }),
    }
}

fn parse_dims(cur: &mut Cursor) -> TmuxResult<CellDims> {
    let width = cur.take_u32("width")?;
    cur.expect(b'x')?;
    let height = cur.take_u32("height")?;
    cur.expect(b',')?;
    let xoff = cur.take_i32("xoff")?;
    cur.expect(b',')?;
    let yoff = cur.take_i32("yoff")?;
    Ok(CellDims {
        width,
        height,
        xoff,
        yoff,
    })
}

fn parse_after_comma(cur: &mut Cursor, dims: CellDims) -> TmuxResult<Cell> {
    let save = cur.mark();
    cur.bump();
    let start = cur.mark();
    cur.skip_digits();
    if cur.peek() == Some(b'x') {
        cur.reset(save);
        return Ok(Cell::Leaf {
            dims,
            pane_id: None,
        });
    }
    let end = cur.mark();
    if end == start {
        return Err(TmuxError::InvalidLayout {
            reason: LayoutError::ExpectedPaneId,
        });
    }
    let pane_id = parse_u32(cur.slice(start, end), "pane_id")?;
    Ok(Cell::Leaf {
        dims,
        pane_id: Some(pane_id),
    })
}

fn parse_children(cur: &mut Cursor, close: u8) -> TmuxResult<Vec<Cell>> {
    let mut children = Vec::new();
    loop {
        children.push(parse_cell(cur)?);
        match cur.peek() {
            Some(b',') => cur.bump(),
            Some(b) if b == close => {
                cur.bump();
                break;
            }
            _ => {
                return Err(TmuxError::InvalidLayout {
                    reason: LayoutError::UnbalancedBracket { expected: close },
                });
            }
        }
    }
    Ok(children)
}

fn parse_u32(bytes: &[u8], field: &'static str) -> TmuxResult<u32> {
    core::str::from_utf8(bytes)
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| TmuxError::InvalidInt {
            field,
            raw: String::from_utf8_lossy(bytes).into_owned(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &[u8]) -> WindowLayout {
        WindowLayout::parse(s).expect("should parse")
    }

    fn err(s: &[u8]) -> TmuxError {
        WindowLayout::parse(s).unwrap_err()
    }

    fn leaf(width: u32, height: u32, xoff: i32, yoff: i32, pane_id: Option<u32>) -> Cell {
        Cell::Leaf {
            dims: CellDims {
                width,
                height,
                xoff,
                yoff,
            },
            pane_id,
        }
    }

    #[test]
    fn single_leaf() {
        assert_eq!(
            p(b"b25f,80x24,0,0,2"),
            WindowLayout {
                checksum: 0xb25f,
                root: leaf(80, 24, 0, 0, Some(2)),
            }
        );
    }

    #[test]
    fn single_leaf_no_id() {
        assert_eq!(
            p(b"0000,80x24,0,0"),
            WindowLayout {
                checksum: 0,
                root: leaf(80, 24, 0, 0, None),
            }
        );
    }

    #[test]
    fn left_right_split() {
        assert_eq!(
            p(b"0000,80x24,0,0{40x24,0,0,1,39x24,41,0,2}").root,
            Cell::Split {
                dims: CellDims {
                    width: 80,
                    height: 24,
                    xoff: 0,
                    yoff: 0,
                },
                dir: SplitDir::LeftRight,
                children: vec![leaf(40, 24, 0, 0, Some(1)), leaf(39, 24, 41, 0, Some(2))],
            }
        );
    }

    #[test]
    fn top_bottom_split() {
        assert_eq!(
            p(b"0000,80x24,0,0[80x12,0,0,1,80x11,0,13,2]").root,
            Cell::Split {
                dims: CellDims {
                    width: 80,
                    height: 24,
                    xoff: 0,
                    yoff: 0,
                },
                dir: SplitDir::TopBottom,
                children: vec![leaf(80, 12, 0, 0, Some(1)), leaf(80, 11, 0, 13, Some(2))],
            }
        );
    }

    #[test]
    fn nested_mixed() {
        let root = p(b"0000,80x24,0,0{40x24,0,0,1,39x24,41,0[39x12,41,0,2,39x11,41,13,3]}").root;
        assert_eq!(
            root,
            Cell::Split {
                dims: CellDims {
                    width: 80,
                    height: 24,
                    xoff: 0,
                    yoff: 0,
                },
                dir: SplitDir::LeftRight,
                children: vec![
                    leaf(40, 24, 0, 0, Some(1)),
                    Cell::Split {
                        dims: CellDims {
                            width: 39,
                            height: 24,
                            xoff: 41,
                            yoff: 0,
                        },
                        dir: SplitDir::TopBottom,
                        children: vec![leaf(39, 12, 41, 0, Some(2)), leaf(39, 11, 41, 13, Some(3))],
                    },
                ],
            }
        );
    }

    #[test]
    fn pane_id_vs_x_ambiguity() {
        // First child `40x24,0,0` has NO pane id: the comma before `39` is a
        // sibling separator because `39` is followed by `x`.
        let root = p(b"0000,80x24,0,0{40x24,0,0,39x24,41,0,2}").root;
        let Cell::Split { children, .. } = root else {
            panic!("expected split");
        };
        assert_eq!(children[0], leaf(40, 24, 0, 0, None));
        assert_eq!(children[1], leaf(39, 24, 41, 0, Some(2)));
    }

    #[test]
    fn pane_id_present_contrast() {
        // Same shape, but the first child carries id 7 (`,7,` — the byte after
        // the digit is `,`, not `x`), proving the lookahead flips correctly.
        let root = p(b"0000,80x24,0,0{40x24,0,0,7,39x24,41,0,2}").root;
        let Cell::Split { children, .. } = root else {
            panic!("expected split");
        };
        assert_eq!(children[0], leaf(40, 24, 0, 0, Some(7)));
        assert_eq!(children[1], leaf(39, 24, 41, 0, Some(2)));
    }

    #[test]
    fn floating_group() {
        let root = p(b"0000,80x24,0,0<40x24,0,0,1,39x24,41,0,2>").root;
        assert!(matches!(
            root,
            Cell::Split {
                dir: SplitDir::Floating,
                ..
            }
        ));
    }

    #[test]
    fn negative_offsets() {
        assert_eq!(p(b"0000,80x24,-3,-5,1").root, leaf(80, 24, -3, -5, Some(1)));
    }

    #[test]
    fn multi_digit_ids_and_dims() {
        assert_eq!(
            p(b"0000,200x150,0,0,123").root,
            leaf(200, 150, 0, 0, Some(123))
        );
    }

    #[test]
    fn checksum_recompute_matches_tmux() {
        assert_eq!(WindowLayout::recompute_checksum(b"80x24,0,0,2"), 0xb25f);
    }

    #[test]
    fn checksum_mismatch_is_lenient() {
        let wl = p(b"0000,80x24,0,0,2");
        assert_eq!(wl.checksum, 0);
        assert!(!wl.verify(b"80x24,0,0,2"));
    }

    #[test]
    fn missing_checksum_comma() {
        assert!(matches!(
            err(b"b25f80x24"),
            TmuxError::InvalidLayout {
                reason: LayoutError::MissingChecksumComma
            }
        ));
    }

    #[test]
    fn bad_checksum_too_short() {
        assert!(matches!(
            err(b"b2f,80x24,0,0,2"),
            TmuxError::InvalidLayout {
                reason: LayoutError::BadChecksum
            }
        ));
    }

    #[test]
    fn bad_checksum_uppercase() {
        assert!(matches!(
            err(b"B25F,80x24,0,0,2"),
            TmuxError::InvalidLayout {
                reason: LayoutError::BadChecksum
            }
        ));
    }

    #[test]
    fn bad_checksum_non_hex() {
        assert!(matches!(
            err(b"b2zf,80x24,0,0,2"),
            TmuxError::InvalidLayout {
                reason: LayoutError::BadChecksum
            }
        ));
    }

    #[test]
    fn missing_x_in_dims() {
        assert!(matches!(
            err(b"0000,8024,0,0,2"),
            TmuxError::InvalidLayout {
                reason: LayoutError::Expected { expected: b'x' }
            }
        ));
    }

    #[test]
    fn non_numeric_dims() {
        assert!(matches!(
            err(b"0000,AAxBB,0,0,1"),
            TmuxError::InvalidInt { field: "width", .. }
        ));
    }

    #[test]
    fn comma_then_no_pane_digits() {
        assert!(matches!(
            err(b"0000,80x24,0,0,}"),
            TmuxError::InvalidLayout {
                reason: LayoutError::ExpectedPaneId
            }
        ));
    }

    #[test]
    fn unbalanced_open_brace() {
        assert!(matches!(
            err(b"0000,80x24,0,0{40x24,0,0,1"),
            TmuxError::InvalidLayout {
                reason: LayoutError::UnbalancedBracket { expected: b'}' }
            }
        ));
    }

    #[test]
    fn trailing_junk() {
        assert!(matches!(
            err(b"b25f,80x24,0,0,2}"),
            TmuxError::InvalidLayout {
                reason: LayoutError::TrailingData
            }
        ));
    }

    #[test]
    fn empty_input() {
        assert!(matches!(
            err(b""),
            TmuxError::InvalidLayout {
                reason: LayoutError::MissingChecksumComma
            }
        ));
    }
}
