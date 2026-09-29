//! The App state machine: which keys the TUI hands to the page and when it
//! asks for page focus.

use crate::protocol::{PageEvent, RelayedKey};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::mem;

/// A side effect for `main.rs` to perform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Cmd {
    /// Exits the app.
    Quit,
    /// Re-reads the file from disk and pushes new content.
    Reload,
    /// Pops the navigation back stack.
    Back,
    /// Gives the page keyboard focus.
    Focus,
    /// Hands a key the TUI received to the page (`key` emit).
    Relay(RelayedKey),
}

impl From<PageEvent> for Cmd {
    fn from(event: PageEvent) -> Self {
        match event {
            PageEvent::Quit => Self::Quit,
            PageEvent::Reload => Self::Reload,
            PageEvent::Back => Self::Back,
        }
    }
}

/// Whole-app state.
#[derive(Debug, Default)]
pub(crate) struct App {
    ready: bool,
    queued: Vec<RelayedKey>,
}

impl App {
    /// The most keys held for the page before its first `ready`.
    const MAX_QUEUED: usize = 64;

    /// Takes a key press that reached the TUI. Ctrl+C quits at once. Any other
    /// key with a DOM name is relayed and page focus is requested after the
    /// first `ready`; before it, up to 64 keys wait for that `ready`, and later
    /// ones are dropped.
    pub fn on_key(&mut self, key: KeyEvent) -> Vec<Cmd> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return vec![Cmd::Quit];
        }
        let Some(relayed) = RelayedKey::from_key(key) else {
            return vec![];
        };
        if self.ready {
            return vec![Cmd::Relay(relayed), Cmd::Focus];
        }
        if self.queued.len() < Self::MAX_QUEUED {
            self.queued.push(relayed);
        }
        vec![]
    }

    /// Takes a `ready` from the page. The first one relays the waiting keys in
    /// order and then requests page focus; later ones return nothing.
    pub fn on_ready(&mut self) -> Vec<Cmd> {
        if mem::replace(&mut self.ready, true) {
            return vec![];
        }
        let mut cmds: Vec<Cmd> = self.queued.drain(..).map(Cmd::Relay).collect();
        cmds.push(Cmd::Focus);
        cmds
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn ctrl_c() -> KeyEvent {
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
    }

    fn relay(c: char) -> Cmd {
        Cmd::Relay(RelayedKey::from_key(key(c)).expect("a character has a DOM name"))
    }

    /// Asserts that Ctrl+C quits at once, before the page is ready and after.
    ///
    /// Case: the user presses Ctrl+C while the page is still loading, or after
    /// coming back to the pane from another one.
    #[test]
    fn ctrl_c_quits_before_and_after_ready() {
        let mut app = App::default();
        assert_eq!(app.on_key(ctrl_c()), vec![Cmd::Quit]);
        app.on_ready();
        assert_eq!(app.on_key(ctrl_c()), vec![Cmd::Quit]);
    }

    /// Asserts that keys reaching the TUI before the first `ready` wait for it
    /// and are then relayed in order, followed by one focus request.
    ///
    /// Case: the user presses `j` and then `k` while the page is still loading.
    #[test]
    fn keys_before_the_first_ready_are_relayed_in_order_once_it_arrives() {
        let mut app = App::default();
        assert_eq!(app.on_key(key('j')), vec![]);
        assert_eq!(app.on_key(key('k')), vec![]);
        assert_eq!(app.on_ready(), vec![relay('j'), relay('k'), Cmd::Focus]);
    }

    /// Asserts that the first `ready` asks for page focus even when no key is
    /// waiting.
    ///
    /// Case: orzmd starts and the user has not pressed anything yet.
    #[test]
    fn the_first_ready_focuses_the_page() {
        assert_eq!(App::default().on_ready(), vec![Cmd::Focus]);
    }

    /// Asserts that a `ready` after the first one asks for nothing.
    ///
    /// Case: the page reloads while the user works in another pane.
    #[test]
    fn a_later_ready_does_nothing() {
        let mut app = App::default();
        app.on_ready();
        assert_eq!(app.on_ready(), vec![]);
    }

    /// Asserts that a key reaching the TUI after the first `ready` is relayed
    /// and page focus is requested after it.
    ///
    /// Case: the user comes back to the pane from another one with the
    /// keyboard and presses `j`.
    #[test]
    fn a_key_after_ready_is_relayed_and_focuses_the_page() {
        let mut app = App::default();
        app.on_ready();
        assert_eq!(app.on_key(key('j')), vec![relay('j'), Cmd::Focus]);
    }

    /// Asserts that keys past the queue limit are dropped rather than held.
    ///
    /// Case: a stuck key repeats while the page keeps loading.
    #[test]
    fn keys_past_the_queue_limit_are_dropped() {
        let mut app = App::default();
        for _ in 0..App::MAX_QUEUED + 5 {
            app.on_key(key('j'));
        }
        assert_eq!(app.on_ready().len(), App::MAX_QUEUED + 1);
    }

    /// Asserts that a key without a DOM name is neither relayed nor held.
    ///
    /// Case: the user presses F1 after coming back to the pane.
    #[test]
    fn a_key_without_a_dom_name_is_dropped() {
        let mut app = App::default();
        app.on_ready();
        assert_eq!(
            app.on_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE)),
            vec![]
        );
    }

    /// Asserts that each page request maps to its command.
    ///
    /// Case: the user presses `q`, `r`, and Backspace in the page.
    #[test]
    fn page_requests_become_commands() {
        assert_eq!(Cmd::from(PageEvent::Quit), Cmd::Quit);
        assert_eq!(Cmd::from(PageEvent::Reload), Cmd::Reload);
        assert_eq!(Cmd::from(PageEvent::Back), Cmd::Back);
    }
}
