//! The per-cell pixel pitch a host reports for a terminal, and its
//! projection of a grid size onto the PTY winsize.

use orzma_vt::prelude::GridSize;
use portable_pty::PtySize;

/// Physical pixels per terminal cell, as the host measures its font.
///
/// `Default` is `0 × 0`, which projects to a zero-pixel winsize for a
/// caller that has no metrics yet.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct CellPixels {
    /// Horizontal cell pitch in physical pixels.
    pub width: u16,
    /// Vertical cell pitch in physical pixels.
    pub height: u16,
}

impl CellPixels {
    /// The PTY winsize for `size`: its cell counts, with the pixel
    /// fields set to the total window pixels `self × size`, saturating
    /// at `u16::MAX` per axis.
    pub fn pty_size(self, size: GridSize) -> PtySize {
        let GridSize { cols, rows } = size;
        if self.width.checked_mul(cols).is_none() || self.height.checked_mul(rows).is_none() {
            tracing::debug!(cols, rows, ?self, "pixel winsize saturated at u16::MAX");
        }
        PtySize {
            rows,
            cols,
            pixel_width: self.width.saturating_mul(cols),
            pixel_height: self.height.saturating_mul(rows),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(cols: u16, rows: u16) -> GridSize {
        GridSize::new(cols, rows).expect("a valid size")
    }

    /// Asserts that the winsize carries the cell counts and the cell
    /// pitch multiplied by them on each axis.
    ///
    /// Case: the GUI measures an 8×16 px cell and the layout gives a pane
    /// 80 columns by 24 rows.
    #[test]
    fn the_winsize_multiplies_the_pitch_by_the_cell_counts() {
        let px = CellPixels {
            width: 8,
            height: 16,
        };
        assert_eq!(
            px.pty_size(grid(80, 24)),
            PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 640,
                pixel_height: 384,
            }
        );
    }

    /// Asserts that a product beyond `u16::MAX` saturates instead of
    /// wrapping.
    ///
    /// Case: the layout gives a pane 4096 columns at a 32 px cell pitch
    /// on a very wide display.
    #[test]
    fn the_winsize_pixels_saturate_instead_of_wrapping() {
        let px = CellPixels {
            width: 32,
            height: 16,
        };
        let size = px.pty_size(grid(4096, 24));
        assert_eq!((size.pixel_width, size.pixel_height), (u16::MAX, 384));
    }

    /// Asserts that the default pitch projects to a zero-pixel winsize.
    ///
    /// Case: a caller without font metrics spawns a terminal.
    #[test]
    fn the_default_pitch_projects_to_zero_pixels() {
        let size = CellPixels::default().pty_size(grid(80, 24));
        assert_eq!((size.pixel_width, size.pixel_height), (0, 0));
    }
}
