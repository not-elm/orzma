//! App state machine for ozbrowse. `on_action` is the single entry point;
//! it returns the [`Cmd`] side-effects for `main.rs` to execute.

use crate::history::History;
use crate::keymap::{Action, Mode};

/// Scroll direction / magnitude for the webview.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScrollAction {
    /// Scroll down one line.
    Down,
    /// Scroll up one line.
    Up,
    /// Scroll down half a page.
    HalfDown,
    /// Scroll up half a page.
    HalfUp,
    /// Scroll down a full page.
    PageDown,
    /// Scroll up a full page.
    PageUp,
    /// Scroll to the top of the document.
    Top,
    /// Scroll to the bottom of the document.
    Bottom,
}

/// A side-effect for `main.rs` to perform after [`App::on_action`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Cmd {
    /// Scroll the webview.
    Scroll(ScrollAction),
    /// Navigate to the given URL.
    Navigate(String),
    /// Reload the current page.
    Reload,
    /// Exit the app.
    Quit,
}

/// Whole-app state for ozbrowse.
#[derive(Debug)]
pub(crate) struct App {
    mode: Mode,
    pending_prefix: Option<char>,
    url: String,
    address_buf: String,
    history: History,
}

impl App {
    /// Creates a new `App` starting at `initial_url`.
    pub(crate) fn new(initial_url: String) -> Self {
        Self {
            mode: Mode::Normal,
            pending_prefix: None,
            url: initial_url,
            address_buf: String::new(),
            history: History::new(),
        }
    }

    /// The current input mode.
    pub(crate) fn mode(&self) -> Mode {
        self.mode
    }

    /// The URL currently loaded in the webview.
    pub(crate) fn url(&self) -> &str {
        &self.url
    }

    /// The address bar buffer (non-empty only in [`Mode::Address`]).
    pub(crate) fn address_buf(&self) -> &str {
        &self.address_buf
    }

    /// Processes an [`Action`], updating state and returning the side effects to perform.
    pub(crate) fn on_action(&mut self, action: Action) -> Vec<Cmd> {
        if let Some(prefix) = self.pending_prefix.take()
            && let Action::Prefix(c) = action
            && c == prefix
        {
            return self.resolve_chord(c);
        }

        match action {
            Action::Prefix(c) => {
                self.pending_prefix = Some(c);
                vec![]
            }
            Action::Quit => vec![Cmd::Quit],
            Action::Reload => vec![Cmd::Reload],
            Action::ScrollLineDown => vec![Cmd::Scroll(ScrollAction::Down)],
            Action::ScrollLineUp => vec![Cmd::Scroll(ScrollAction::Up)],
            Action::ScrollHalfDown => vec![Cmd::Scroll(ScrollAction::HalfDown)],
            Action::ScrollHalfUp => vec![Cmd::Scroll(ScrollAction::HalfUp)],
            Action::ScrollPageDown => vec![Cmd::Scroll(ScrollAction::PageDown)],
            Action::ScrollPageUp => vec![Cmd::Scroll(ScrollAction::PageUp)],
            Action::GoBottom => vec![Cmd::Scroll(ScrollAction::Bottom)],
            Action::HistoryBack => {
                if let Some(prev) = self.history.back(self.url.clone()) {
                    self.url = prev.clone();
                    vec![Cmd::Navigate(prev)]
                } else {
                    vec![]
                }
            }
            Action::HistoryForward => {
                if let Some(next) = self.history.forward(self.url.clone()) {
                    self.url = next.clone();
                    vec![Cmd::Navigate(next)]
                } else {
                    vec![]
                }
            }
            Action::OpenAddress => {
                self.mode = Mode::Address;
                self.address_buf.clear();
                vec![]
            }
            Action::AddressChar(c) => {
                self.address_buf.push(c);
                vec![]
            }
            Action::AddressBackspace => {
                self.address_buf.pop();
                vec![]
            }
            Action::AddressConfirm => {
                let new_url = self.address_buf.clone();
                self.mode = Mode::Normal;
                self.address_buf.clear();
                if new_url.is_empty() {
                    vec![]
                } else {
                    let old = self.url.clone();
                    self.url = self.history.navigate(old, new_url.clone());
                    vec![Cmd::Navigate(new_url)]
                }
            }
            Action::Escape => {
                self.mode = Mode::Normal;
                self.address_buf.clear();
                vec![]
            }
            Action::EnterInsert => {
                self.mode = Mode::Insert;
                vec![]
            }
            Action::OpenHelp => {
                self.mode = Mode::Help;
                vec![]
            }
            Action::Ignore => vec![],
        }
    }

