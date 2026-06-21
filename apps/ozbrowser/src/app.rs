//! App state machine for ozbrowser. `on_action` is the single entry point;
//! it returns the [`Cmd`] side-effects for `main.rs` to execute.

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
    /// Navigate to the given URL.
    Navigate(String),
    /// Navigate back in history.
    HistoryBack,
    /// Navigate forward in history.
    HistoryForward,
    /// Reload the current page.
    Reload,
    /// Scroll the webview.
    Scroll(ScrollAction),
    /// Show the link-hint overlay on the page.
    HintShow,
    /// Forward a typed hint-label character to the page.
    HintKey(char),
    /// Forward a hint-label backspace to the page.
    HintBackspace,
    /// Tear down the link-hint overlay on the page.
    HintHide,
    /// Exit the app.
    Quit,
}

/// Whole-app state for ozbrowser.
#[derive(Debug)]
pub(crate) struct App {
    mode: Mode,
    pending_prefix: Option<char>,
    url: String,
    address_buf: String,
}

impl App {
    /// Creates a new `App` starting at `initial_url`.
    pub(crate) fn new(initial_url: String) -> Self {
        Self {
            mode: Mode::Normal,
            pending_prefix: None,
            url: initial_url,
            address_buf: String::new(),
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
            Action::HistoryBack => vec![Cmd::HistoryBack],
            Action::HistoryForward => vec![Cmd::HistoryForward],
            Action::OpenAddress => {
                self.address_buf = self.url.clone();
                self.mode = Mode::Address;
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
                let input = self.address_buf.trim();
                let empty = input.is_empty();
                let url = normalize_url(input);
                self.mode = Mode::Normal;
                self.address_buf.clear();
                if empty || url == self.url {
                    vec![]
                } else {
                    vec![Cmd::Navigate(url)]
                }
            }
            Action::Escape => {
                let was_hint = self.mode == Mode::Hint;
                self.mode = Mode::Normal;
                self.address_buf.clear();
                if was_hint {
                    vec![Cmd::HintHide]
                } else {
                    vec![]
                }
            }
            Action::EnterInsert => {
                self.mode = Mode::Insert;
                vec![]
            }
            Action::EnterHint => {
                self.mode = Mode::Hint;
                vec![Cmd::HintShow]
            }
            Action::HintKey(c) => vec![Cmd::HintKey(c)],
            Action::HintBackspace => vec![Cmd::HintBackspace],
            Action::OpenHelp => {
                self.mode = Mode::Help;
                vec![]
            }
            Action::Ignore => vec![],
        }
    }

    /// Records a page-driven URL change reported via `urlChanged` (CEF owns the
    /// session history now, so this only updates the displayed URL).
    pub(crate) fn on_page_url_changed(&mut self, url: String) {
        self.url = url;
    }

    /// Applies a `hintResult` reported by the page: a hint that focused a form
    /// field switches to Insert mode; any other resolution returns to Normal.
    /// A no-op unless currently in Hint mode (guards against a late result
    /// arriving after the user already cancelled with Esc).
    pub(crate) fn on_hint_result(&mut self, kind: &str) {
        if self.mode != Mode::Hint {
            return;
        }
        self.mode = if kind == "focusedInput" {
            Mode::Insert
        } else {
            Mode::Normal
        };
    }

    fn resolve_chord(&mut self, c: char) -> Vec<Cmd> {
        match c {
            'g' => vec![Cmd::Scroll(ScrollAction::Top)],
            _ => vec![],
        }
    }
}

