//! Tests for the webview placements anchored to screen rows.

use super::*;

fn mount(screen: &mut Screen, id: u64, view: &str) {
    screen.mount_placement(
        PlacementId(id),
        PlacementSize { rows: 2, cols: 4 },
        view.to_string(),
        None,
    );
}

/// Asserts that a mount anchors to the row the write cursor sits
/// on and to the cursor's column.
///
/// Case: a program prints a header, moves the cursor down two
/// rows and across three columns, and mounts a webview there.
#[test]
fn a_mount_anchors_at_the_write_cursor() {
    let mut screen = screen();
    screen.state.line = ScreenLine(2);
    screen.state.column = GridColumn(3);
    mount(&mut screen, 1, "memo");
    let projected = screen.project_placements();
    assert_eq!(projected.len(), 1);
    assert_eq!(projected[0].point.line, GridLine(2));
    assert_eq!(projected[0].point.column, GridColumn(3));
}

/// Asserts that a placement whose anchor row left the ring is
/// omitted by the projection and named by the sweep, so the two
/// cannot disagree.
///
/// Case: a webview sits on the last row of the screen and a
/// full-screen application scrolls backwards until that row is
/// discarded.
#[test]
fn a_placement_the_projection_omits_is_also_swept() {
    let mut screen = screen();
    screen.state.line = ScreenLine(2);
    mount(&mut screen, 1, "memo");
    assert_eq!(screen.project_placements().len(), 1);

    for _ in 0..3 {
        screen.reverse_index();
    }

    assert!(screen.project_placements().is_empty());
    assert_eq!(screen.evict_lost_anchors(), vec![PlacementId(1)]);
    assert_eq!(screen.placement_count(), 0);
}

/// Asserts that an anchor pushed into history projects to a
/// negative grid line and survives the sweep, and that a reset
/// is what makes that same placement evictable.
///
/// Case: a webview was mounted beside a line of output that the
/// shell has since scrolled into the scrollback, and the shell
/// then sends `ESC c`.
#[test]
fn a_reset_drops_a_placement_the_sweep_alone_would_leave() {
    let mut screen = screen();
    screen.state.line = ScreenLine(2);
    mount(&mut screen, 1, "memo");
    for _ in 0..3 {
        screen.line_feed();
    }
    assert_eq!(screen.project_placements()[0].point.line, GridLine(-1));
    assert!(screen.evict_lost_anchors().is_empty());

    assert_eq!(screen.reset(), Some(DamageSpan::Full));

    assert_eq!(screen.evict_lost_anchors(), vec![PlacementId(1)]);
}

/// Asserts that a reset leaves this screen's placements
/// unresolvable, so the next sweep names all of them.
///
/// Case: an application sends `RIS` while webviews are mounted on
/// the screen it resets.
#[test]
fn a_reset_leaves_this_screens_placements_unresolvable() {
    let mut screen = screen();
    mount(&mut screen, 1, "memo");
    mount(&mut screen, 2, "chart");
    assert_eq!(screen.reset(), None);
    assert!(screen.project_placements().is_empty());
    assert_eq!(
        screen.evict_lost_anchors(),
        vec![PlacementId(1), PlacementId(2)]
    );
}

/// Asserts that a re-mount at the same address supersedes the
/// live placement without naming the superseded id.
///
/// Case: a program re-renders the same named view.
#[test]
fn a_remount_supersedes_without_naming_the_superseded_id() {
    let mut screen = screen();
    mount(&mut screen, 1, "memo");
    screen.supersede_placement("memo", None);
    mount(&mut screen, 2, "memo");
    assert_eq!(screen.placement_count(), 1);
    assert_eq!(screen.take_placements(), vec![PlacementId(2)]);
}

/// Asserts that an unmount addressed to a view removes it and
/// reports that something went.
///
/// Case: a program tears down one of two mounted views.
#[test]
fn an_unmount_removes_the_addressed_placement() {
    let mut screen = screen();
    mount(&mut screen, 1, "memo");
    mount(&mut screen, 2, "chart");
    assert!(screen.unmount_placement(Some("memo"), None));
    assert_eq!(screen.placement_count(), 1);
}