    fn resolve_chord(&mut self, c: char) -> Vec<Cmd> {
        match c {
            'g' => vec![Cmd::Scroll(ScrollAction::Top)],
            _ => vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        App::new("https://example.com".into())
    }

    #[test]
    fn new_app_starts_in_normal_mode() {
        let a = app();
        assert_eq!(a.mode(), Mode::Normal);
    }

    #[test]
    fn new_app_url_is_initial_url() {
        let a = app();
        assert_eq!(a.url(), "https://example.com");
    }

    #[test]
    fn scroll_actions_produce_scroll_cmds() {
        let mut a = app();
        assert_eq!(a.on_action(Action::ScrollLineDown), vec![Cmd::Scroll(ScrollAction::Down)]);
        assert_eq!(a.on_action(Action::ScrollLineUp), vec![Cmd::Scroll(ScrollAction::Up)]);
        assert_eq!(a.on_action(Action::ScrollHalfDown), vec![Cmd::Scroll(ScrollAction::HalfDown)]);
        assert_eq!(a.on_action(Action::ScrollHalfUp), vec![Cmd::Scroll(ScrollAction::HalfUp)]);
        assert_eq!(a.on_action(Action::ScrollPageDown), vec![Cmd::Scroll(ScrollAction::PageDown)]);
        assert_eq!(a.on_action(Action::ScrollPageUp), vec![Cmd::Scroll(ScrollAction::PageUp)]);
        assert_eq!(a.on_action(Action::GoBottom), vec![Cmd::Scroll(ScrollAction::Bottom)]);
    }

    #[test]
    fn gg_chord_scrolls_to_top() {
        let mut a = app();
        assert_eq!(a.on_action(Action::Prefix('g')), vec![]);
        assert_eq!(a.on_action(Action::Prefix('g')), vec![Cmd::Scroll(ScrollAction::Top)]);
    }

    #[test]
    fn dangling_prefix_then_other_key_clears_and_processes() {
        let mut a = app();
        assert_eq!(a.on_action(Action::Prefix('g')), vec![]);
        assert_eq!(
            a.on_action(Action::ScrollLineDown),
            vec![Cmd::Scroll(ScrollAction::Down)]
        );
    }

    #[test]
    fn open_address_enters_address_mode_with_empty_buf() {
        let mut a = app();
        a.on_action(Action::AddressChar('x'));
        a.on_action(Action::OpenAddress);
        assert_eq!(a.mode(), Mode::Address);
        assert_eq!(a.address_buf(), "");
    }

    #[test]
    fn address_char_and_backspace_edit_buf() {
        let mut a = app();
        a.on_action(Action::OpenAddress);
        a.on_action(Action::AddressChar('h'));
        a.on_action(Action::AddressChar('i'));
        assert_eq!(a.address_buf(), "hi");
        a.on_action(Action::AddressBackspace);
        assert_eq!(a.address_buf(), "h");
    }

    #[test]
    fn address_confirm_navigates_and_returns_to_normal() {
        let mut a = app();
        a.on_action(Action::OpenAddress);
        a.on_action(Action::AddressChar('h'));
        a.on_action(Action::AddressChar('t'));
        a.on_action(Action::AddressChar('t'));
        a.on_action(Action::AddressChar('p'));
        a.on_action(Action::AddressChar('s'));
        a.on_action(Action::AddressChar(':'));
        a.on_action(Action::AddressChar('/'));
        a.on_action(Action::AddressChar('/'));
        a.on_action(Action::AddressChar('n'));
        let cmds = a.on_action(Action::AddressConfirm);
        assert_eq!(cmds, vec![Cmd::Navigate("https://n".into())]);
        assert_eq!(a.mode(), Mode::Normal);
        assert_eq!(a.url(), "https://n");
        assert_eq!(a.address_buf(), "");
    }

    #[test]
    fn address_confirm_with_empty_buf_is_noop() {
        let mut a = app();
        a.on_action(Action::OpenAddress);
        let cmds = a.on_action(Action::AddressConfirm);
        assert_eq!(cmds, vec![]);
        assert_eq!(a.mode(), Mode::Normal);
        assert_eq!(a.url(), "https://example.com");
    }

    #[test]
    fn escape_from_address_mode_returns_to_normal() {
        let mut a = app();
        a.on_action(Action::OpenAddress);
        a.on_action(Action::AddressChar('x'));
        a.on_action(Action::Escape);
        assert_eq!(a.mode(), Mode::Normal);
        assert_eq!(a.address_buf(), "");
    }

    #[test]
    fn history_back_navigates_to_previous_url() {
        let mut a = app();
        a.on_action(Action::OpenAddress);
        for c in "https://b.com".chars() {
            a.on_action(Action::AddressChar(c));
        }
        a.on_action(Action::AddressConfirm);
        let cmds = a.on_action(Action::HistoryBack);
        assert_eq!(cmds, vec![Cmd::Navigate("https://example.com".into())]);
        assert_eq!(a.url(), "https://example.com");
    }

    #[test]
    fn history_back_with_empty_stack_is_noop() {
        let mut a = app();
        let cmds = a.on_action(Action::HistoryBack);
        assert_eq!(cmds, vec![]);
        assert_eq!(a.url(), "https://example.com");
    }

    #[test]
    fn history_forward_after_back_restores_url() {
        let mut a = app();
        a.on_action(Action::OpenAddress);
        for c in "https://b.com".chars() {
            a.on_action(Action::AddressChar(c));
        }
        a.on_action(Action::AddressConfirm);
        a.on_action(Action::HistoryBack);
        let cmds = a.on_action(Action::HistoryForward);
        assert_eq!(cmds, vec![Cmd::Navigate("https://b.com".into())]);
        assert_eq!(a.url(), "https://b.com");
    }

    #[test]
    fn history_forward_with_empty_stack_is_noop() {
        let mut a = app();
        let cmds = a.on_action(Action::HistoryForward);
        assert_eq!(cmds, vec![]);
    }

    #[test]
    fn quit_returns_quit_cmd() {
        let mut a = app();
        assert_eq!(a.on_action(Action::Quit), vec![Cmd::Quit]);
    }

    #[test]
    fn reload_returns_reload_cmd() {
        let mut a = app();
        assert_eq!(a.on_action(Action::Reload), vec![Cmd::Reload]);
    }

    #[test]
    fn enter_insert_switches_mode() {
        let mut a = app();
        a.on_action(Action::EnterInsert);
        assert_eq!(a.mode(), Mode::Insert);
    }

    #[test]
    fn escape_from_insert_returns_to_normal() {
        let mut a = app();
        a.on_action(Action::EnterInsert);
        a.on_action(Action::Escape);
        assert_eq!(a.mode(), Mode::Normal);
    }

    #[test]
    fn open_help_switches_mode_to_help() {
        let mut a = app();
        a.on_action(Action::OpenHelp);
        assert_eq!(a.mode(), Mode::Help);
    }

    #[test]
    fn ignore_produces_no_cmds() {
        let mut a = app();
        assert_eq!(a.on_action(Action::Ignore), vec![]);
    }
}
