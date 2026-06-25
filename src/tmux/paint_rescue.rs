//! Structural rescue for tmux panes whose grid was left unpainted after a
//! layout change: detects the unpainted state and asks `ozmux_tmux` to
//! re-`capture-pane` until the grid paints (spec Component 2).

/// Returns whether a pane's grid is structurally unpainted and needs a full
/// re-seed. The dims-vs-handle clause catches the common `0×0` grid; the
/// `cells_len != rows` clause catches a grid whose dims were written but whose
/// rows were never repopulated (e.g. a lost resize snapshot). A genuinely blank
/// captured pane has `cells_len == rows`, so it does not fire.
// NOTE: the only caller (the rescue system) lands in the next task; this
// allowance is removed then. #[expect] is unusable here because the fn is
// live in the test target but dead in the lib build.
#[allow(dead_code)]
fn grid_needs_full_seed(
    grid_cols: u16,
    grid_rows: u16,
    cells_len: usize,
    handle_cols: u16,
    handle_rows: u16,
) -> bool {
    (grid_cols, grid_rows) != (handle_cols, handle_rows) || cells_len != grid_rows as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_grid_against_sized_handle_needs_seed() {
        assert!(grid_needs_full_seed(0, 0, 0, 80, 24));
    }

    #[test]
    fn dims_written_but_cells_empty_needs_seed() {
        assert!(grid_needs_full_seed(80, 24, 0, 80, 24));
    }

    #[test]
    fn blank_captured_pane_does_not_need_seed() {
        // A real snapshot yields one (possibly empty) row vector per row.
        assert!(!grid_needs_full_seed(80, 24, 24, 80, 24));
    }

    #[test]
    fn painted_matching_grid_does_not_need_seed() {
        assert!(!grid_needs_full_seed(80, 24, 24, 80, 24));
    }
}
