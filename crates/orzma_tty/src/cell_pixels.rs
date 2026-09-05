//! The per-cell pixel pitch a host reports for a terminal, and its
//! projection onto the total window pixels a PTY winsize carries.

/// Physical pixels per terminal cell, as the host measures its font.
///
/// `Default` is `0 × 0`, which projects to a zero-pixel winsize (the
/// pre-multiplexer behaviour) for callers that have no metrics yet.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct CellPixels {
    /// Horizontal cell pitch in physical pixels.
    pub width: u16,
    /// Vertical cell pitch in physical pixels.
    pub height: u16,
}

impl CellPixels {
    /// Total window pixels for a `cols × rows` grid, saturating at
    /// `u16::MAX` per axis.
    ///
    /// The result is what `ws_xpixel` / `ws_ypixel` expect: tmux writes
    /// `xpixel * cols`, and portable-pty forwards `pixel_width` verbatim
    /// into `ws_xpixel` despite documenting it as a per-cell width.
    pub fn window_pixels(self, cols: u16, rows: u16) -> (u16, u16) {
        let width = self.width.saturating_mul(cols);
        let height = self.height.saturating_mul(rows);
        if (self.width != 0 && width == u16::MAX) || (self.height != 0 && height == u16::MAX) {
            tracing::debug!(cols, rows, ?self, "pixel winsize saturated at u16::MAX");
        }
        (width, height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts that the window pixels are the cell pitch multiplied by the
    /// cell counts on each axis.
    ///
    /// Case: the GUI measures an 8×16 px cell and the layout gives a pane
    /// 80 columns by 24 rows.
    #[test]
    fn window_pixels_multiply_the_pitch_by_the_cell_counts() {
        let px = CellPixels {
            width: 8,
            height: 16,
        };
        assert_eq!(px.window_pixels(80, 24), (640, 384));
    }

    /// Asserts that a product beyond `u16::MAX` saturates instead of
    /// wrapping.
    ///
    /// Case: a 4096-column grid at a 32 px cell pitch on a very wide
    /// display.
    #[test]
    fn window_pixels_saturate_instead_of_wrapping() {
        let px = CellPixels {
            width: 32,
            height: 16,
        };
        assert_eq!(px.window_pixels(4096, 24), (u16::MAX, 384));
    }

    /// Asserts that the default pitch projects to a zero-pixel winsize.
    ///
    /// Case: a caller without font metrics spawns a terminal.
    #[test]
    fn default_pitch_projects_to_zero_pixels() {
        assert_eq!(CellPixels::default().window_pixels(80, 24), (0, 0));
    }
}
