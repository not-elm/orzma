//! Event-loop tests for workspaces: creation, display, closing, and the
//! hidden panes' size and focus.

use crate::backend::{
    NewPaneAt, OrzmuxEvent, PaneId, PaneTarget, RequestId, SplitOrientation, WorkspaceId,
};
use crate::event_loop::OrzmuxCommand;
use crate::event_loop::tests::enable_focus_reporting;
use crate::test_support::Harness;
use orzma_vt::prelude::GridSize;
use std::collections::VecDeque;

fn workspaces_of(events: &VecDeque<OrzmuxEvent>) -> Vec<(Vec<WorkspaceId>, Option<WorkspaceId>)> {
    events
        .iter()
        .filter_map(|e| match e {
            OrzmuxEvent::Workspaces {
                entries, active, ..
            } => Some((entries.iter().map(|w| w.id).collect(), *active)),
            _ => None,
        })
        .collect()
}

fn last_layout_panes(events: &VecDeque<OrzmuxEvent>) -> Vec<PaneId> {
    events
        .iter()
        .rev()
        .find_map(|e| match e {
            OrzmuxEvent::Layout { layout, .. } => {
                Some(layout.panes.iter().map(|r| r.pane).collect())
            }
            _ => None,
        })
        .unwrap_or_default()
}

/// Asserts that the first pane opens the first workspace and announces it
/// between `PaneOpened` and `Layout`.
///
/// Case: orzma starts and asks for its first shell.
#[test]
fn the_first_pane_opens_the_first_workspace() {
    let mut h = Harness::new();
    h.resize(GridSize::new(80, 24).expect("a valid size"));
    h.drain();
    h.send(OrzmuxCommand::NewPane {
        request: RequestId(1),
        at: NewPaneAt::Workspace,
        cwd: None,
        env: vec![],
    });
    let events = h.drain();
    assert!(matches!(events[0], OrzmuxEvent::PaneOpened { .. }));
    assert!(matches!(events[1], OrzmuxEvent::Workspaces { .. }));
    assert!(matches!(events[2], OrzmuxEvent::Layout { .. }));
    assert_eq!(
        workspaces_of(&events),
        vec![(vec![WorkspaceId(1)], Some(WorkspaceId(1)))]
    );
}

/// Asserts that a new workspace is displayed and that the `Layout` then
/// lists only its pane.
///
/// Case: the user presses new-workspace while a shell runs in the first
/// workspace.
#[test]
fn a_new_workspace_is_displayed_alone() {
    let mut h = Harness::new();
    let (first, _p1) = h.open_root();
    let (second, _p2) = h.open_workspace(RequestId(2));
    let layout = h.backend().workspaces().active().map(|w| w.tree.panes());
    assert_eq!(layout, Some(vec![second]));
    assert_ne!(first, second);
}

/// Asserts that the last pane of the last workspace empties both the
/// `Workspaces` snapshot and the `Layout`.
///
/// Case: the user exits the only shell orzma has.
#[test]
fn the_last_workspace_closing_empties_everything() {
    let mut h = Harness::new();
    let (root, pane) = h.open_root();
    pane.exit(Some(0));
    h.pump_pane(root);
    let events = h.drain();
    assert_eq!(workspaces_of(&events), vec![(vec![], None)]);
    assert!(last_layout_panes(&events).is_empty());
}

/// Asserts that a window resize resizes the panes of hidden workspaces and
/// carries their repaint in `Layout.frames`.
///
/// Case: the user widens the window while a build runs in a background
/// tab.
#[test]
fn a_resize_resizes_hidden_panes_and_carries_their_frames() {
    let mut h = Harness::new();
    let (hidden, _p1) = h.open_root();
    let (_shown, _p2) = h.open_workspace(RequestId(2));
    h.resize(GridSize::new(100, 30).expect("a valid size"));
    let events = h.drain();
    let Some(OrzmuxEvent::Layout { frames, .. }) = events
        .iter()
        .rev()
        .find(|e| matches!(e, OrzmuxEvent::Layout { .. }))
    else {
        panic!("expected a Layout, got {events:?}");
    };
    assert!(frames.iter().any(|(pane, _)| *pane == hidden));
    let size = h
        .backend()
        .pane(hidden)
        .expect("the hidden pane")
        .tty
        .pty_size();
    assert_eq!((size.cols, size.rows), (100, 30));
}

