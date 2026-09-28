//! Event-loop tests for workspaces: creation, display, closing, and the
//! hidden panes' size and focus.

use crate::backend::{
    CloseReason, CloseTarget, NewPaneAt, OrzmuxEvent, PaneId, PaneTarget, RequestId,
    SplitOrientation, WorkspaceId, WorkspaceTarget,
};
use crate::event_loop::OrzmuxCommand;
use crate::event_loop::tests::{enable_focus_reporting, osc7};
use crate::test_support::Harness;
use orzma_vt::prelude::GridSize;
use std::collections::VecDeque;
use tempfile::TempDir;

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

/// Asserts that a new workspace with no explicit directory starts in the
/// directory the displayed workspace's active pane reported, not in a
/// hidden workspace's.
///
/// Case: the user `cd`s into a project in the second workspace's shell
/// while the first workspace's shell reported another directory, and
/// presses new-workspace while the OS cannot be asked for either pane's
/// directory.
#[test]
fn a_new_workspace_inherits_the_displayed_active_panes_cwd() {
    let elsewhere = TempDir::new().expect("a temporary directory");
    let project = TempDir::new().expect("a temporary directory");
    let mut h = Harness::new();
    let (first, first_pane) = h.open_root();
    first_pane.print(&osc7(elsewhere.path()));
    h.pump_pane(first);
    h.drain();
    let (second, second_pane) = h.open_workspace(RequestId(2));
    second_pane.print(&osc7(project.path()));
    h.pump_pane(second);
    h.drain();
    h.clear_spawn_cwds();

    h.open_workspace(RequestId(3));

    assert_eq!(h.last_spawn_cwd().as_deref(), Some(project.path()));
}

/// Asserts that a new workspace is announced as displayed between
/// `PaneOpened` and `Layout`, and that the `Layout` then lists only its
/// pane.
///
/// Case: the user presses new-workspace while a shell runs in the first
/// workspace.
#[test]
fn a_new_workspace_is_displayed_alone() {
    let mut h = Harness::new();
    let (first, _p1) = h.open_root();
    h.send(OrzmuxCommand::NewPane {
        request: RequestId(2),
        at: NewPaneAt::Workspace,
        cwd: None,
        env: vec![],
    });
    let events = h.drain();
    let Some(OrzmuxEvent::PaneOpened { pane: second, .. }) = events.front() else {
        panic!("expected PaneOpened, got {events:?}");
    };
    assert_ne!(first, *second);
    assert!(
        matches!(
            events.get(1),
            Some(OrzmuxEvent::Workspaces { entries, active, .. })
                if entries.iter().map(|w| w.id).eq([WorkspaceId(1), WorkspaceId(2)])
                    && *active == Some(WorkspaceId(2))
        ),
        "expected Workspaces displaying the new workspace, got {events:?}"
    );
    let Some(OrzmuxEvent::Layout { layout, .. }) = events.get(2) else {
        panic!("expected a Layout after Workspaces, got {events:?}");
    };
    assert_eq!(
        layout.panes.iter().map(|r| r.pane).collect::<Vec<_>>(),
        vec![*second]
    );
    assert_eq!(layout.active, Some(*second));
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
/// pane a new workspace hides reports focus loss, does not regain focus
/// while hidden, and regains it once its workspace is displayed again,
/// while the pane that loses the display reports focus loss in turn.
///
/// Case: the user opens a new tab while nvim runs in the first one,
/// resizes the window, then switches back to the first tab.
#[test]
fn focus_reports_reach_only_the_displayed_active_pane() {
    let mut h = Harness::new();
    let (root, root_pane) = h.open_root();
    enable_focus_reporting(&mut h, root, &root_pane);
    let (shown, shown_pane) = h.open_workspace(RequestId(2));
    enable_focus_reporting(&mut h, shown, &shown_pane);
    h.resize(GridSize::new(100, 30).expect("a valid size"));
    h.settle_writes();
    assert_eq!(root_pane.received(), b"\x1b[O");
    let first_ws = h
        .backend()
        .workspaces()
        .workspace_of(root)
        .expect("root's workspace");
    h.send(OrzmuxCommand::SelectWorkspace {
        workspace: WorkspaceTarget::Id(first_ws),
    });
    h.settle_writes();
    assert_eq!(root_pane.received(), b"\x1b[O\x1b[I");
    assert_eq!(shown_pane.received(), b"\x1b[O");
}

/// Asserts that when the displayed workspace's last pane closes, the
/// workspace to its right is displayed and the session goes on.
///
/// Case: the user exits the only shell of the middle tab while two other
/// tabs are open.
#[test]
fn the_last_pane_closing_displays_the_right_neighbour() {
    let mut h = Harness::new();
    let (_a, _pa) = h.open_root();
    let (b, pb) = h.open_workspace(RequestId(2));
    let (c, _pc) = h.open_workspace(RequestId(3));
    let ws_b = h
        .backend()
        .workspaces()
        .workspace_of(b)
        .expect("b's workspace");
    h.send(OrzmuxCommand::SelectWorkspace {
        workspace: WorkspaceTarget::Id(ws_b),
    });
    h.drain();
    pb.exit(Some(0));
    h.pump_pane(b);
    let events = h.drain();
    assert!(events.iter().any(|e| matches!(
        e,
        OrzmuxEvent::PaneClosed { pane, reason: CloseReason::ChildExit { .. } } if *pane == b
    )));
    assert_eq!(last_layout_panes(&events), vec![c]);
    let (ids, active) = workspaces_of(&events).pop().expect("a Workspaces snapshot");
    assert_eq!(ids.len(), 2);
    assert_eq!(active, h.backend().workspaces().workspace_of(c));
}

/// Asserts that `Next` and `Previous` wrap, that the displayed workspace's
/// pane becomes the `Layout`, and that selecting the displayed workspace
/// or a missing index publishes nothing.
///
/// Case: the user cycles through three tabs and presses the key for a
/// ninth one.
#[test]
fn selecting_workspaces_wraps_and_ignores_no_ops() {
    let mut h = Harness::new();
    let (a, _pa) = h.open_root();
    let (_b, _pb) = h.open_workspace(RequestId(2));
    let (c, _pc) = h.open_workspace(RequestId(3));
    h.send(OrzmuxCommand::SelectWorkspace {
        workspace: WorkspaceTarget::Next,
    });
    assert_eq!(last_layout_panes(&h.drain()), vec![a]);
    h.send(OrzmuxCommand::SelectWorkspace {
        workspace: WorkspaceTarget::Previous,
    });
    assert_eq!(last_layout_panes(&h.drain()), vec![c]);
    h.send(OrzmuxCommand::SelectWorkspace {
        workspace: WorkspaceTarget::Active,
    });
    assert!(h.drain().is_empty());
    h.send(OrzmuxCommand::SelectWorkspace {
        workspace: WorkspaceTarget::Index(8),
    });
    assert!(h.drain().is_empty());
}

/// Asserts that closing a workspace kills each of its panes, then emits
/// one `Workspaces` and one `Layout` of the neighbour.
///
/// Case: the user closes a tab holding two split shells.
#[test]
fn closing_a_workspace_kills_its_panes_and_shows_the_neighbour() {
    let mut h = Harness::new();
    let (a, _pa) = h.open_root();
    let (b, _pb) = h.open_workspace(RequestId(2));
    h.send(OrzmuxCommand::NewPane {
        request: RequestId(3),
        at: NewPaneAt::Split {
            pane: PaneTarget::Active,
            orientation: SplitOrientation::Vertical,
        },
        cwd: None,
        env: vec![],
    });
    h.drain();
    h.send(OrzmuxCommand::CloseWorkspace {
        workspace: CloseTarget::Active,
    });
    let events = h.drain();
    let closed: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            OrzmuxEvent::PaneClosed {
                pane,
                reason: CloseReason::Killed,
            } => Some(*pane),
            _ => None,
        })
        .collect();
    assert_eq!(closed.len(), 2);
    assert!(closed.contains(&b));
    assert_eq!(workspaces_of(&events).len(), 1);
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, OrzmuxEvent::Layout { .. }))
            .count(),
        1
    );
    assert_eq!(last_layout_panes(&events), vec![a]);
}

