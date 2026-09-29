//! App state machine for orzbrowser: each entry point updates the state and
//! returns the side effects to perform.

use crate::address::AddressTarget;
use crate::focus::{FocusDrain, Target};
use crate::keymap::{Action, KeySet, Mode};
use crate::protocol::{ChromeEvent, PageEvent};
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
    /// Drop the page's pending chord.
    CancelChord,
    /// Turn the page preload's scroll keys on or off.
    SetPageScrollKeys(bool),
    /// Take focus from the page's focused text field.
    BlurPageInput,
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
    page_pending: Option<char>,
    url: String,
    seed: String,
    address_epoch: u32,
    holder: Option<Target>,
    awaiting: Option<Target>,
    pane_focused: bool,
    page_placed: bool,
    chrome_ready: bool,
    page_keys: KeySet,
    chrome_keys: KeySet,
    page_scroll_keys_sent: bool,
}

impl App {
    /// Creates an app that shows `url` in Normal mode.
    pub fn new(url: String) -> Self {
        Self {
            mode: Mode::Normal,
            pending_prefix: None,
            page_pending: None,
            url,
            seed: String::new(),
            address_epoch: 0,
            holder: None,
            awaiting: None,
            pane_focused: true,
            page_placed: true,
            chrome_ready: false,
            page_keys: KeySet::PageNormal,
            chrome_keys: KeySet::Normal,
            page_scroll_keys_sent: true,
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
            page_scroll_keys_sent: false,
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

    /// The first key of a pending two-key chord: the TUI's, else the page's
    /// in Normal mode.
    pub fn pending_key(&self) -> Option<char> {
        self.pending_prefix
            .or(self.page_pending.filter(|_| self.mode == Mode::Normal))
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

    /// Records whether orzbrowser's pane holds the keyboard. In Normal mode a
    /// gain gives the page keyboard focus; while the pane lacks the keyboard,
    /// the page is never asked for focus.
    pub fn on_pane_focus(&mut self, focused: bool) -> Vec<Cmd> {
        self.pane_focused = focused;
        let cmds = self.claim_page_focus();
        self.with_key_sets(cmds)
    }

    /// Processes an [`Action`] and returns the side effects to perform. In
    /// Normal mode an action also drops the page's pending chord.
    pub fn on_action(&mut self, action: Action) -> Vec<Cmd> {
        let cancel_page_chord = self.mode == Mode::Normal && self.page_pending.take().is_some();
        let mut cmds = self.action_cmds(action);
        if cancel_page_chord {
            cmds.insert(0, Cmd::CancelChord);
        }
        self.with_key_sets(cmds)
    }

    // TODO: a text field that takes focus in Normal mode (a click, Tab, or a
    // page shortcut such as `/`) keeps the page's Normal forward keys, so
    // typing `q`, `r`, `o`, `i`, `f`, `H`, or `L` into it runs the command
    // instead of typing. Entering Insert automatically needs the page's
    // preload to report that an editable element took focus.
    /// Applies the focus changes both webviews reported in one loop pass.
    ///
    /// While a focus request of this app is unanswered, no mode is cancelled.
    /// Otherwise a chrome that lost focus closes the address bar, a page that
    /// lost focus leaves Insert mode and its text field, and a gain on either
    /// webview cancels Hint and Help. In Normal mode a chrome that gained
    /// focus hands it to the page, even while a request is unanswered. A page
    /// that gained focus drops the TUI's pending chord.
    pub fn on_focus_drain(&mut self, drain: FocusDrain) -> Vec<Cmd> {
        if drain.is_empty() {
            return vec![];
        }
        self.holder = drain.holder_after(self.holder);
        if drain.page.last == Some(true) {
            self.pending_prefix = None;
        }
        if self
            .awaiting
            .is_some_and(|target| drain.of(target).saw_true)
        {
            self.awaiting = None;
        }
        let mut cmds = if self.awaiting.is_some() {
            vec![]
        } else {
            self.cancel_modes_after(&drain)
        };
        if self.holder == Some(Target::Chrome) {
            cmds.extend(self.claim_page_focus());
        }
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
        let cmds = self.return_to_normal(navigate);
        self.with_key_sets(cmds)
    }

    /// Applies a report from the chrome page: the first ready lets an open
    /// address bar take keyboard focus, Esc closes the address bar, and a
    /// click on the omnibox in Normal mode opens it.
    pub fn on_chrome_event(&mut self, event: ChromeEvent) -> Vec<Cmd> {
        let cmds = match event {
            ChromeEvent::Ready => self.chrome_ready(),
            ChromeEvent::Cancel if self.mode == Mode::Address => self.return_to_normal(None),
            ChromeEvent::OpenAddress if self.mode == Mode::Normal => self.open_address(),
            ChromeEvent::Cancel | ChromeEvent::OpenAddress => vec![],
        };
        self.with_key_sets(cmds)
    }

    /// Applies a report from the page's preload. A ready means a fresh page
    /// whose scroll keys start off: they are told again, the page's pending
    /// chord is dropped, and the page is focused in Normal mode. A pending
    /// report records the chord key the toolbar shows.
    pub fn on_page_event(&mut self, event: PageEvent) -> Vec<Cmd> {
        let cmds = match event {
            PageEvent::Ready => {
                self.page_scroll_keys_sent = false;
                self.page_pending = None;
                self.claim_page_focus()
            }
            PageEvent::Pending { key } => {
                self.page_pending = key;
                vec![]
            }
        };
        self.with_key_sets(cmds)
    }

    /// Applies a `hintResult` reported by the page: a hint that focused a
    /// form field switches to Insert mode with the page focused; any other
    /// result returns to Normal with the page focused. A no-op outside Hint
    /// mode.
    pub fn on_hint_result(&mut self, kind: &str) -> Vec<Cmd> {
        if self.mode != Mode::Hint {
            return vec![];
        }
        let cmds = if kind == "focusedInput" {
            self.mode = Mode::Insert;
            vec![self.request_focus(Target::Page)]
        } else {
            self.mode = Mode::Normal;
            vec![self.normal_focus()]
        };
        self.with_key_sets(cmds)
    }

    /// Records a page-driven URL change reported via `urlChanged`.
    pub fn on_page_url_changed(&mut self, url: String) {
        self.url = url;
    }

    fn action_cmds(&mut self, action: Action) -> Vec<Cmd> {
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
            Action::OpenAddress => self.open_address(),
            Action::RefocusChrome => self.refocus_chrome(),
            Action::Escape => self.escape(),
            Action::EnterInsert => self.enter_insert(),
            Action::EnterHint => self.enter_hint(),
            Action::HintKey(c) => vec![Cmd::HintKey(c)],
            Action::HintBackspace => vec![Cmd::HintBackspace],
            Action::OpenHelp => self.open_help(),
            Action::Ignore => vec![],
        }
    }

    fn cancel_modes_after(&mut self, drain: &FocusDrain) -> Vec<Cmd> {
        let cancel = match self.mode {
            Mode::Address => self.holder != Some(Target::Chrome),
            Mode::Insert => self.holder != Some(Target::Page),
            Mode::Hint | Mode::Help => drain.saw_any_true(),
            Mode::Normal => false,
        };
        if cancel {
            self.exit_to_normal().into_iter().collect()
        } else {
            vec![]
        }
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
        self.seed = self.url.clone();
        self.address_epoch = self.address_epoch.wrapping_add(1);
        if self.chrome_ready {
            vec![self.request_focus(Target::Chrome)]
        } else {
            self.awaiting = Some(Target::Chrome);
            vec![]
        }
    }

    /// Returns to Normal mode: `lead`, then the cleanup of the mode being
    /// left, then the focus for Normal mode.
    fn return_to_normal(&mut self, lead: Option<Cmd>) -> Vec<Cmd> {
        let cleanup = self.exit_to_normal();
        let focus = self.normal_focus();
        lead.into_iter().chain(cleanup).chain([focus]).collect()
    }

    /// Switches to Normal mode and returns the cleanup the mode being left
    /// needs: Insert takes focus from the page's text field, and Hint tears
    /// down its overlay.
    fn exit_to_normal(&mut self) -> Option<Cmd> {
        let cleanup = match self.mode {
            Mode::Insert => Some(Cmd::BlurPageInput),
            Mode::Hint => Some(Cmd::HintHide),
            Mode::Normal | Mode::Address | Mode::Help => None,
        };
        self.mode = Mode::Normal;
        cleanup
    }

    fn refocus_chrome(&mut self) -> Vec<Cmd> {
        if self.mode != Mode::Address || !self.chrome_ready {
            return vec![];
        }
        vec![self.request_focus(Target::Chrome)]
    }

    fn escape(&mut self) -> Vec<Cmd> {
        if self.mode == Mode::Normal {
            return vec![];
        }
        self.return_to_normal(None)
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
        self.mode = Mode::Hint;
        vec![Cmd::HintShow, self.blur()]
    }

    fn open_help(&mut self) -> Vec<Cmd> {
        if !self.page_placed {
            return vec![];
        }
        self.mode = Mode::Help;
        vec![self.blur()]
    }

    /// The focus for entering Normal mode: the page when it has rows and the
    /// pane holds the keyboard, otherwise the TUI.
    fn normal_focus(&mut self) -> Cmd {
        if self.page_placed && self.pane_focused {
            self.request_focus(Target::Page)
        } else {
            self.blur()
        }
    }

    /// Asks for page focus when Normal mode should give the page the keyboard
    /// but the page does not hold it.
    fn claim_page_focus(&mut self) -> Vec<Cmd> {
        let claim = self.mode == Mode::Normal
            && self.pane_focused
            && self.page_placed
            && self.holder != Some(Target::Page);
        if claim {
            vec![self.request_focus(Target::Page)]
        } else {
            vec![]
        }
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

    /// Whether the page's preload should handle the scroll keys: in Normal
    /// mode while the pane holds the keyboard.
    fn page_scroll_keys(&self) -> bool {
        self.mode == Mode::Normal && self.pane_focused
    }

    /// Puts the commands that sync the webviews with the current mode first
    /// in `cmds`: whether the page's preload handles the scroll keys, then a
    /// `SetForwardKeys` for each webview whose key set the mode changes.
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
        let scroll_keys = self.page_scroll_keys();
        if scroll_keys != self.page_scroll_keys_sent {
            self.page_scroll_keys_sent = scroll_keys;
            cmds.insert(0, Cmd::SetPageScrollKeys(scroll_keys));
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
        a.on_chrome_event(ChromeEvent::Ready);
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

    /// Asserts that opening the address bar turns the page's scroll keys off,
    /// empties the chrome's forward keys, focuses the chrome, and seeds the
    /// input with the current URL.
    ///
    /// Case: the user presses `o` to type a new address.
    #[test]
    fn opening_the_address_bar_focuses_the_chrome() {
        let mut a = ready_app();
        assert_eq!(
            a.on_action(Action::OpenAddress),
            vec![
                Cmd::SetPageScrollKeys(false),
                chrome_keys(KeySet::Empty),
                Cmd::Focus(Target::Chrome)
            ]
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
            vec![Cmd::SetPageScrollKeys(false), chrome_keys(KeySet::Empty)]
        );
        assert_eq!(
            a.on_chrome_event(ChromeEvent::Ready),
            vec![Cmd::Focus(Target::Chrome)]
        );
        assert_eq!(a.on_chrome_event(ChromeEvent::Ready), vec![]);
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
            a.on_chrome_event(ChromeEvent::Ready),
            vec![Cmd::Focus(Target::Chrome)]
        );
        assert_eq!(a.on_focus_drain(drain(&[], &[true])), vec![]);
        assert_eq!(a.mode(), Mode::Address);
    }

    /// Asserts that submitting an address or a search navigates, turns the
    /// page's scroll keys back on, returns the chrome keys, and gives the
    /// keyboard to the page.
    ///
    /// Case: the user types a domain, and later a phrase, and presses Enter.
    #[test]
    fn submitting_navigates_and_focuses_the_page() {
        let mut a = ready_app();
        a.on_action(Action::OpenAddress);
        a.on_focus_drain(drain(&[], &[true]));
        assert_eq!(
            a.on_address_target(AddressTarget::Open("https://docs.rs/".to_owned())),
            vec![
                Cmd::SetPageScrollKeys(true),
                chrome_keys(KeySet::Normal),
                Cmd::Navigate("https://docs.rs/".to_owned()),
                Cmd::Focus(Target::Page)
            ]
        );
        assert_eq!(a.mode(), Mode::Normal);

        a.on_action(Action::OpenAddress);
        let search = "https://duckduckgo.com/?q=rust%20async".to_owned();
        assert_eq!(
            a.on_address_target(AddressTarget::Search(search.clone())),
            vec![
                Cmd::SetPageScrollKeys(true),
                chrome_keys(KeySet::Normal),
                Cmd::Navigate(search),
                Cmd::Focus(Target::Page)
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
            vec![
                Cmd::SetPageScrollKeys(true),
                chrome_keys(KeySet::Normal),
                Cmd::Reload,
                Cmd::Focus(Target::Page)
            ]
        );
        a.on_action(Action::OpenAddress);
        assert_eq!(
            a.on_address_target(AddressTarget::Empty),
            vec![
                Cmd::SetPageScrollKeys(true),
                chrome_keys(KeySet::Normal),
                Cmd::Focus(Target::Page)
            ]
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
        a.on_chrome_event(ChromeEvent::Cancel);
        assert_eq!(
            a.on_address_target(AddressTarget::Open("https://docs.rs/".to_owned())),
            vec![]
        );
    }

    /// Asserts that closing the address bar gives the keyboard to the page,
    /// whether or not the page held it when the bar opened.
    ///
    /// Case: the user presses `o` right after launch and presses Esc; later
    /// the user clicks the page, presses `o`, and presses Esc.
    #[test]
    fn closing_the_address_bar_focuses_the_page() {
        let mut a = ready_app();
        a.on_action(Action::OpenAddress);
        assert_eq!(
            a.on_chrome_event(ChromeEvent::Cancel),
            vec![
                Cmd::SetPageScrollKeys(true),
                chrome_keys(KeySet::Normal),
                Cmd::Focus(Target::Page)
            ]
        );

        let mut a = ready_app();
        a.on_focus_drain(drain(&[true], &[]));
        a.on_action(Action::OpenAddress);
        a.on_focus_drain(drain(&[false], &[true]));
        assert_eq!(
            a.on_chrome_event(ChromeEvent::Cancel),
            vec![
                Cmd::SetPageScrollKeys(true),
                chrome_keys(KeySet::Normal),
                Cmd::Focus(Target::Page)
            ]
        );
    }

    /// Asserts that closing the address bar blurs instead of focusing a page
    /// that has no rows.
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
            a.on_chrome_event(ChromeEvent::Cancel),
            vec![
                Cmd::SetPageScrollKeys(true),
                chrome_keys(KeySet::Normal),
                Cmd::Blur
            ]
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
        a.on_chrome_event(ChromeEvent::Cancel);
        a.on_action(Action::OpenAddress);
        assert_eq!(a.address_epoch(), 2);
        assert_eq!(a.on_focus_drain(drain(&[], &[false])), vec![]);
        assert_eq!(a.mode(), Mode::Address);
        assert_eq!(a.on_focus_drain(drain(&[], &[true])), vec![]);
        assert_eq!(a.mode(), Mode::Address);
        assert_eq!(
            a.on_focus_drain(drain(&[true], &[false])),
            vec![Cmd::SetPageScrollKeys(true), chrome_keys(KeySet::Normal)]
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
            vec![Cmd::SetPageScrollKeys(true), chrome_keys(KeySet::Normal)]
        );
        assert_eq!(a.mode(), Mode::Normal);
    }

    /// Asserts that a key reaching the TUI in the address bar refocuses the
    /// chrome, and that Esc there closes the address bar and focuses the page.
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
            vec![
                Cmd::SetPageScrollKeys(true),
                chrome_keys(KeySet::Normal),
                Cmd::Focus(Target::Page)
            ]
        );
        assert_eq!(a.mode(), Mode::Normal);
    }

    /// Asserts that a key reaching the TUI while the chrome already holds
    /// focus does not wait for a gain, so a later click on the page closes the
    /// address bar.
    ///
    /// Case: the user presses `o`, the chrome takes the keyboard, a `j` typed
    /// before the chrome's forward keys changed reaches the TUI, and the user
    /// then clicks the page.
    #[test]
    fn refocusing_a_focused_chrome_does_not_wait() {
        let mut a = ready_app();
        a.on_action(Action::OpenAddress);
        a.on_focus_drain(drain(&[], &[true]));
        assert_eq!(
            a.on_action(Action::RefocusChrome),
            vec![Cmd::Focus(Target::Chrome)]
        );
        assert_eq!(
            a.on_focus_drain(drain(&[true], &[false])),
            vec![Cmd::SetPageScrollKeys(true), chrome_keys(KeySet::Normal)]
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
            a.on_chrome_event(ChromeEvent::OpenAddress),
            vec![
                Cmd::SetPageScrollKeys(false),
                chrome_keys(KeySet::Empty),
                Cmd::Focus(Target::Chrome)
            ]
        );
        let mut a = ready_app();
        a.on_action(Action::EnterInsert);
        assert_eq!(a.on_chrome_event(ChromeEvent::OpenAddress), vec![]);
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

    /// Asserts that Insert mode turns the page's scroll keys off and hands the
    /// keyboard to the page with Esc as its only forward key, and that Esc
    /// turns them back on, takes focus from the page's text field, and keeps
    /// the page focused.
    ///
    /// Case: the user presses `i` to type into a search box, then Esc.
    #[test]
    fn insert_mode_hands_the_keyboard_to_the_page_and_esc_keeps_it() {
        let mut a = app();
        assert_eq!(
            a.on_action(Action::EnterInsert),
            vec![
                Cmd::SetPageScrollKeys(false),
                page_keys(KeySet::Insert),
                Cmd::Focus(Target::Page)
            ]
        );
        assert_eq!(
            a.on_action(Action::Escape),
            vec![
                Cmd::SetPageScrollKeys(true),
                page_keys(KeySet::PageNormal),
                Cmd::BlurPageInput,
                Cmd::Focus(Target::Page)
            ]
        );
        assert_eq!(a.mode(), Mode::Normal);
    }

    /// Asserts that Insert mode waits for the page's gain, then returns to
    /// Normal when the chrome takes focus, takes focus from the page's text
    /// field, and hands the keyboard back to the page.
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
            vec![
                Cmd::SetPageScrollKeys(true),
                page_keys(KeySet::PageNormal),
                Cmd::BlurPageInput,
                Cmd::Focus(Target::Page)
            ]
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

    /// Asserts that Hint and Help turn the page's scroll keys off before
    /// anything else and take the keyboard back from the webviews, and that
    /// leaving Hint gives it to the page.
    ///
    /// Case: the user presses `f`, cancels, and presses `?`.
    #[test]
    fn hint_and_help_blur_the_webviews() {
        let mut a = app();
        assert_eq!(
            a.on_action(Action::EnterHint),
            vec![Cmd::SetPageScrollKeys(false), Cmd::HintShow, Cmd::Blur]
        );
        assert_eq!(
            a.on_action(Action::Escape),
            vec![
                Cmd::SetPageScrollKeys(true),
                Cmd::HintHide,
                Cmd::Focus(Target::Page)
            ]
        );
        assert_eq!(
            a.on_action(Action::OpenHelp),
            vec![Cmd::SetPageScrollKeys(false), Cmd::Blur]
        );
        assert_eq!(a.mode(), Mode::Help);
    }

    /// Asserts that a click on either webview cancels Hint and Help, and that
    /// a click on the chrome hands the keyboard to the page.
    ///
    /// Case: the user clicks the chrome while picking a hint, and clicks the
    /// page while reading the help.
    #[test]
    fn a_click_cancels_hint_and_help() {
        let mut a = app();
        a.on_action(Action::EnterHint);
        assert_eq!(
            a.on_focus_drain(drain(&[], &[true])),
            vec![
                Cmd::SetPageScrollKeys(true),
                Cmd::HintHide,
                Cmd::Focus(Target::Page)
            ]
        );
        assert_eq!(a.mode(), Mode::Normal);
        a.on_action(Action::OpenHelp);
        assert_eq!(
            a.on_focus_drain(drain(&[true], &[])),
            vec![Cmd::SetPageScrollKeys(true)]
        );
        assert_eq!(a.mode(), Mode::Normal);
    }

    /// Asserts that a hint on a form field enters Insert mode, other hints
    /// return to Normal with the page focused, and a result after leaving Hint
    /// is ignored.
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
            assert_eq!(
                a.on_hint_result(kind),
                vec![Cmd::SetPageScrollKeys(true), Cmd::Focus(Target::Page)],
                "{kind}"
            );
            assert_eq!(a.mode(), Mode::Normal, "{kind}");
        }

        let mut a = app();
        a.on_action(Action::EnterHint);
        a.on_action(Action::Escape);
        assert_eq!(a.on_hint_result("focusedInput"), vec![]);
        assert_eq!(a.mode(), Mode::Normal);
    }

    /// Asserts that leaving Hint or Help gives the keyboard to the page.
    ///
    /// Case: the user opens the hints or the help and leaves each with its own
    /// way out.
    #[test]
    fn leaving_hint_or_help_focuses_the_page() {
        let mut a = app();
        a.on_action(Action::EnterHint);
        assert_eq!(
            a.on_action(Action::Escape),
            vec![
                Cmd::SetPageScrollKeys(true),
                Cmd::HintHide,
                Cmd::Focus(Target::Page)
            ]
        );

        let mut a = app();
        a.on_action(Action::EnterHint);
        assert_eq!(
            a.on_hint_result("clicked"),
            vec![Cmd::SetPageScrollKeys(true), Cmd::Focus(Target::Page)]
        );

        let mut a = app();
        a.on_action(Action::OpenHelp);
        assert_eq!(
            a.on_action(Action::Escape),
            vec![Cmd::SetPageScrollKeys(true), Cmd::Focus(Target::Page)]
        );
    }

    /// Asserts that a click on the toolbar outside the omnibox hands the
    /// keyboard back to the page.
    ///
    /// Case: the user clicks the mode badge while browsing.
    #[test]
    fn a_toolbar_click_in_normal_mode_refocuses_the_page() {
        let mut a = ready_app();
        assert_eq!(
            a.on_focus_drain(drain(&[], &[true])),
            vec![Cmd::Focus(Target::Page)]
        );
        assert_eq!(a.on_focus_drain(drain(&[true], &[false])), vec![]);
        assert_eq!(a.mode(), Mode::Normal);
    }

    /// Asserts that a toolbar click hands the keyboard back to the page even
    /// while a page focus request is still unanswered.
    ///
    /// Case: the user closes the hints with Esc and clicks the mode badge
    /// before the page reports its focus gain.
    #[test]
    fn a_toolbar_click_while_the_page_focus_is_pending_refocuses_the_page() {
        let mut a = ready_app();
        a.on_action(Action::EnterHint);
        a.on_action(Action::Escape);
        assert_eq!(
            a.on_focus_drain(drain(&[], &[true])),
            vec![Cmd::Focus(Target::Page)]
        );
    }

    /// Asserts that an omnibox click still ends with the chrome focused in the
    /// address bar when the chrome's gain arrives before the `openAddress`
    /// report.
    ///
    /// Case: the user clicks the omnibox while browsing, and the chrome's
    /// focus gain lands in the pass before its report.
    #[test]
    fn an_omnibox_click_ends_with_the_chrome_focused() {
        let mut a = ready_app();
        a.on_focus_drain(drain(&[], &[true]));
        assert_eq!(
            a.on_chrome_event(ChromeEvent::OpenAddress),
            vec![
                Cmd::SetPageScrollKeys(false),
                chrome_keys(KeySet::Empty),
                Cmd::Focus(Target::Chrome)
            ]
        );
        assert_eq!(
            a.on_focus_drain(drain(&[true, false], &[false, true])),
            vec![]
        );
        assert_eq!(a.mode(), Mode::Address);
    }

    /// Asserts that Insert mode left because the page lost focus to something
    /// other than the chrome takes focus from the page's text field without
    /// asking for page focus.
    ///
    /// Case: the user types into the page in Insert mode and clicks another
    /// pane.
    #[test]
    fn losing_the_page_elsewhere_leaves_insert_without_a_request() {
        let mut a = app();
        a.on_action(Action::EnterInsert);
        a.on_focus_drain(drain(&[true], &[]));
        assert_eq!(
            a.on_focus_drain(drain(&[false], &[])),
            vec![
                Cmd::SetPageScrollKeys(true),
                page_keys(KeySet::PageNormal),
                Cmd::BlurPageInput
            ]
        );
        assert_eq!(a.mode(), Mode::Normal);
    }

    /// Asserts that leaving a mode while the pane lacks the keyboard blurs
    /// instead of focusing the page.
    ///
    /// Case: the user presses `f`, picks a link, and switches to another pane
    /// before the page reports the hint.
    #[test]
    fn leaving_a_mode_without_the_pane_blurs() {
        let mut a = app();
        a.on_action(Action::EnterHint);
        assert_eq!(a.on_pane_focus(false), vec![]);
        assert_eq!(a.on_hint_result("clicked"), vec![Cmd::Blur]);
    }

    /// Asserts that the pane losing the keyboard turns the page's scroll keys
    /// off, and that regaining it in Normal mode turns them on and focuses the
    /// page once.
    ///
    /// Case: the user switches to another pane and back with the pane
    /// shortcuts while browsing.
    #[test]
    fn regaining_the_pane_focuses_the_page() {
        let mut a = app();
        assert_eq!(a.on_pane_focus(false), vec![Cmd::SetPageScrollKeys(false)]);
        assert_eq!(
            a.on_pane_focus(true),
            vec![Cmd::SetPageScrollKeys(true), Cmd::Focus(Target::Page)]
        );
        assert_eq!(a.on_focus_drain(drain(&[true], &[])), vec![]);
        assert_eq!(a.on_pane_focus(true), vec![]);
    }

    /// Asserts that the page handles its scroll keys only in Normal mode while
    /// the pane holds the keyboard.
    ///
    /// Case: the user browses, presses `i` to type, presses Esc, and switches
    /// to another app with a key still down.
    #[test]
    fn the_page_scroll_keys_follow_the_mode_and_the_pane() {
        let mut a = app();
        assert!(a.page_scroll_keys());
        a.on_action(Action::EnterInsert);
        assert!(!a.page_scroll_keys());
        a.on_action(Action::Escape);
        assert!(a.page_scroll_keys());
        a.on_pane_focus(false);
        assert!(!a.page_scroll_keys());
    }

    /// Asserts that a page ready turns a fresh page's scroll keys on in Normal
    /// mode and focuses the page, does neither in the address bar or without
    /// the pane, and does not refocus a page that holds focus.
    ///
    /// Case: orzbrowser launches with an address and the page reloads, a page
    /// loads while the user works in another pane, and orzbrowser launches
    /// with the address bar open.
    #[test]
    fn a_page_ready_resends_the_scroll_keys_and_focuses_the_page() {
        let mut a = app();
        assert_eq!(
            a.on_page_event(PageEvent::Ready),
            vec![Cmd::SetPageScrollKeys(true), Cmd::Focus(Target::Page)]
        );
        assert_eq!(
            a.on_page_event(PageEvent::Ready),
            vec![Cmd::SetPageScrollKeys(true)]
        );

        let mut a = app();
        a.on_pane_focus(false);
        assert_eq!(a.on_page_event(PageEvent::Ready), vec![]);

        let mut a = App::with_address_open(URL.to_owned());
        assert_eq!(a.on_page_event(PageEvent::Ready), vec![]);
    }

    /// Asserts that the page's pending `g` shows in Normal mode, that a key
    /// the TUI handles cancels it, and that a page ready drops it.
    ///
    /// Case: the user presses `g` on the page and then `H`; later presses `g`
    /// and the page reloads.
    #[test]
    fn the_page_chord_shows_and_a_tui_key_cancels_it() {
        let mut a = app();
        a.on_page_event(PageEvent::Pending { key: Some('g') });
        assert_eq!(a.pending_key(), Some('g'));
        assert_eq!(
            a.on_action(Action::HistoryBack),
            vec![Cmd::CancelChord, Cmd::HistoryBack]
        );
        assert_eq!(a.pending_key(), None);

        a.on_page_event(PageEvent::Pending { key: Some('g') });
        a.on_page_event(PageEvent::Ready);
        assert_eq!(a.pending_key(), None);
    }

    /// Asserts that the TUI's pending key shows whatever the page reports, and
    /// that the page's shows only in Normal mode.
    ///
    /// Case: a stale page report arrives while the user types in Insert mode,
    /// and the user presses `g` in the TUI before the page takes focus.
    #[test]
    fn the_tui_chord_wins_and_the_page_chord_needs_normal_mode() {
        let mut a = app();
        a.on_action(Action::EnterInsert);
        a.on_page_event(PageEvent::Pending { key: Some('g') });
        assert_eq!(a.pending_key(), None);

        let mut a = app();
        a.on_action(Action::Prefix('g'));
        a.on_page_event(PageEvent::Pending { key: None });
        assert_eq!(a.pending_key(), Some('g'));
    }

    /// Asserts that the page gaining focus drops the TUI's pending chord.
    ///
    /// Case: the user presses `g` while the page is still loading, and the
    /// page then reports ready and takes the keyboard.
    #[test]
    fn a_page_focus_gain_drops_the_tui_chord() {
        let mut a = app();
        a.on_action(Action::Prefix('g'));
        a.on_page_event(PageEvent::Ready);
        a.on_focus_drain(drain(&[true], &[]));
        assert_eq!(a.pending_key(), None);
    }
}