/// Prepends `https://` to a scheme-less address-bar input so a bare hostname
/// (`github.com`) navigates instead of being rejected by the host's URL
/// validation. Input already carrying a `scheme://` is returned unchanged.
fn normalize_url(input: &str) -> String {
    if input.contains("://") {
        input.to_owned()
    } else {
        format!("https://{input}")
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
        assert_eq!(
            a.on_action(Action::ScrollLineDown),
            vec![Cmd::Scroll(ScrollAction::Down)]
        );
        assert_eq!(
            a.on_action(Action::ScrollLineUp),
            vec![Cmd::Scroll(ScrollAction::Up)]
        );
        assert_eq!(
            a.on_action(Action::ScrollHalfDown),
            vec![Cmd::Scroll(ScrollAction::HalfDown)]
        );
        assert_eq!(
            a.on_action(Action::ScrollHalfUp),
            vec![Cmd::Scroll(ScrollAction::HalfUp)]
        );
        assert_eq!(
            a.on_action(Action::ScrollPageDown),
            vec![Cmd::Scroll(ScrollAction::PageDown)]
        );
        assert_eq!(
            a.on_action(Action::ScrollPageUp),
            vec![Cmd::Scroll(ScrollAction::PageUp)]
        );
        assert_eq!(
            a.on_action(Action::GoBottom),
            vec![Cmd::Scroll(ScrollAction::Bottom)]
        );
    }

    #[test]
    fn gg_chord_scrolls_to_top() {
        let mut a = app();
        assert_eq!(a.on_action(Action::Prefix('g')), vec![]);
        assert_eq!(
            a.on_action(Action::Prefix('g')),
            vec![Cmd::Scroll(ScrollAction::Top)]
        );
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
    fn open_address_pre_fills_current_url_and_sets_address_mode() {
        let mut a = app();
        a.on_action(Action::OpenAddress);
        assert_eq!(a.mode(), Mode::Address);
        assert_eq!(a.address_buf(), "https://example.com");
    }

    #[test]
    fn address_char_and_backspace_edit_buf() {
        let mut a = app();
        a.on_action(Action::OpenAddress);
        a.on_action(Action::AddressBackspace);
        assert_eq!(a.address_buf(), "https://example.co");
        a.on_action(Action::AddressChar('x'));
        assert_eq!(a.address_buf(), "https://example.cox");
    }

    #[test]
    fn address_confirm_navigates_and_returns_to_normal() {
        let mut a = app();
        a.on_action(Action::OpenAddress);
        for _ in 0.."https://example.com".len() {
            a.on_action(Action::AddressBackspace);
        }
        for c in "https://n".chars() {
            a.on_action(Action::AddressChar(c));
        }
        let cmds = a.on_action(Action::AddressConfirm);
        assert_eq!(cmds, vec![Cmd::Navigate("https://n".into())]);
        assert_eq!(a.mode(), Mode::Normal);
    }

    #[test]
    fn address_confirm_prepends_https_to_bare_host_and_clears_buffer() {
        let mut a = app();
        a.on_action(Action::OpenAddress);
        for _ in 0.."https://example.com".len() {
            a.on_action(Action::AddressBackspace);
        }
        for c in "github.com".chars() {
            a.on_action(Action::AddressChar(c));
        }
        let cmds = a.on_action(Action::AddressConfirm);
        assert_eq!(cmds, vec![Cmd::Navigate("https://github.com".into())]);
        assert_eq!(a.address_buf(), "", "buffer cleared after confirm");
    }

    #[test]
    fn address_confirm_with_empty_buf_is_noop() {
        let mut a = app();
        a.on_action(Action::OpenAddress);
        for _ in 0.."https://example.com".len() {
            a.on_action(Action::AddressBackspace);
        }
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
    fn history_back_forward_produce_commands() {
        let mut a = app();
        assert_eq!(a.on_action(Action::HistoryBack), vec![Cmd::HistoryBack]);
        assert_eq!(
            a.on_action(Action::HistoryForward),
            vec![Cmd::HistoryForward]
        );
    }

    #[test]
    fn page_url_changed_updates_displayed_url() {
        let mut a = app();
        a.on_page_url_changed("https://docs.rs".into());
        assert_eq!(a.url(), "https://docs.rs");
    }

    #[test]
    fn address_confirm_with_same_url_is_noop() {
        let mut a = app();
        a.on_action(Action::OpenAddress);
        let cmds = a.on_action(Action::AddressConfirm);
        assert_eq!(cmds, vec![]);
        assert_eq!(a.mode(), Mode::Normal);
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

    #[test]
    fn enter_hint_sets_hint_mode_and_emits_show() {
        let mut a = app();
        assert_eq!(a.on_action(Action::EnterHint), vec![Cmd::HintShow]);
        assert_eq!(a.mode(), Mode::Hint);
    }

    #[test]
    fn hint_key_and_backspace_emit_commands_without_mode_change() {
        let mut a = app();
        a.on_action(Action::EnterHint);
        assert_eq!(a.on_action(Action::HintKey('a')), vec![Cmd::HintKey('a')]);
        assert_eq!(a.mode(), Mode::Hint);
        assert_eq!(a.on_action(Action::HintBackspace), vec![Cmd::HintBackspace]);
        assert_eq!(a.mode(), Mode::Hint);
    }

    #[test]
    fn escape_from_hint_mode_hides_and_returns_to_normal() {
        let mut a = app();
        a.on_action(Action::EnterHint);
        assert_eq!(a.on_action(Action::Escape), vec![Cmd::HintHide]);
        assert_eq!(a.mode(), Mode::Normal);
    }

    #[test]
    fn hint_result_focused_input_switches_to_insert() {
        let mut a = app();
        a.on_action(Action::EnterHint);
        a.on_hint_result("focusedInput");
        assert_eq!(a.mode(), Mode::Insert);
    }

    #[test]
    fn hint_result_non_input_kinds_return_to_normal() {
        for kind in ["navigated", "clicked", "empty"] {
            let mut a = app();
            a.on_action(Action::EnterHint);
            a.on_hint_result(kind);
            assert_eq!(
                a.mode(),
                Mode::Normal,
                "kind {kind:?} must return to Normal"
            );
        }
    }

    #[test]
    fn hint_result_is_ignored_when_not_in_hint_mode() {
        let mut a = app();
        a.on_hint_result("focusedInput");
        assert_eq!(a.mode(), Mode::Normal);
    }
}