/// Asserts that a rename is sanitized and announced only when it changes
/// the name.
///
/// Case: the user renames a tab, then confirms the same name again.
#[test]
fn a_rename_is_announced_only_on_change() {
    let mut h = Harness::new();
    let (_a, _pa) = h.open_root();
    let id = WorkspaceId(1);
    h.send(OrzmuxCommand::RenameWorkspace {
        workspace: id,
        name: Some(" logs\n".into()),
    });
    let events = h.drain();
    let Some(OrzmuxEvent::Workspaces { entries, .. }) = events.front() else {
        panic!("expected Workspaces, got {events:?}");
    };
    assert_eq!(entries[0].name.as_deref(), Some("logs"));
    h.send(OrzmuxCommand::RenameWorkspace {
        workspace: id,
        name: Some("logs".into()),
    });
    assert!(h.drain().is_empty());
}

/// Asserts that every `MoveWorkspace`, including a no-op and one naming a
/// closed workspace, is answered with exactly one `Workspaces` carrying
/// its sequence.
///
/// Case: the user drops a tab where it was, and another drop races the
/// close of the dragged tab.
#[test]
fn move_workspace_always_answers_with_workspaces() {
    let mut h = Harness::new();
    let (_a, _pa) = h.open_root();
    let (_b, _pb) = h.open_workspace(RequestId(2));
    for (workspace, index) in [
        (WorkspaceId(1), 1),
        (WorkspaceId(1), 1),
        (WorkspaceId(9), 0),
    ] {
        let seq = h.send(OrzmuxCommand::MoveWorkspace { workspace, index });
        let events = h.drain();
        assert_eq!(events.len(), 1, "one answer for {workspace:?} → {index}");
        assert!(matches!(
            events[0],
            OrzmuxEvent::Workspaces { seq: answered, .. } if answered == seq
        ));
    }
}
