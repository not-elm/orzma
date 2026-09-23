//! App state machine for orzbrowser. `on_action` is the single entry point;
//! it returns the [`Cmd`] side-effects for `main.rs` to execute.

use crate::keymap::{Action, KeySet, Mode};
use std::mem;

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
    /// Replace the page's forward keys with the given list.
    SetForwardKeys(KeySet),
    /// Give the page keyboard focus.
    Focus,
    /// Take keyboard focus back from the page to the TUI.
    Blur,
}

/// Whole-app state for orzbrowser.
#[derive(Debug)]
pub(crate) struct App {
    mode: Mode,
    pending_prefix: Option<char>,
    url: String,
    address_buf: String,
    page_focused: bool,
    refocus_after_text_mode: bool,
    key_set: KeySet,
}

impl App {
    /// Creates a new `App` starting at `initial_url`.
    pub(crate) fn new(initial_url: String) -> Self {
        Self {
            mode: Mode::Normal,
            pending_prefix: None,
            url: initial_url,
            address_buf: String::new(),
            page_focused: false,
            refocus_after_text_mode: false,
            key_set: KeySet::Normal,
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

        let cmds = match action {
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
                vec![self.enter_text_mode(Mode::Address)]
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
                let mut cmds = if empty || url == self.url {
                    vec![]
                } else {
                    vec![Cmd::Navigate(url)]
                };
                cmds.extend(self.take_refocus());
                cmds
            }
            Action::Escape => {
                let left = self.mode;
                self.mode = Mode::Normal;
                self.address_buf.clear();
                match left {
                    Mode::Hint => {
                        let mut cmds = vec![Cmd::HintHide];
                        cmds.extend(self.take_refocus());
                        cmds
                    }
                    Mode::Insert => vec![Cmd::Blur],
                    Mode::Address | Mode::Help => self.take_refocus().into_iter().collect(),
                    _ => vec![],
                }
            }
            Action::EnterInsert => {
                self.mode = Mode::Insert;
                vec![Cmd::Focus]
            }
            Action::EnterHint => vec![Cmd::HintShow, self.enter_text_mode(Mode::Hint)],
            Action::HintKey(c) => vec![Cmd::HintKey(c)],
            Action::HintBackspace => vec![Cmd::HintBackspace],
            Action::OpenHelp => vec![self.enter_text_mode(Mode::Help)],
            Action::Ignore => vec![],
        };
        self.with_key_set(cmds)
    }

    /// Records a page-driven URL change reported via `urlChanged` (CEF owns the
    /// session history now, so this only updates the displayed URL).
    pub(crate) fn on_page_url_changed(&mut self, url: String) {
        self.url = url;
    }

    /// Applies a `hintResult` reported by the page: a hint that focused a form
    /// field switches to Insert mode, carrying the insert keys and page focus;
    /// any other resolution returns to Normal, refocusing the page if it held
    /// focus when Hint mode began. A no-op unless currently in Hint mode
    /// (guards against a late result arriving after the user already
    /// cancelled with Esc).
    pub(crate) fn on_hint_result(&mut self, kind: &str) -> Vec<Cmd> {
        if self.mode != Mode::Hint {
            return vec![];
        }
        let cmds = if kind == "focusedInput" {
            self.mode = Mode::Insert;
            self.refocus_after_text_mode = false;
            vec![Cmd::Focus]
        } else {
            self.mode = Mode::Normal;
            self.take_refocus().into_iter().collect()
        };
        self.with_key_set(cmds)
    }

    // TODO: a click that focuses a text input on the page leaves the app in
    // Normal mode, so the forwarded Normal keys (`j`, `k`, …) scroll instead
    // of typing. Entering Insert automatically needs the page to report,
    // via a preload script, that an editable element took focus.
    /// Records the page's focus state and applies the host's report: a page
    /// that gains focus while Address, Hint or Help is open cancels that
    /// text mode, and one that loses focus in Insert mode returns to Normal
    /// with the Normal keys.
    pub(crate) fn on_focus_change(&mut self, focused: bool) -> Vec<Cmd> {
        self.page_focused = focused;
        let cmds = match (self.mode, focused) {
            (Mode::Address | Mode::Help, true) => {
                self.mode = Mode::Normal;
                self.address_buf.clear();
                self.refocus_after_text_mode = false;
                vec![]
            }
            (Mode::Hint, true) => {
                self.mode = Mode::Normal;
                self.refocus_after_text_mode = false;
                vec![Cmd::HintHide]
            }
            (Mode::Insert, false) => {
                self.mode = Mode::Normal;
                vec![]
            }
            _ => vec![],
        };
        self.with_key_set(cmds)
    }

    fn resolve_chord(&mut self, c: char) -> Vec<Cmd> {
        match c {
            'g' => vec![Cmd::Scroll(ScrollAction::Top)],
            _ => vec![],
        }
    }

