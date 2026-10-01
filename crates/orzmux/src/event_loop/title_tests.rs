//! Event-loop tests for pane titles: the backend keeps each pane's title
//! and announces only real changes.

use crate::backend::{NewPaneAt, OrzmuxEvent, PaneId, RequestId};
use crate::event_loop::OrzmuxCommand;
use crate::test_support::Harness;
use orzma_vt::prelude::{GridSize, VtSignal};
use std::collections::VecDeque;

fn titles_of(events: &VecDeque<OrzmuxEvent>) -> Vec<(PaneId, Option<String>)> {
    events
        .iter()
        .filter_map(|e| match e {
            OrzmuxEvent::PaneTitle { pane, title } => Some((*pane, title.clone())),
            _ => None,
        })
        .collect()
}

/// Asserts that a title is announced once and trimmed, that repeating it
/// announces nothing, and that a reset or a blank title announces `None`.
///
/// Case: a shell prompt sets the title on every command, the user runs
/// `reset`, and a program later sets a blank title.
#[test]
fn a_title_is_announced_only_when_it_changes() {
    let mut h = Harness::new();
    let (root, pane) = h.open_root();
    pane.print(b"\x1b]2; vim \x07");
    h.pump_pane(root);
    assert_eq!(titles_of(&h.drain()), vec![(root, Some("vim".to_string()))]);
    assert_eq!(h.backend().pane(root).and_then(|p| p.title()), Some("vim"));

    pane.print(b"\x1b]2;vim\x07");
    h.pump_pane(root);
    assert!(titles_of(&h.drain()).is_empty());

    pane.print(b"\x1bc");
    h.pump_pane(root);
    assert_eq!(titles_of(&h.drain()), vec![(root, None)]);

    pane.print(b"\x1b]2;sh\x07\x1b]2;   \x07");
    h.pump_pane(root);
    assert_eq!(
        titles_of(&h.drain()),
        vec![(root, Some("sh".to_string())), (root, None)]
    );
}

/// Asserts that title signals no longer reach the GUI as raw `Signal`s.
///
/// Case: vim sets the window title and later the user resets the
/// terminal.
#[test]
fn raw_title_signals_are_not_forwarded() {
    let mut h = Harness::new();
    let (root, pane) = h.open_root();
    pane.print(b"\x1b]0;vim\x07\x1bc");
    h.pump_pane(root);
    let events = h.drain();
    assert!(!events.iter().any(|e| matches!(
        e,
        OrzmuxEvent::Signal {
            signal: VtSignal::Title(_) | VtSignal::ResetTitle,
            ..
        }
    )));
}

/// Asserts that the first title a new pane's program sets is announced
/// after the pane opens.
///
/// Case: the shell's startup file sets the title before the first
/// prompt.
#[test]
fn a_new_panes_first_title_follows_its_opening() {
    let mut h = Harness::new();
    h.set_spawn_output(b"\x1b]2;zsh\x07");
    h.resize(GridSize::new(80, 24).expect("a valid size"));
    h.drain();
    h.send(OrzmuxCommand::NewPane {
        request: RequestId(1),
        at: NewPaneAt::Tab,
        cwd: None,
        env: vec![],
    });
    let pane = h
        .backend()
        .panes()
        .map(|(id, _)| id)
        .next()
        .expect("one pane opened");
    h.pump_pane(pane);
    let events = h.drain();
    let opened = events
        .iter()
        .position(|e| matches!(e, OrzmuxEvent::PaneOpened { .. }));
    let titled = events
        .iter()
        .position(|e| matches!(e, OrzmuxEvent::PaneTitle { .. }));
    assert!(
        opened.is_some() && titled.is_some() && opened < titled,
        "{events:?}"
    );
}
