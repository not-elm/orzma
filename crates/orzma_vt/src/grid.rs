use unicode_width::UnicodeWidthChar;

use crate::grid::{cell::Cell, cursor::Cursor};

pub mod cell;
mod cursor;

pub struct Grid {
    lines: Vec<Line>,
    cols: usize,
    rows: usize,
    cursor: Cursor,
}

impl Grid {
    pub fn new(cols: usize, rows: usize) -> Self {
        Self {
            cols,
            rows,
            lines: vec![],
            cursor: Cursor::default(),
        }
    }

    pub fn write(&mut self, c: char) {
        let Some(cell_width) = c.width() else {
            return;
        };
        let y = self.cursor.y;
        let x = self.cursor.x;
        self.lines[y].0[x].c = c;
    }

    #[cfg(test)]
    fn assert_char_at(&self, cols: usize, row: usize, c: char) {
        assert_eq!(self.lines[cols].0[row].c, c);
    }
}

pub struct Line(Vec<Cell>);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_places_width_one_char_at_origin() {
        let mut grid = Grid::new(4, 3);
        grid.write('a');
        grid.assert_char_at(0, 0, 'a');
    }
}