    /// Enters the TUI text mode `mode`, remembering whether the page held
    /// focus so that leaving it can give focus back, and returns the `Blur`
    /// that takes the keyboard from the page.
    fn enter_text_mode(&mut self, mode: Mode) -> Cmd {
        self.mode = mode;
        self.refocus_after_text_mode = self.page_focused;
        Cmd::Blur
    }

    /// `Focus` when the page held focus when the text mode began, clearing
    /// the flag.
    fn take_refocus(&mut self) -> Option<Cmd> {
        mem::take(&mut self.refocus_after_text_mode).then_some(Cmd::Focus)
    }

    /// Puts `SetForwardKeys` first in `cmds` when the current mode needs a
    /// different forward-key set than the page carries.
    fn with_key_set(&mut self, mut cmds: Vec<Cmd>) -> Vec<Cmd> {
        let wanted = KeySet::of(self.mode);
        if wanted != self.key_set {
            self.key_set = wanted;
            cmds.insert(0, Cmd::SetForwardKeys(wanted));
        }
        cmds
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
        assert_eq!(
            a.on_action(Action::EnterHint),
            vec![Cmd::HintShow, Cmd::Blur]
        );
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

    /// Asserts that entering and leaving insert mode switches the forward
    /// keys and hands the keyboard to the page and back.
    ///
    /// Case: the user presses `i` to type into a search box, then Esc.
    #[test]
    fn insert_mode_switches_forward_keys_and_focus() {
        let mut a = app();
        assert_eq!(
            a.on_action(Action::EnterInsert),
            vec![Cmd::SetForwardKeys(KeySet::Insert), Cmd::Focus]
        );
        assert_eq!(
            a.on_action(Action::Escape),
            vec![Cmd::SetForwardKeys(KeySet::Normal), Cmd::Blur]
        );
        assert_eq!(a.mode(), Mode::Normal);
    }

    /// Asserts that the TUI's text modes take the keyboard back from the
    /// page.
    ///
    /// Case: after clicking the page, the user opens the address bar, the
    /// link hints, and the help.
    #[test]
    fn text_modes_blur_the_page() {
        let mut a = app();
        assert_eq!(a.on_action(Action::OpenAddress), vec![Cmd::Blur]);
        a.on_action(Action::Escape);
        assert_eq!(
            a.on_action(Action::EnterHint),
            vec![Cmd::HintShow, Cmd::Blur]
        );
        a.on_action(Action::Escape);
        assert_eq!(a.on_action(Action::OpenHelp), vec![Cmd::Blur]);
    }

    /// Asserts that a hint that focused a form field enters insert mode with
    /// the insert keys and page focus.
    ///
    /// Case: the user follows a link hint onto a text input.
    #[test]
    fn a_focused_input_hint_enters_insert_mode() {
        let mut a = app();
        a.on_action(Action::EnterHint);
        assert_eq!(
            a.on_hint_result("focusedInput"),
            vec![Cmd::SetForwardKeys(KeySet::Insert), Cmd::Focus]
        );
        assert_eq!(a.mode(), Mode::Insert);
    }

    /// Asserts that a `focusedInput` result arriving after the user left hint
    /// mode does nothing.
    ///
    /// Case: the user presses Esc just before the page reports the hint it
    /// resolved.
    #[test]
    fn a_late_focused_input_result_after_escape_does_nothing() {
        let mut a = app();
        a.on_action(Action::EnterHint);
        a.on_action(Action::Escape);
        assert_eq!(a.on_hint_result("focusedInput"), vec![]);
        assert_eq!(a.mode(), Mode::Normal);
    }

    /// Asserts that a click on the page cancels the TUI's text modes, and
    /// that losing focus in insert mode returns to normal mode.
    ///
    /// Case: the user clicks the page while typing an address or picking a
    /// hint, and later presses the release-focus shortcut while typing into
    /// a form.
    #[test]
    fn focus_changes_cancel_text_modes_and_leave_insert() {
        let mut a = app();
        a.on_action(Action::OpenAddress);
        assert_eq!(a.on_focus_change(true), vec![]);
        assert_eq!(a.mode(), Mode::Normal);
        assert_eq!(a.address_buf(), "");

        a.on_action(Action::EnterHint);
        assert_eq!(a.on_focus_change(true), vec![Cmd::HintHide]);
        assert_eq!(a.mode(), Mode::Normal);

        a.on_action(Action::EnterInsert);
        assert_eq!(a.on_focus_change(true), vec![], "the echo of our own focus");
        assert_eq!(
            a.on_focus_change(false),
            vec![Cmd::SetForwardKeys(KeySet::Normal)]
        );
        assert_eq!(a.mode(), Mode::Normal);

        assert_eq!(a.on_focus_change(true), vec![], "a click in normal mode");
        assert_eq!(a.mode(), Mode::Normal);
    }

    /// Asserts that leaving Address, Hint or Help refocuses the page when it
    /// held focus before the mode was entered.
    ///
    /// Case: the user clicks the page, then opens the address bar, follows a
    /// link hint to a non-input target, or opens help, and exits each with
    /// the mode's own way out.
    #[test]
    fn text_mode_exit_refocuses_a_previously_focused_page() {
        let mut a = app();
        a.on_focus_change(true);
        a.on_action(Action::OpenAddress);
        assert_eq!(a.on_action(Action::Escape), vec![Cmd::Focus]);

        let mut a = app();
        a.on_focus_change(true);
        a.on_action(Action::OpenAddress);
        a.on_action(Action::AddressChar('x'));
        assert_eq!(
            a.on_action(Action::AddressConfirm),
            vec![Cmd::Navigate("https://example.comx".into()), Cmd::Focus]
        );

        let mut a = app();
        a.on_focus_change(true);
        a.on_action(Action::EnterHint);
        assert_eq!(a.on_action(Action::Escape), vec![Cmd::HintHide, Cmd::Focus]);

        let mut a = app();
        a.on_focus_change(true);
        a.on_action(Action::EnterHint);
        assert_eq!(a.on_hint_result("clicked"), vec![Cmd::Focus]);

        let mut a = app();
        a.on_focus_change(true);
        a.on_action(Action::OpenHelp);
        assert_eq!(a.on_action(Action::Escape), vec![Cmd::Focus]);
    }

    /// Asserts that leaving Address, Hint or Help does not refocus the page
    /// when it did not hold focus before the mode was entered.
    ///
    /// Case: the user opens the address bar, follows a link hint to a
    /// non-input target, or opens help without first clicking the page, and
    /// exits each the same way.
    #[test]
    fn text_mode_exit_does_not_refocus_an_unfocused_page() {
        let mut a = app();
        a.on_action(Action::OpenAddress);
        assert_eq!(a.on_action(Action::Escape), vec![]);

        let mut a = app();
        a.on_action(Action::OpenAddress);
        a.on_action(Action::AddressChar('x'));
        assert_eq!(
            a.on_action(Action::AddressConfirm),
            vec![Cmd::Navigate("https://example.comx".into())]
        );

        let mut a = app();
        a.on_action(Action::EnterHint);
        assert_eq!(a.on_action(Action::Escape), vec![Cmd::HintHide]);

        let mut a = app();
        a.on_action(Action::EnterHint);
        assert_eq!(a.on_hint_result("clicked"), vec![]);

        let mut a = app();
        a.on_action(Action::OpenHelp);
        assert_eq!(a.on_action(Action::Escape), vec![]);
    }

    /// Asserts that a hint landing on a form field emits exactly one
    /// `Focus`, even when the page held focus before Hint mode began, and
    /// that leaving the resulting Insert mode with Esc emits no extra
    /// `Focus`.
    ///
    /// Case: the user clicks the page, follows a link hint onto a text
    /// input, types into it, then presses Esc.
    #[test]
    fn focused_input_hint_emits_one_focus_and_leaves_no_pending_refocus() {
        let mut a = app();
        a.on_focus_change(true);
        a.on_action(Action::EnterHint);
        assert_eq!(
            a.on_hint_result("focusedInput"),
            vec![Cmd::SetForwardKeys(KeySet::Insert), Cmd::Focus]
        );
        assert_eq!(
            a.on_action(Action::Escape),
            vec![Cmd::SetForwardKeys(KeySet::Normal), Cmd::Blur]
        );
    }

    /// Asserts that a focus notification arriving while a text mode is open
    /// clears the pending refocus, so a later text-mode round that starts
    /// from an unfocused page emits no `Focus`.
    ///
    /// Case: the user clicks the page and opens the address bar, but the
    /// page reports focus again before the address bar is closed; the user
    /// then clicks away, reopens the address bar, and exits it.
    #[test]
    fn focus_change_during_a_text_mode_clears_the_pending_refocus() {
        let mut a = app();
        a.on_focus_change(true);
        a.on_action(Action::OpenAddress);
        assert_eq!(a.on_focus_change(true), vec![]);

        a.on_focus_change(false);
        a.on_action(Action::OpenAddress);
        assert_eq!(a.on_action(Action::Escape), vec![]);
    }

    /// Asserts that a focus notification while Help is open returns to
    /// Normal with no commands.
    ///
    /// Case: the user opens help, then clicks the page.
    #[test]
    fn focus_change_true_leaves_help_mode_quietly() {
        let mut a = app();
        a.on_action(Action::OpenHelp);
        assert_eq!(a.on_focus_change(true), vec![]);
        assert_eq!(a.mode(), Mode::Normal);
    }
}
