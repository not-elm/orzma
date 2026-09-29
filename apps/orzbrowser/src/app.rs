//! App state machine for orzbrowser: each entry point updates the state and
//! returns the side effects to perform.

use crate::address::AddressTarget;
use crate::focus::{FocusDrain, Target};
use crate::keymap::{Action, KeySet, Mode};
use crate::protocol::PageEvent;
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

/// A side effect an [`App`] entry point asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Cmd {
    /// Navigate the page to the given URL.
    Navigate(String),
    /// Navigate the page back in history.
    HistoryBack,
    /// Navigate the page forward in history.
    HistoryForward,
    /// Reload the page.
    Reload,
    /// Scroll the page.
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
    /// Replace the forward keys of the given webview with the given set.
    SetForwardKeys(Target, KeySet),
    /// Give the given webview keyboard focus.
    Focus(Target),
    /// Take keyboard focus back from either webview to the TUI.
    Blur,
}

/// Whole-app state for orzbrowser.
#[derive(Debug)]
pub(crate) struct App {
    mode: Mode,
    pending_prefix: Option<char>,
    url: String,
    seed: String,
    address_epoch: u32,
    holder: Option<Target>,
    awaiting: Option<Target>,
    refocus_page: bool,
    page_placed: bool,
    chrome_ready: bool,
    page_keys: KeySet,
    chrome_keys: KeySet,
}

impl App {
    /// Creates an app that shows `url` in Normal mode.
    pub fn new(url: String) -> Self {
        Self {
            mode: Mode::Normal,
            pending_prefix: None,
            url,
            seed: String::new(),
            address_epoch: 0,
            holder: None,
            awaiting: None,
            refocus_page: false,
            page_placed: true,
            chrome_ready: false,
            page_keys: KeySet::Normal,
            chrome_keys: KeySet::Normal,
        }
    }

    /// Creates an app that shows `url` with an empty address bar open.
    ///
    /// The chrome takes keyboard focus once its page reports ready.
    pub fn with_address_open(url: String) -> Self {
        Self {
            mode: Mode::Address,
            address_epoch: 1,
            awaiting: Some(Target::Chrome),
            chrome_keys: KeySet::Empty,
            ..Self::new(url)
        }
    }

    /// The current input mode.
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// The URL loaded in the page.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The first key of a pending two-key chord.
    pub fn pending_key(&self) -> Option<char> {
        self.pending_prefix
    }

    /// The text the address input starts with when the address bar opens.
    pub fn seed(&self) -> &str {
        &self.seed
    }

    /// How many times the address bar has opened.
    pub fn address_epoch(&self) -> u32 {
        self.address_epoch
    }

    /// The forward keys the page carries.
    pub fn page_keys(&self) -> KeySet {
        self.page_keys
    }

    /// The forward keys the chrome carries.
    pub fn chrome_keys(&self) -> KeySet {
        self.chrome_keys
    }

    /// Records whether the page has any rows on screen.
    ///
    /// While it has none, Insert, Hint, and Help cannot be entered, and
    /// leaving a text mode gives the keyboard to the TUI instead of the page.
    pub fn set_page_placed(&mut self, placed: bool) {
        self.page_placed = placed;
    }

