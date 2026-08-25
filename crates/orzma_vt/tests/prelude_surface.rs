//! Compile-time guard on the crate's public vocabulary surface.
//!
//! The import list below IS the assertion: an integration test sees
//! only the public API, so a symbol dropped from `prelude` breaks this
//! file at compile time rather than silently shrinking the surface.

#[expect(
    unused_imports,
    reason = "the import list is the assertion; nothing needs to reference these"
)]
use orzma_vt::prelude::{
    ApcWebviewVerb, CURSOR_VISIBLE_BIT, CellSide, Color, Cursor, CursorShape, DirtyRow,
    DisplayOffset, Frame, GridColumn, GridLine, GridPoint, GridSize, Hyperlink, HyperlinkId,
    HyperlinkUri, MouseEncoding, MouseTracking, OrzmaVt, Palette, PlacementId, ProjectedPlacement,
    Rgb, Row, Run, ScreenKind, ScreenLine, Scroll, SelectionGeometry, SelectionKind,
    SelectionRange, Style, ViCursor, ViModeSwitch, ViewportLine, Vt, VtModes, VtSignal, VtUpdate,
    is_allowed,
};

/// Asserts that the prelude still exposes a usable terminal: a `Vt` can
/// be built through it and reports the size it was given.
///
/// Case: a downstream crate depends on `orzma_vt` and reaches the whole
/// vocabulary through `orzma_vt::prelude`, as `orzma_tty` and
/// `bevy_orzma_tty` already do.
#[test]
fn the_prelude_exposes_a_usable_terminal() {
    let vt = OrzmaVt::new(GridSize { cols: 80, rows: 24 }, 1000);
    assert_eq!(vt.grid_size(), GridSize { cols: 80, rows: 24 });
}
