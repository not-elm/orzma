//! Mode-flag, resize, and grid-size tests.

use super::*;

// NOTE: alacritty's `TermMode::default()` enables ALTERNATE_SCROLL,
// so a fresh terminal is NOT `VtModes::default()`.
fn baseline() -> VtModes {
    VtModes {
        alternate_scroll: true,
        ..VtModes::default()
    }
}

#[test]
fn fresh_terminal_reports_alacritty_baseline() {
    let vt = AlacrittyVtBackend::new(80, 24);
    assert_eq!(vt.modes(), baseline());
}

/// Asserts that `resize` reshapes the emulated grid to the
/// requested dimensions.
///
/// Case: the user resizes the window to a non-square grid.
#[test]
fn resize_updates_the_grid_size() {
    let mut vt = AlacrittyVtBackend::new(80, 24);
    vt.resize(120, 40);
    assert_eq!(
        vt.grid_size(),
        GridSize {
            cols: 120,
            rows: 40
        }
    );
}

/// Asserts that `resize` reports full damage.
///
/// Case: a resize reflows the whole grid, but no PTY output need
/// follow — an idle shell prompt stays idle.
#[test]
fn resize_reports_full_damage() {
    let mut vt = AlacrittyVtBackend::new(80, 24);
    assert_eq!(vt.resize(120, 40), Some(StagedDamage::Full));
}

/// Asserts that a resize to the current dimensions reports no damage.
///
/// Case: the host recomputes cells after a pixel-only window change
/// and re-applies the grid size the terminal already has.
#[test]
fn a_same_size_resize_reports_no_damage() {
    let mut vt = AlacrittyVtBackend::new(80, 24);
    assert_eq!(vt.resize(80, 24), None);
}

/// Asserts that `grid_size` maps the term's columns to `cols` and its
/// screen lines to `rows`.
///
/// Case: paging on a non-square 80x24 grid, where half a page must
/// resolve from the 24-row axis rather than the 80-column one.
#[test]
fn grid_size_maps_cols_and_rows_from_the_term() {
    assert_eq!(
        AlacrittyVtBackend::new(80, 24).grid_size(),
        GridSize { cols: 80, rows: 24 }
    );
}

#[test]
fn decset_sets_flags_and_enums() {
    let vt = vt_after(b"\x1b[?1h\x1b[?2004h\x1b[?1004h\x1b[?1000h\x1b[?1006h");
    assert_eq!(
        vt.modes(),
        VtModes {
            app_cursor: true,
            bracketed_paste: true,
            focus_in_out: true,
            mouse_tracking: MouseTracking::Clicks,
            mouse_encoding: MouseEncoding::Sgr,
            ..baseline()
        }
    );
}

#[test]
fn mouse_encodings_are_exclusive() {
    let vt = vt_after(b"\x1b[?1005h\x1b[?1006h");
    assert_eq!(vt.modes().mouse_encoding, MouseEncoding::Sgr);
    let vt = vt_after(b"\x1b[?1006h\x1b[?1005h");
    assert_eq!(vt.modes().mouse_encoding, MouseEncoding::Utf8);
}

#[test]
fn mouse_tracking_levels_replace_each_other() {
    let vt = vt_after(b"\x1b[?1000h\x1b[?1003h");
    assert_eq!(vt.modes().mouse_tracking, MouseTracking::Motion);
    let vt = vt_after(b"\x1b[?1002h");
    assert_eq!(vt.modes().mouse_tracking, MouseTracking::Drag);
}

#[test]
fn alt_screen_and_decrst_roundtrip() {
    let vt = vt_after(b"\x1b[?1049h");
    assert_eq!(vt.modes().active_screen, ScreenKind::Alternate);
    let vt = vt_after(b"\x1b[?1006h\x1b[?1006l");
    assert_eq!(vt.modes().mouse_encoding, MouseEncoding::X10);
    let vt = vt_after(b"\x1b[?1049h\x1b[?1007l");
    assert!(!vt.modes().alternate_scroll);
    assert!(!vt.modes().alternate_scroll_active());
}