    /// Processes an [`Action`] and returns the side effects to perform.
    pub fn on_action(&mut self, action: Action) -> Vec<Cmd> {
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
            Action::OpenAddress => self.open_address(),
            Action::RefocusChrome => self.refocus_chrome(),
            Action::Escape => self.escape(),
            Action::EnterInsert => self.enter_insert(),
            Action::EnterHint => self.enter_hint(),
            Action::HintKey(c) => vec![Cmd::HintKey(c)],
            Action::HintBackspace => vec![Cmd::HintBackspace],
            Action::OpenHelp => self.open_help(),
            Action::Ignore => vec![],
        };
        self.with_key_sets(cmds)
    }

    // TODO: a click that focuses a text input on the page leaves the app in
    // Normal mode, so the forwarded Normal keys (`j`, `k`, …) scroll instead
    // of typing. Entering Insert automatically needs the page to report,
    // via a preload script, that an editable element took focus.
    /// Applies the focus changes both webviews reported in one loop pass.
    ///
    /// While a focus request of this app is unanswered, no mode is cancelled.
    /// Otherwise a chrome that lost focus closes the address bar, a page that
    /// lost focus leaves Insert mode, and a gain on either webview cancels
    /// Hint and Help.
    pub fn on_focus_drain(&mut self, drain: FocusDrain) -> Vec<Cmd> {
        if drain.is_empty() {
            return vec![];
        }
        self.holder = drain.holder_after(self.holder);
        if self
            .awaiting
            .is_some_and(|target| drain.of(target).saw_true)
        {
            self.awaiting = None;
        }
        if self.awaiting.is_some() {
            return vec![];
        }
        let cmds = match self.mode {
            Mode::Address if self.holder != Some(Target::Chrome) => {
                self.mode = Mode::Normal;
                vec![]
            }
            Mode::Insert if self.holder != Some(Target::Page) => {
                self.mode = Mode::Normal;
                vec![]
            }
            Mode::Hint if drain.saw_any_true() => {
                self.mode = Mode::Normal;
                vec![Cmd::HintHide]
            }
            Mode::Help if drain.saw_any_true() => {
                self.mode = Mode::Normal;
                vec![]
            }
            _ => vec![],
        };
        self.with_key_sets(cmds)
    }

    /// Applies the address the chrome page submitted.
    ///
    /// An address to open or search navigates the page, or reloads it when it
    /// is the current URL; an empty one only closes the address bar. Ignored
    /// outside the address bar.
    pub fn on_address_target(&mut self, target: AddressTarget) -> Vec<Cmd> {
        if self.mode != Mode::Address {
            return vec![];
        }
        let navigate = match target {
            AddressTarget::Open(url) | AddressTarget::Search(url) => Some(if url == self.url {
                Cmd::Reload
            } else {
                Cmd::Navigate(url)
            }),
            AddressTarget::Empty => None,
            AddressTarget::Invalid(_) => return vec![],
        };
        let cmds = self.leave_address(navigate);
        self.with_key_sets(cmds)
    }

    /// Applies a report from the chrome page: the first ready lets an open
    /// address bar take keyboard focus, Esc closes the address bar, and a
    /// click on the omnibox in Normal mode opens it.
    pub fn on_page_event(&mut self, event: PageEvent) -> Vec<Cmd> {
        let cmds = match event {
            PageEvent::Ready => self.chrome_ready(),
            PageEvent::Cancel if self.mode == Mode::Address => self.leave_address(None),
            PageEvent::OpenAddress if self.mode == Mode::Normal => self.open_address(),
            PageEvent::Cancel | PageEvent::OpenAddress => vec![],
        };
        self.with_key_sets(cmds)
    }

    /// Applies a `hintResult` reported by the page: a hint that focused a
    /// form field switches to Insert mode with the page focused; any other
    /// result returns to Normal, refocusing the page if it held focus when
    /// Hint mode began. A no-op outside Hint mode.
    pub fn on_hint_result(&mut self, kind: &str) -> Vec<Cmd> {
        if self.mode != Mode::Hint {
            return vec![];
        }
        let cmds = if kind == "focusedInput" {
            self.mode = Mode::Insert;
            vec![self.request_focus(Target::Page)]
        } else {
            self.mode = Mode::Normal;
            self.take_refocus().into_iter().collect()
        };
        self.with_key_sets(cmds)
    }

    /// Records a page-driven URL change reported via `urlChanged`.
    pub fn on_page_url_changed(&mut self, url: String) {
        self.url = url;
    }

    fn chrome_ready(&mut self) -> Vec<Cmd> {
        let first = !mem::replace(&mut self.chrome_ready, true);
        if first && self.mode == Mode::Address {
            vec![self.request_focus(Target::Chrome)]
        } else {
            vec![]
        }
    }

    fn resolve_chord(&mut self, c: char) -> Vec<Cmd> {
        match c {
            'g' => vec![Cmd::Scroll(ScrollAction::Top)],
            _ => vec![],
        }
    }

    fn open_address(&mut self) -> Vec<Cmd> {
        self.mode = Mode::Address;
        self.refocus_page = self.holder == Some(Target::Page);
        self.seed = self.url.clone();
        self.address_epoch = self.address_epoch.wrapping_add(1);
        if self.chrome_ready {
            vec![self.request_focus(Target::Chrome)]
        } else {
            self.awaiting = Some(Target::Chrome);
            vec![]
        }
    }

    fn leave_address(&mut self, navigate: Option<Cmd>) -> Vec<Cmd> {
        self.mode = Mode::Normal;
        let focus = self.take_refocus().unwrap_or_else(|| self.blur());
        navigate.into_iter().chain([focus]).collect()
    }

    fn refocus_chrome(&mut self) -> Vec<Cmd> {
        if self.mode != Mode::Address || !self.chrome_ready {
            return vec![];
        }
        vec![self.request_focus(Target::Chrome)]
    }

    fn escape(&mut self) -> Vec<Cmd> {
        match self.mode {
            Mode::Address => self.leave_address(None),
            Mode::Insert => {
                self.mode = Mode::Normal;
                vec![self.blur()]
            }
            Mode::Hint => {
                self.mode = Mode::Normal;
                let mut cmds = vec![Cmd::HintHide];
                cmds.extend(self.take_refocus());
                cmds
            }
            Mode::Help => {
                self.mode = Mode::Normal;
                self.take_refocus().into_iter().collect()
            }
            Mode::Normal => vec![],
        }
    }

    fn enter_insert(&mut self) -> Vec<Cmd> {
        if !self.page_placed {
            return vec![];
        }
        self.mode = Mode::Insert;
        vec![self.request_focus(Target::Page)]
    }

    fn enter_hint(&mut self) -> Vec<Cmd> {
        if !self.page_placed {
            return vec![];
        }
        vec![Cmd::HintShow, self.enter_text_mode(Mode::Hint)]
    }

    fn open_help(&mut self) -> Vec<Cmd> {
        if !self.page_placed {
            return vec![];
        }
        vec![self.enter_text_mode(Mode::Help)]
    }

    /// Enters Hint or Help, remembering whether the page held focus, and
    /// returns the `Blur` that takes the keyboard from the webviews.
    fn enter_text_mode(&mut self, mode: Mode) -> Cmd {
        self.mode = mode;
        self.refocus_page = self.holder == Some(Target::Page);
        self.blur()
    }

    /// The command that refocuses the page when it held focus when the current
    /// mode began and still has rows, clearing the flag.
    fn take_refocus(&mut self) -> Option<Cmd> {
        let refocus = mem::take(&mut self.refocus_page) && self.page_placed;
        refocus.then(|| self.request_focus(Target::Page))
    }

    /// Returns the command that focuses `target`, waiting for its gain unless
    /// it already holds focus.
    fn request_focus(&mut self, target: Target) -> Cmd {
        self.awaiting = (self.holder != Some(target)).then_some(target);
        if target == Target::Page {
            self.holder = Some(Target::Page);
        }
        Cmd::Focus(target)
    }

    fn blur(&mut self) -> Cmd {
        self.holder = None;
        self.awaiting = None;
        Cmd::Blur
    }

    /// Puts a `SetForwardKeys` first in `cmds` for each webview whose key set
    /// the current mode changes.
    fn with_key_sets(&mut self, mut cmds: Vec<Cmd>) -> Vec<Cmd> {
        let chrome = KeySet::for_chrome(self.mode);
        if chrome != self.chrome_keys {
            self.chrome_keys = chrome;
            cmds.insert(0, Cmd::SetForwardKeys(Target::Chrome, chrome));
        }
        let page = KeySet::for_page(self.mode);
        if page != self.page_keys {
            self.page_keys = page;
            cmds.insert(0, Cmd::SetForwardKeys(Target::Page, page));
        }
        cmds
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::address::InvalidAddress;
    use crate::focus::HandleFocus;

    const URL: &str = "https://example.com/";

    fn app() -> App {
        App::new(URL.to_owned())
    }

    /// An app whose chrome page has reported ready.
    fn ready_app() -> App {
        let mut a = app();
        a.on_page_event(PageEvent::Ready);
        a
    }

    fn drain(page: &[bool], chrome: &[bool]) -> FocusDrain {
        FocusDrain {
            page: HandleFocus::fold(page.iter().copied()),
            chrome: HandleFocus::fold(chrome.iter().copied()),
        }
    }

    fn page_keys(set: KeySet) -> Cmd {
        Cmd::SetForwardKeys(Target::Page, set)
    }

    fn chrome_keys(set: KeySet) -> Cmd {
        Cmd::SetForwardKeys(Target::Chrome, set)
    }

    /// Asserts that the scroll actions map to their scroll commands.
    ///
    /// Case: the user scrolls the page with the vim keys.
    #[test]
    fn scroll_actions_produce_scroll_cmds() {
        let mut a = app();
        for (action, scroll) in [
            (Action::ScrollLineDown, ScrollAction::Down),
            (Action::ScrollLineUp, ScrollAction::Up),
            (Action::ScrollHalfDown, ScrollAction::HalfDown),
            (Action::ScrollHalfUp, ScrollAction::HalfUp),
            (Action::ScrollPageDown, ScrollAction::PageDown),
            (Action::ScrollPageUp, ScrollAction::PageUp),
            (Action::GoBottom, ScrollAction::Bottom),
        ] {
            assert_eq!(a.on_action(action), vec![Cmd::Scroll(scroll)]);
        }
    }

    /// Asserts that `gg` scrolls to the top, the pending `g` is reported, and
    /// another key drops it.
    ///
    /// Case: the user presses `g` twice, then presses `g` and changes their
    /// mind.
    #[test]
    fn gg_scrolls_to_the_top_and_shows_the_pending_key() {
        let mut a = app();
        assert_eq!(a.on_action(Action::Prefix('g')), vec![]);
        assert_eq!(a.pending_key(), Some('g'));
        assert_eq!(
            a.on_action(Action::Prefix('g')),
            vec![Cmd::Scroll(ScrollAction::Top)]
        );
        a.on_action(Action::Prefix('g'));
        assert_eq!(
            a.on_action(Action::ScrollLineDown),
            vec![Cmd::Scroll(ScrollAction::Down)]
        );
        assert_eq!(a.pending_key(), None);
    }

    /// Asserts that history, reload, and quit produce their commands.
    ///
    /// Case: the user presses `H`, `L`, `r`, and `q`.
    #[test]
    fn history_reload_and_quit_produce_commands() {
        let mut a = app();
        assert_eq!(a.on_action(Action::HistoryBack), vec![Cmd::HistoryBack]);
        assert_eq!(
            a.on_action(Action::HistoryForward),
            vec![Cmd::HistoryForward]
        );
        assert_eq!(a.on_action(Action::Reload), vec![Cmd::Reload]);
        assert_eq!(a.on_action(Action::Quit), vec![Cmd::Quit]);
    }

    /// Asserts that opening the address bar empties the chrome's forward keys,
    /// focuses the chrome, and seeds the input with the current URL.
    ///
    /// Case: the user presses `o` to type a new address.
    #[test]
    fn opening_the_address_bar_focuses_the_chrome() {
        let mut a = ready_app();
        assert_eq!(
            a.on_action(Action::OpenAddress),
            vec![chrome_keys(KeySet::Empty), Cmd::Focus(Target::Chrome)]
        );
        assert_eq!(a.mode(), Mode::Address);
        assert_eq!(a.seed(), URL);
        assert_eq!(a.address_epoch(), 1);
    }

    /// Asserts that the address bar waits for the chrome page before focusing
    /// it, and focuses it only on the first ready.
    ///
    /// Case: the user presses `o` right after launch, before the chrome page
    /// has loaded; later the chrome page reloads.
    #[test]
    fn the_address_bar_waits_for_the_chrome_page() {
        let mut a = app();
        assert_eq!(
            a.on_action(Action::OpenAddress),
            vec![chrome_keys(KeySet::Empty)]
        );
        assert_eq!(
            a.on_page_event(PageEvent::Ready),
            vec![Cmd::Focus(Target::Chrome)]
        );
        assert_eq!(a.on_page_event(PageEvent::Ready), vec![]);
    }

    /// Asserts that launching without an address opens an empty address bar
    /// that the chrome takes once it is ready.
    ///
    /// Case: the user runs `orzbrowser` with no argument.
    #[test]
    fn launching_without_an_address_opens_the_address_bar() {
        let mut a = App::with_address_open("https://duckduckgo.com/".to_owned());
        assert_eq!(a.mode(), Mode::Address);
        assert_eq!(a.chrome_keys(), KeySet::Empty);
        assert_eq!(a.seed(), "");
        assert_eq!(a.address_epoch(), 1);
        assert_eq!(a.on_focus_drain(drain(&[], &[])), vec![]);
        assert_eq!(
            a.on_page_event(PageEvent::Ready),
            vec![Cmd::Focus(Target::Chrome)]
        );
        assert_eq!(a.on_focus_drain(drain(&[], &[true])), vec![]);
        assert_eq!(a.mode(), Mode::Address);
    }

    /// Asserts that submitting an address or a search navigates, returns the
    /// chrome keys, and gives the keyboard back to the TUI.
    ///
    /// Case: the user types a domain, and later a phrase, and presses Enter.
    #[test]
    fn submitting_navigates_and_returns_the_keyboard() {
        let mut a = ready_app();
        a.on_action(Action::OpenAddress);
        a.on_focus_drain(drain(&[], &[true]));
        assert_eq!(
            a.on_address_target(AddressTarget::Open("https://docs.rs/".to_owned())),
            vec![
                chrome_keys(KeySet::Normal),
                Cmd::Navigate("https://docs.rs/".to_owned()),
                Cmd::Blur
            ]
        );
        assert_eq!(a.mode(), Mode::Normal);

        a.on_action(Action::OpenAddress);
        let search = "https://duckduckgo.com/?q=rust%20async".to_owned();
        assert_eq!(
            a.on_address_target(AddressTarget::Search(search.clone())),
            vec![
                chrome_keys(KeySet::Normal),
                Cmd::Navigate(search),
                Cmd::Blur
            ]
        );
    }

    /// Asserts that submitting the current URL reloads, and an empty submit
    /// only closes the address bar.
    ///
    /// Case: the user presses Enter on the prefilled URL, and later clears the
    /// input and presses Enter.
    #[test]
    fn the_current_url_reloads_and_an_empty_submit_closes() {
        let mut a = ready_app();
        a.on_action(Action::OpenAddress);
        assert_eq!(
            a.on_address_target(AddressTarget::Open(URL.to_owned())),
            vec![chrome_keys(KeySet::Normal), Cmd::Reload, Cmd::Blur]
        );
        a.on_action(Action::OpenAddress);
        assert_eq!(
            a.on_address_target(AddressTarget::Empty),
            vec![chrome_keys(KeySet::Normal), Cmd::Blur]
        );
    }

    /// Asserts that an invalid target, and any target after the address bar
    /// closed, is ignored.
    ///
    /// Case: an invalid submit reaches the loop, and a submit arrives just
    /// after the user pressed Esc.
    #[test]
    fn stray_targets_are_ignored() {
        let mut a = ready_app();
        a.on_action(Action::OpenAddress);
        assert_eq!(
            a.on_address_target(AddressTarget::Invalid(InvalidAddress::Malformed)),
            vec![]
        );
        assert_eq!(a.mode(), Mode::Address);
        a.on_page_event(PageEvent::Cancel);
        assert_eq!(
            a.on_address_target(AddressTarget::Open("https://docs.rs/".to_owned())),
            vec![]
        );
    }

    /// Asserts that closing the address bar gives the keyboard back to a page
    /// that held it when the bar opened.
    ///
    /// Case: the user clicks the page, presses `o`, and presses Esc.
    #[test]
    fn closing_the_address_bar_refocuses_a_page_that_held_focus() {
        let mut a = ready_app();
        a.on_focus_drain(drain(&[true], &[]));
        assert_eq!(
            a.on_action(Action::OpenAddress),
            vec![chrome_keys(KeySet::Empty), Cmd::Focus(Target::Chrome)]
        );
        assert_eq!(a.on_focus_drain(drain(&[false], &[true])), vec![]);
        assert_eq!(
            a.on_page_event(PageEvent::Cancel),
            vec![chrome_keys(KeySet::Normal), Cmd::Focus(Target::Page)]
        );
    }

    /// Asserts that closing the address bar blurs instead of refocusing a
    /// page that has no rows.
    ///
    /// Case: the user clicks the page, presses `o`, shrinks the pane to two
    /// rows, and presses Esc.
    #[test]
    fn closing_the_address_bar_blurs_when_the_page_has_no_rows() {
        let mut a = ready_app();
        a.on_focus_drain(drain(&[true], &[]));
        a.on_action(Action::OpenAddress);
        a.on_focus_drain(drain(&[false], &[true]));
        a.set_page_placed(false);
        assert_eq!(
            a.on_page_event(PageEvent::Cancel),
            vec![chrome_keys(KeySet::Normal), Cmd::Blur]
        );
    }

    /// Asserts that a stale chrome blur, arriving after the address bar
    /// reopened, does not close it, while a later click on the page does.
    ///
    /// Case: the user presses `o`, Esc, and `o` in quick succession, then
    /// clicks the page.
    #[test]
    fn a_stale_chrome_blur_does_not_close_a_reopened_address_bar() {
        let mut a = ready_app();
        a.on_action(Action::OpenAddress);
        a.on_focus_drain(drain(&[], &[true]));
        a.on_page_event(PageEvent::Cancel);
        a.on_action(Action::OpenAddress);
        assert_eq!(a.address_epoch(), 2);
        assert_eq!(a.on_focus_drain(drain(&[], &[false])), vec![]);
        assert_eq!(a.mode(), Mode::Address);
        assert_eq!(a.on_focus_drain(drain(&[], &[true])), vec![]);
        assert_eq!(a.mode(), Mode::Address);
        assert_eq!(
            a.on_focus_drain(drain(&[true], &[false])),
            vec![chrome_keys(KeySet::Normal)]
        );
        assert_eq!(a.mode(), Mode::Normal);
    }

    /// Asserts that a click on the page closes the address bar, whether or
    /// not the chrome's loss arrives in the same pass.
    ///
    /// Case: the user opens the address bar and clicks the page instead of
    /// typing.
    #[test]
    fn clicking_the_page_closes_the_address_bar() {
        let mut a = ready_app();
        a.on_action(Action::OpenAddress);
        a.on_focus_drain(drain(&[], &[true]));
        assert_eq!(
            a.on_focus_drain(drain(&[true], &[])),
            vec![chrome_keys(KeySet::Normal)]
        );
        assert_eq!(a.mode(), Mode::Normal);
    }

    /// Asserts that opening the address bar while the chrome already holds
    /// focus does not wait for a gain, so a later click on the page closes it.
    ///
    /// Case: the user clicks the chrome outside the omnibox, presses `o`, and
    /// then clicks the page.
    #[test]
    fn opening_the_address_bar_from_a_focused_chrome_does_not_wait() {
        let mut a = ready_app();
        a.on_focus_drain(drain(&[], &[true]));
        a.on_action(Action::OpenAddress);
        assert_eq!(
            a.on_focus_drain(drain(&[true], &[false])),
            vec![chrome_keys(KeySet::Normal)]
        );
        assert_eq!(a.mode(), Mode::Normal);
    }

    /// Asserts that a key reaching the TUI in the address bar refocuses the
    /// chrome, and that Esc there closes the address bar.
    ///
    /// Case: the user types right after pressing `o`, then presses Esc before
    /// the chrome takes the keyboard.
    #[test]
    fn a_key_reaching_the_tui_refocuses_the_chrome() {
        let mut a = ready_app();
        assert_eq!(a.on_action(Action::RefocusChrome), vec![]);
        a.on_action(Action::OpenAddress);
        assert_eq!(
            a.on_action(Action::RefocusChrome),
            vec![Cmd::Focus(Target::Chrome)]
        );
        assert_eq!(
            a.on_action(Action::Escape),
            vec![chrome_keys(KeySet::Normal), Cmd::Blur]
        );
        assert_eq!(a.mode(), Mode::Normal);
    }

    /// Asserts that a key reaching the TUI while the chrome already holds
    /// focus does not wait for a gain, so a later click on the page closes the
    /// address bar.
    ///
    /// Case: the user clicks the chrome, presses `o`, and types `j` before the
    /// chrome's forward keys change, then clicks the page.
    #[test]
    fn refocusing_a_focused_chrome_does_not_wait() {
        let mut a = ready_app();
        a.on_focus_drain(drain(&[], &[true]));
        a.on_action(Action::OpenAddress);
        assert_eq!(
            a.on_action(Action::RefocusChrome),
            vec![Cmd::Focus(Target::Chrome)]
        );
        assert_eq!(
            a.on_focus_drain(drain(&[true], &[false])),
            vec![chrome_keys(KeySet::Normal)]
        );
        assert_eq!(a.mode(), Mode::Normal);
    }

    /// Asserts that an address bar opened before the chrome page is ready
    /// ignores focus losses until the chrome gains focus.
    ///
    /// Case: the user presses `o` right after launch, and a loss from an
    /// earlier click arrives before the chrome page has loaded.
    #[test]
    fn an_address_bar_opened_before_ready_ignores_stale_losses() {
        let mut a = app();
        a.on_action(Action::OpenAddress);
        assert_eq!(a.on_focus_drain(drain(&[false], &[])), vec![]);
        assert_eq!(a.mode(), Mode::Address);
    }

    /// Asserts that the omnibox click opens the address bar only from Normal
    /// mode.
    ///
    /// Case: the user clicks the omnibox while browsing, and while typing into
    /// the page in Insert mode.
    #[test]
    fn an_omnibox_click_opens_the_address_bar_from_normal_only() {
        let mut a = ready_app();
        assert_eq!(
            a.on_page_event(PageEvent::OpenAddress),
            vec![chrome_keys(KeySet::Empty), Cmd::Focus(Target::Chrome)]
        );
        let mut a = ready_app();
        a.on_action(Action::EnterInsert);
        assert_eq!(a.on_page_event(PageEvent::OpenAddress), vec![]);
        assert_eq!(a.mode(), Mode::Insert);
    }

    /// Asserts that a URL change while typing keeps the address bar's seed and
    /// epoch.
    ///
    /// Case: the page redirects while the user edits the address.
    #[test]
    fn a_url_change_while_typing_keeps_the_address_bar() {
        let mut a = ready_app();
        a.on_action(Action::OpenAddress);
        a.on_page_url_changed("https://example.com/next".to_owned());
        assert_eq!(a.mode(), Mode::Address);
        assert_eq!(a.seed(), URL);
        assert_eq!(a.address_epoch(), 1);
        assert_eq!(a.url(), "https://example.com/next");
    }

    /// Asserts that Insert mode hands the keyboard to the page with Esc as its
    /// only forward key, and takes it back on Esc.
    ///
    /// Case: the user presses `i` to type into a search box, then Esc.
    #[test]
    fn insert_mode_hands_the_keyboard_to_the_page_and_back() {
        let mut a = app();
        assert_eq!(
            a.on_action(Action::EnterInsert),
            vec![page_keys(KeySet::Insert), Cmd::Focus(Target::Page)]
        );
        assert_eq!(
            a.on_action(Action::Escape),
            vec![page_keys(KeySet::Normal), Cmd::Blur]
        );
        assert_eq!(a.mode(), Mode::Normal);
    }

    /// Asserts that Insert mode waits for the page's gain, then returns to
    /// Normal when the chrome takes focus.
    ///
    /// Case: the user presses `i`, then clicks the chrome.
    #[test]
    fn clicking_the_chrome_in_insert_mode_returns_to_normal() {
        let mut a = app();
        a.on_action(Action::EnterInsert);
        assert_eq!(a.on_focus_drain(drain(&[true], &[])), vec![]);
        assert_eq!(a.mode(), Mode::Insert);
        assert_eq!(
            a.on_focus_drain(drain(&[false], &[true])),
            vec![page_keys(KeySet::Normal)]
        );
        assert_eq!(a.mode(), Mode::Normal);
    }

    /// Asserts that Insert, Hint, and Help are ignored while the page has no
    /// rows.
    ///
    /// Case: the user shrinks the pane to two rows and presses `i`, `f`, and
    /// `?`.
    #[test]
    fn insert_hint_and_help_are_ignored_while_the_page_has_no_rows() {
        let mut a = app();
        a.set_page_placed(false);
        assert_eq!(a.on_action(Action::EnterInsert), vec![]);
        assert_eq!(a.on_action(Action::EnterHint), vec![]);
        assert_eq!(a.on_action(Action::OpenHelp), vec![]);
        assert_eq!(a.mode(), Mode::Normal);
    }

    /// Asserts that Hint and Help take the keyboard back from the webviews.
    ///
    /// Case: the user presses `f`, cancels, and presses `?`.
    #[test]
    fn hint_and_help_blur_the_webviews() {
        let mut a = app();
        assert_eq!(
            a.on_action(Action::EnterHint),
            vec![Cmd::HintShow, Cmd::Blur]
        );
        assert_eq!(a.on_action(Action::Escape), vec![Cmd::HintHide]);
        assert_eq!(a.on_action(Action::OpenHelp), vec![Cmd::Blur]);
        assert_eq!(a.mode(), Mode::Help);
    }

    /// Asserts that a click on either webview cancels Hint and Help.
    ///
    /// Case: the user clicks the chrome while picking a hint, and clicks the
    /// page while reading the help.
    #[test]
    fn a_click_cancels_hint_and_help() {
        let mut a = app();
        a.on_action(Action::EnterHint);
        assert_eq!(a.on_focus_drain(drain(&[], &[true])), vec![Cmd::HintHide]);
        assert_eq!(a.mode(), Mode::Normal);
        a.on_action(Action::OpenHelp);
        assert_eq!(a.on_focus_drain(drain(&[true], &[])), vec![]);
        assert_eq!(a.mode(), Mode::Normal);
    }

    /// Asserts that a hint on a form field enters Insert mode, other hints
    /// return to Normal, and a result after leaving Hint is ignored.
    ///
    /// Case: the user follows a hint onto a text input, onto a link, and
    /// presses Esc just before the page reports a hint.
    #[test]
    fn hint_results_resolve_the_hint_mode() {
        let mut a = app();
        a.on_action(Action::EnterHint);
        assert_eq!(
            a.on_hint_result("focusedInput"),
            vec![page_keys(KeySet::Insert), Cmd::Focus(Target::Page)]
        );
        assert_eq!(a.mode(), Mode::Insert);

        for kind in ["navigated", "clicked", "empty"] {
            let mut a = app();
            a.on_action(Action::EnterHint);
            assert_eq!(a.on_hint_result(kind), vec![], "{kind}");
            assert_eq!(a.mode(), Mode::Normal, "{kind}");
        }

        let mut a = app();
        a.on_action(Action::EnterHint);
        a.on_action(Action::Escape);
        assert_eq!(a.on_hint_result("focusedInput"), vec![]);
        assert_eq!(a.mode(), Mode::Normal);
    }

    /// Asserts that leaving Hint or Help gives the keyboard back to a page
    /// that held it.
    ///
    /// Case: the user clicks the page, then opens the hints or the help and
    /// leaves each with its own way out.
    #[test]
    fn leaving_hint_or_help_refocuses_a_page_that_held_focus() {
        let mut a = app();
        a.on_focus_drain(drain(&[true], &[]));
        a.on_action(Action::EnterHint);
        assert_eq!(
            a.on_action(Action::Escape),
            vec![Cmd::HintHide, Cmd::Focus(Target::Page)]
        );

        let mut a = app();
        a.on_focus_drain(drain(&[true], &[]));
        a.on_action(Action::EnterHint);
        assert_eq!(a.on_hint_result("clicked"), vec![Cmd::Focus(Target::Page)]);

        let mut a = app();
        a.on_focus_drain(drain(&[true], &[]));
        a.on_action(Action::OpenHelp);
        assert_eq!(a.on_action(Action::Escape), vec![Cmd::Focus(Target::Page)]);
    }
}