/// Asserts that selecting a pane of a hidden workspace neither selects it
/// nor switches workspaces, and still answers with a `Layout`.
///
/// Case: a stale click or a background program names a pane that is no
/// longer on screen.
#[test]
fn selecting_a_hidden_pane_changes_nothing_but_answers() {
    let mut h = Harness::new();
    let (hidden, _p1) = h.open_root();
    let (shown, _p2) = h.open_workspace(RequestId(2));
    h.send(OrzmuxCommand::SelectPane { pane: hidden });
    let events = h.drain();
    let Some(OrzmuxEvent::Layout { layout, .. }) = events.back() else {
        panic!("expected a Layout, got {events:?}");
    };
    assert_eq!(layout.active, Some(shown));
    assert!(workspaces_of(&events).is_empty());
}

/// Asserts that a split must target the displayed workspace.
///
/// Case: a split request names a pane of a hidden tab.
#[test]
fn a_split_of_a_hidden_pane_fails_to_spawn() {
    let mut h = Harness::new();
    let (hidden, _p1) = h.open_root();
    let (_shown, _p2) = h.open_workspace(RequestId(2));
    h.send(OrzmuxCommand::NewPane {
        request: RequestId(3),
        at: NewPaneAt::Split {
            pane: PaneTarget::Id(hidden),
            orientation: SplitOrientation::Vertical,
        },
        cwd: None,
        env: vec![],
    });
    let events = h.drain();
    assert!(matches!(
        events.front(),
        Some(OrzmuxEvent::SpawnFailed { .. })
    ));
}

/// Asserts that a failed spawn of a new workspace leaves the set and the
/// displayed workspace as they were.
///
/// Case: the shell binary vanished while orzma runs, and the user presses
/// new-workspace.
#[test]
fn a_failed_new_workspace_is_rolled_back() {
    let mut h = Harness::new();
    let (root, _p1) = h.open_root();
    h.fail_next_spawn();
    h.send(OrzmuxCommand::NewPane {
        request: RequestId(2),
        at: NewPaneAt::Workspace,
        cwd: None,
        env: vec![],
    });
    let events = h.drain();
    assert!(matches!(
        events.front(),
        Some(OrzmuxEvent::SpawnFailed { .. })
    ));
    assert!(workspaces_of(&events).is_empty());
    assert_eq!(h.backend().workspaces().entries().len(), 1);
    assert_eq!(
        h.backend()
            .workspaces()
            .active()
            .and_then(|w| w.tree.active()),
        Some(root)
    );
}

/// Asserts that moving a divider of a hidden workspace publishes nothing
/// and leaves that workspace's panes as they were.
///
/// Case: a stale divider drag from the previous workspace arrives after
/// the user switched tabs.
#[test]
fn a_resize_split_of_a_hidden_workspace_does_nothing() {
    let window = GridSize::new(80, 24).expect("a valid size");
    let mut h = Harness::new();
    let (root, _p1) = h.open_root();
    h.send(OrzmuxCommand::NewPane {
        request: RequestId(2),
        at: NewPaneAt::Split {
            pane: PaneTarget::Active,
            orientation: SplitOrientation::Vertical,
        },
        cwd: None,
        env: vec![],
    });
    let mut split_events = h.drain();
    let Some(OrzmuxEvent::Layout { layout, .. }) = split_events.pop_back() else {
        panic!("expected a Layout after the split, got {split_events:?}");
    };
    let split = layout.separators[0].split;
    let hidden = h
        .backend()
        .workspaces()
        .workspace_of(root)
        .expect("the root pane's workspace");
    let (_shown, _p3) = h.open_workspace(RequestId(3));
    let hidden_widths = |h: &Harness| -> Vec<u16> {
        h.backend()
            .workspaces()
            .get(hidden)
            .expect("the hidden workspace")
            .tree
            .solve(window)
            .panes
            .iter()
            .map(|rect| rect.cols)
            .collect()
    };
    let before = hidden_widths(&h);
    h.send(OrzmuxCommand::ResizeSplit {
        split,
        position: 60,
    });
    assert!(h.drain().is_empty());
    assert_eq!(hidden_widths(&h), before);
}

/// Asserts that focus reports reach only the displayed active pane: the
/// pane a new workspace hides reports focus loss and does not regain focus
/// while hidden.
///
/// Case: the user opens a new tab while nvim runs in the first one, then
/// resizes the window.
#[test]
fn focus_reports_reach_only_the_displayed_active_pane() {
    let mut h = Harness::new();
    let (root, root_pane) = h.open_root();
    enable_focus_reporting(&mut h, root, &root_pane);
    let (_shown, _p2) = h.open_workspace(RequestId(2));
    h.resize(GridSize::new(100, 30).expect("a valid size"));
    h.settle_writes();
    assert_eq!(root_pane.received(), b"\x1b[O");
}
