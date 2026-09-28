//! The pure App state machine. `on_action` is the single entry point; it
//! returns the side-effect [`Cmd`]s for `main.rs` to execute. No SDK or I/O here.

use crate::chrome::SearchStage;
use crate::keymap::{Action, KeySet, Mode};
use crate::outline::Heading;
use crate::protocol::{ScrollAction, SearchCause, SearchDir};
use std::mem;

/// A side effect for `main.rs` to perform after `on_action`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Cmd {
    /// Scroll the page.
    Scroll(ScrollAction),
    /// Scroll heading `index` (the `id="h{index}"` anchor) into view.
    ScrollToHeading(usize),
    /// Navigate to the next/previous search match.
    SearchNav(SearchDir),
    /// Clear the in-page search highlight.
    ClearSearch,
    /// Put a character into the page's search input, replacing its selection.
    SearchType(char),
    /// Delete backwards in the page's search input.
    SearchBackspace,
    /// Handle an Enter the TUI received as Enter in the page's search input.
    SearchEnter,
    /// Ask the page to end the typed search by its match count.
    SearchResolve,
    /// Abandon the typed search and return to where it started.
    SearchCancel,
    /// Replace the page's forward keys with the given set.
    SetForwardKeys(KeySet),
    /// Re-read the file from disk and push new content.
    Reload,
    /// Pop the navigation back stack.
    Back,
    /// Exit the app.
    Quit,
    /// Give the page keyboard focus.
    Focus,
    /// Take keyboard focus back from the page to the TUI.
    Blur,
}

/// Whole-app state.
#[derive(Debug, Default)]
pub(crate) struct App {
    mode: Mode,
    pending_prefix: Option<char>,
    outline: Vec<Heading>,
    outline_open: bool,
    outline_selected: usize,
    current_heading_index: Option<usize>,
    search_active: bool,
    page_focused: bool,
    blur_after_search: bool,
    key_set: KeySet,
}

impl App {
    /// The current input mode.
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Whether the outline panel is open.
    pub fn outline_open(&self) -> bool {
        self.outline_open
    }

    /// The selected outline index.
    pub fn selected(&self) -> usize {
        self.outline_selected
    }

    /// The headings to draw in the outline panel.
    pub fn outline(&self) -> &[Heading] {
        &self.outline
    }

    /// Replaces the outline (called after a (re)load), clamping the selection.
    pub fn set_outline(&mut self, outline: Vec<Heading>) {
        self.outline = outline;
        if self.outline_selected >= self.outline.len() {
            self.outline_selected = self.outline.len().saturating_sub(1);
        }
    }

    /// Records the heading index nearest the viewport top (from `scrollState`).
    pub fn set_current_heading_index(&mut self, index: Option<usize>) {
        self.current_heading_index = index;
    }

    /// The first key of a pending two-key chord (`g`, `[`, `]`), if any.
    pub fn pending_key(&self) -> Option<char> {
        self.pending_prefix
    }

    /// The search stage the page shows: `Typing` while a query is being
    /// typed, `Active` while confirmed matches are highlighted, else `Closed`.
    pub fn search_stage(&self) -> SearchStage {
        if self.mode == Mode::Search {
            SearchStage::Typing
        } else if self.search_active {
            SearchStage::Active
        } else {
            SearchStage::Closed
        }
    }

    /// Processes an [`Action`], returning the side effects to perform.
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
            Action::Back => vec![Cmd::Back],
            Action::ScrollLineDown => vec![Cmd::Scroll(ScrollAction::Down)],
            Action::ScrollLineUp => vec![Cmd::Scroll(ScrollAction::Up)],
            Action::ScrollHalfDown => vec![Cmd::Scroll(ScrollAction::HalfDown)],
            Action::ScrollHalfUp => vec![Cmd::Scroll(ScrollAction::HalfUp)],
            Action::ScrollPageDown => vec![Cmd::Scroll(ScrollAction::PageDown)],
            Action::ScrollPageUp => vec![Cmd::Scroll(ScrollAction::PageUp)],
            Action::GoBottom => vec![Cmd::Scroll(ScrollAction::Bottom)],
            Action::ToggleOutline => {
                self.outline_open = !self.outline_open;
                self.mode = if self.outline_open {
                    Mode::Outline
                } else {
                    Mode::Normal
                };
                vec![]
            }
            Action::OutlineMoveDown => {
                if self.outline_selected + 1 < self.outline.len() {
                    self.outline_selected += 1;
                }
                vec![]
            }
            Action::OutlineMoveUp => {
                self.outline_selected = self.outline_selected.saturating_sub(1);
                vec![]
            }
            Action::OutlineConfirm => {
                if self.outline.is_empty() {
                    vec![]
                } else {
                    vec![Cmd::ScrollToHeading(self.outline_selected)]
                }
            }
            Action::EnterSearch => {
                self.blur_after_search = !self.page_focused;
                self.mode = Mode::Search;
                vec![Cmd::Focus]
            }
            Action::SearchChar(c) => vec![Cmd::SearchType(c)],
            Action::SearchBackspace => vec![Cmd::SearchBackspace],
            Action::SearchConfirm => vec![Cmd::SearchEnter],
            Action::PageSearchSubmit(cause) if self.mode == Mode::Search => {
                self.mode = Mode::Normal;
                self.search_active = true;
                self.release_after_search(cause).into_iter().collect()
            }
            Action::PageSearchEscape(cause) if self.mode == Mode::Search => {
                self.cancel_search(cause)
            }
            Action::PageSearchSubmit(_) | Action::PageSearchEscape(_) => vec![],
            Action::PageSearchClose if self.mode == Mode::Normal && self.search_active => {
                self.search_active = false;
                vec![Cmd::ClearSearch]
            }
            Action::PageSearchClose => vec![],
            Action::SearchNext if self.search_active => vec![Cmd::SearchNav(SearchDir::Next)],
            Action::SearchPrev if self.search_active => vec![Cmd::SearchNav(SearchDir::Prev)],
            Action::SearchNext | Action::SearchPrev => vec![],
            Action::Escape if self.mode == Mode::Search => self.cancel_search(SearchCause::Key),
            Action::Escape => {
                self.outline_open = false;
                self.mode = Mode::Normal;
                if self.search_active {
                    self.search_active = false;
                    vec![Cmd::ClearSearch]
                } else {
                    vec![]
                }
            }
            Action::Ignore => vec![],
        };
        self.with_key_set(cmds)
    }

    /// Clears search state when the viewed document changes, returning the
    /// forward-key change that leaving a typed search needs.
    pub fn clear_search_state(&mut self) -> Vec<Cmd> {
        self.search_active = false;
        if self.mode == Mode::Search {
            self.mode = Mode::Normal;
            self.blur_after_search = false;
        }
        self.with_key_set(vec![])
    }

    /// Records a focus change the host reported for the page. Losing focus
    /// while a query is typed asks the page to end the search; gaining focus
    /// changes nothing else.
    pub fn on_focus_change(&mut self, focused: bool) -> Vec<Cmd> {
        self.page_focused = focused;
        let cmds = if !focused && self.mode == Mode::Search {
            vec![Cmd::SearchResolve]
        } else {
            vec![]
        };
        self.with_key_set(cmds)
    }

    fn cancel_search(&mut self, cause: SearchCause) -> Vec<Cmd> {
        self.mode = Mode::Normal;
        self.search_active = false;
        let mut cmds = vec![Cmd::SearchCancel];
        cmds.extend(self.release_after_search(cause));
        cmds
    }

    /// `Blur` when a key ended a search that began without page focus; clears the flag.
    fn release_after_search(&mut self, cause: SearchCause) -> Option<Cmd> {
        let blur = mem::take(&mut self.blur_after_search);
        (blur && cause == SearchCause::Key).then_some(Cmd::Blur)
    }

    /// Puts `SetForwardKeys` first in `cmds` when the mode needs a different
    /// forward-key set than the page carries.
    fn with_key_set(&mut self, mut cmds: Vec<Cmd>) -> Vec<Cmd> {
        let wanted = KeySet::of(self.mode);
        if wanted != self.key_set {
            self.key_set = wanted;
            cmds.insert(0, Cmd::SetForwardKeys(wanted));
        }
        cmds
    }

    fn resolve_chord(&mut self, c: char) -> Vec<Cmd> {
        match c {
            'g' => vec![Cmd::Scroll(ScrollAction::Top)],
            ']' => self.heading_jump(true),
            '[' => self.heading_jump(false),
            _ => vec![],
        }
    }

    fn heading_jump(&self, forward: bool) -> Vec<Cmd> {
        if self.outline.is_empty() {
            return vec![];
        }
        let last = self.outline.len() - 1;
        let target = match (self.current_heading_index, forward) {
            (None, _) => 0,
            (Some(i), true) => (i + 1).min(last),
            (Some(i), false) => i.saturating_sub(1),
        };
        if Some(target) == self.current_heading_index {
            return vec![];
        }
        vec![Cmd::ScrollToHeading(target)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::KeySet;
    use crate::protocol::SearchCause;

    fn app_with_outline(n: usize) -> App {
        let mut app = App::default();
        app.set_outline(
            (0..n)
                .map(|i| Heading {
                    level: 1,
                    text: format!("h{i}"),
                })
                .collect(),
        );
        app
    }

    #[test]
    fn gg_chord_scrolls_to_top() {
        let mut app = App::default();
        assert_eq!(app.on_action(Action::Prefix('g')), vec![]);
        assert_eq!(
            app.on_action(Action::Prefix('g')),
            vec![Cmd::Scroll(ScrollAction::Top)]
        );
    }

    #[test]
    fn dangling_prefix_then_other_key_is_cleared() {
        let mut app = App::default();
        assert_eq!(app.on_action(Action::Prefix('g')), vec![]);
        assert_eq!(
            app.on_action(Action::ScrollLineDown),
            vec![Cmd::Scroll(ScrollAction::Down)]
        );
    }

    #[test]
    fn bracket_chord_navigates_headings_from_current() {
        let mut app = app_with_outline(3);
        app.set_current_heading_index(Some(0));
        app.on_action(Action::Prefix(']'));
        assert_eq!(
            app.on_action(Action::Prefix(']')),
            vec![Cmd::ScrollToHeading(1)]
        );
        app.set_current_heading_index(Some(2));
        app.on_action(Action::Prefix('['));
        assert_eq!(
            app.on_action(Action::Prefix('[')),
            vec![Cmd::ScrollToHeading(1)]
        );
    }

    #[test]
    fn next_heading_from_none_goes_to_first() {
        let mut app = app_with_outline(3);
        app.on_action(Action::Prefix(']'));
        assert_eq!(
            app.on_action(Action::Prefix(']')),
            vec![Cmd::ScrollToHeading(0)]
        );
    }

    #[test]
    fn outline_toggle_move_and_confirm() {
        let mut app = app_with_outline(3);
        app.on_action(Action::ToggleOutline);
        assert_eq!(app.mode(), Mode::Outline);
        assert!(app.outline_open());
        app.on_action(Action::OutlineMoveDown);
        app.on_action(Action::OutlineMoveDown);
        assert_eq!(
            app.on_action(Action::OutlineConfirm),
            vec![Cmd::ScrollToHeading(2)]
        );
    }

    #[test]
    fn outline_move_is_clamped() {
        let mut app = app_with_outline(2);
        app.on_action(Action::ToggleOutline);
        app.on_action(Action::OutlineMoveUp);
        assert_eq!(app.selected(), 0);
        app.on_action(Action::OutlineMoveDown);
        app.on_action(Action::OutlineMoveDown);
        assert_eq!(app.selected(), 1);
    }

    #[test]
    fn quit_and_reload_passthrough() {
        let mut app = App::default();
        assert_eq!(app.on_action(Action::Quit), vec![Cmd::Quit]);
        assert_eq!(app.on_action(Action::Reload), vec![Cmd::Reload]);
    }

    #[test]
    fn escape_in_outline_mode_closes_panel() {
        let mut app = app_with_outline(3);
        app.on_action(Action::ToggleOutline);
        assert!(app.outline_open());
        assert_eq!(app.on_action(Action::Escape), vec![]);
        assert!(!app.outline_open());
        assert_eq!(app.mode(), Mode::Normal);
    }

    #[test]
    fn heading_jump_is_noop_at_boundaries() {
        let mut app = app_with_outline(3);
        app.set_current_heading_index(Some(2));
        app.on_action(Action::Prefix(']'));
        assert_eq!(app.on_action(Action::Prefix(']')), vec![]);
        app.set_current_heading_index(Some(0));
        app.on_action(Action::Prefix('['));
        assert_eq!(app.on_action(Action::Prefix('[')), vec![]);
    }

    #[test]
    fn back_action_emits_back_cmd() {
        let mut app = App::default();
        assert_eq!(app.on_action(Action::Back), vec![Cmd::Back]);
    }

    fn focused_app() -> App {
        let mut app = App::default();
        app.on_focus_change(true);
        app
    }

    fn submitted(app: &mut App) {
        app.on_action(Action::EnterSearch);
        app.on_action(Action::PageSearchSubmit(SearchCause::Key));
    }

    /// Asserts that `/` empties the forward keys, focuses the page, and starts typing.
    ///
    /// Case: the user presses `/` before ever clicking the page.
    #[test]
    fn entering_search_focuses_the_page_with_no_forward_keys() {
        let mut app = App::default();
        assert_eq!(
            app.on_action(Action::EnterSearch),
            vec![Cmd::SetForwardKeys(KeySet::Search), Cmd::Focus]
        );
        assert_eq!(app.search_stage(), SearchStage::Typing);
    }

    /// Asserts that the focus echo of `/` changes nothing, and that a key submit
    /// then restores the forward keys and blurs a page that was unfocused at `/`.
    ///
    /// Case: the user presses `/` without having clicked the page, types a query, and presses Enter.
    #[test]
    fn a_key_submit_blurs_a_page_that_was_unfocused_at_slash() {
        let mut app = App::default();
        app.on_action(Action::EnterSearch);
        assert_eq!(app.on_focus_change(true), vec![]);
        assert_eq!(app.mode(), Mode::Search);
        assert_eq!(
            app.on_action(Action::PageSearchSubmit(SearchCause::Key)),
            vec![Cmd::SetForwardKeys(KeySet::Normal), Cmd::Blur]
        );
        assert_eq!(app.search_stage(), SearchStage::Active);
    }

    /// Asserts that a submit leaves focus on a page that already had it at `/`.
    ///
    /// Case: the user clicks the page, then searches and presses Enter.
    #[test]
    fn a_submit_keeps_focus_on_a_page_that_had_it() {
        let mut app = focused_app();
        app.on_action(Action::EnterSearch);
        assert_eq!(
            app.on_action(Action::PageSearchSubmit(SearchCause::Key)),
            vec![Cmd::SetForwardKeys(KeySet::Normal)]
        );
    }

    /// Asserts that a submit caused by a blur never blurs the page.
    ///
    /// Case: the user types a query and then clicks the document body.
    #[test]
    fn a_blur_submit_never_blurs() {
        let mut app = App::default();
        app.on_action(Action::EnterSearch);
        assert_eq!(
            app.on_action(Action::PageSearchSubmit(SearchCause::Blur)),
            vec![Cmd::SetForwardKeys(KeySet::Normal)]
        );
    }

    /// Asserts that escaping a typed search cancels it and returns focus as it was.
    ///
    /// Case: the user presses `/`, types, and presses Esc without having clicked the page.
    #[test]
    fn escaping_a_typed_search_cancels_it() {
        let mut app = App::default();
        app.on_action(Action::EnterSearch);
        assert_eq!(
            app.on_action(Action::PageSearchEscape(SearchCause::Key)),
            vec![
                Cmd::SetForwardKeys(KeySet::Normal),
                Cmd::SearchCancel,
                Cmd::Blur
            ]
        );
        assert_eq!(app.search_stage(), SearchStage::Closed);
    }

    /// Asserts that cancelling a second search also drops the first, confirmed one
    /// rather than keeping it.
    ///
    /// Case: the user confirms a search, starts another with `/`, and presses Esc.
    #[test]
    fn escaping_a_second_search_drops_the_first() {
        let mut app = App::default();
        submitted(&mut app);
        app.on_action(Action::EnterSearch);
        let cmds = app.on_action(Action::Escape);
        assert!(cmds.contains(&Cmd::SearchCancel));
        assert_eq!(app.search_stage(), SearchStage::Closed);
    }

    /// Asserts that page search reports outside a typed search are ignored.
    ///
    /// Case: the page sends a second report for the same search (Enter, then the
    /// input's blur), or a late report while the outline is open.
    #[test]
    fn page_search_reports_outside_a_typed_search_do_nothing() {
        let mut app = App::default();
        submitted(&mut app);
        assert_eq!(
            app.on_action(Action::PageSearchSubmit(SearchCause::Blur)),
            vec![]
        );
        assert_eq!(app.search_stage(), SearchStage::Active);

        let mut app = app_with_outline(2);
        app.on_action(Action::ToggleOutline);
        assert_eq!(
            app.on_action(Action::PageSearchEscape(SearchCause::Key)),
            vec![]
        );
        assert!(app.outline_open());
    }

    /// Asserts that losing focus while typing asks the page to end the search.
    ///
    /// Case: the user types a query and then clicks another pane.
    #[test]
    fn losing_focus_while_typing_asks_the_page_to_resolve() {
        let mut app = focused_app();
        app.on_action(Action::EnterSearch);
        assert_eq!(app.on_focus_change(false), vec![Cmd::SearchResolve]);
        assert_eq!(app.mode(), Mode::Search);
    }

    /// Asserts that keys reaching the TUI during a typed search are relayed to the page.
    ///
    /// Case: the user types right after `/`, before the page holds keyboard focus.
    #[test]
    fn keys_reaching_the_tui_while_typing_are_relayed() {
        let mut app = App::default();
        app.on_action(Action::EnterSearch);
        assert_eq!(
            app.on_action(Action::SearchChar('a')),
            vec![Cmd::SearchType('a')]
        );
        assert_eq!(
            app.on_action(Action::SearchBackspace),
            vec![Cmd::SearchBackspace]
        );
        assert_eq!(app.on_action(Action::SearchConfirm), vec![Cmd::SearchEnter]);
    }

    /// Asserts that leaving the document mid-search restores the reading forward keys once.
    ///
    /// Case: the user is typing a query and clicks a link to another Markdown file.
    #[test]
    fn leaving_the_document_mid_search_restores_the_forward_keys() {
        let mut app = App::default();
        app.on_action(Action::EnterSearch);
        assert_eq!(
            app.clear_search_state(),
            vec![Cmd::SetForwardKeys(KeySet::Normal)]
        );
        assert_eq!(app.mode(), Mode::Normal);
        assert_eq!(app.clear_search_state(), vec![]);
    }

    /// Asserts that `n` navigates only while a confirmed search is active.
    ///
    /// Case: the user presses `n` before and after confirming a search.
    #[test]
    fn n_navigates_only_when_search_active() {
        let mut app = App::default();
        assert_eq!(app.on_action(Action::SearchNext), vec![]);
        submitted(&mut app);
        assert_eq!(
            app.on_action(Action::SearchNext),
            vec![Cmd::SearchNav(SearchDir::Next)]
        );
    }

    /// Asserts that Esc clears a confirmed search.
    ///
    /// Case: the user confirms a search, reads the matches, and presses Esc.
    #[test]
    fn escape_clears_an_active_search() {
        let mut app = App::default();
        submitted(&mut app);
        assert_eq!(app.on_action(Action::Escape), vec![Cmd::ClearSearch]);
        assert_eq!(app.search_stage(), SearchStage::Closed);
    }

    /// Asserts that an action that keeps the mode does not resend the forward keys.
    ///
    /// Case: the user scrolls with `j` while reading.
    #[test]
    fn actions_that_keep_the_mode_do_not_resend_forward_keys() {
        let mut app = App::default();
        assert_eq!(
            app.on_action(Action::ScrollLineDown),
            vec![Cmd::Scroll(ScrollAction::Down)]
        );
    }

    /// Asserts that the close button clears a confirmed search and does nothing
    /// while a query is typed.
    ///
    /// Case: the user confirms a search and clicks the find box's close button.
    #[test]
    fn the_close_button_clears_a_confirmed_search() {
        let mut app = App::default();
        app.on_action(Action::EnterSearch);
        assert_eq!(app.on_action(Action::PageSearchClose), vec![]);
        app.on_action(Action::PageSearchSubmit(SearchCause::Key));
        assert_eq!(
            app.on_action(Action::PageSearchClose),
            vec![Cmd::ClearSearch]
        );
        assert_eq!(app.search_stage(), SearchStage::Closed);
    }

    /// Asserts that a search abandoned by a blur is cancelled without blurring the page.
    ///
    /// Case: the user confirms a search, starts another with `/`, types a query with
    /// no match, and clicks the document body.
    #[test]
    fn a_blur_escape_cancels_without_blurring() {
        let mut app = App::default();
        submitted(&mut app);
        app.on_action(Action::EnterSearch);
        assert_eq!(
            app.on_action(Action::PageSearchEscape(SearchCause::Blur)),
            vec![Cmd::SetForwardKeys(KeySet::Normal), Cmd::SearchCancel]
        );
        assert_eq!(app.search_stage(), SearchStage::Closed);
    }

    /// Asserts that a focus change outside the search leaves the mode as it
    /// was.
    ///
    /// Case: the echo of the app's own focus request arrives while the
    /// outline panel is open.
    #[test]
    fn a_focus_change_outside_search_keeps_the_mode() {
        let mut app = app_with_outline(2);
        app.on_action(Action::ToggleOutline);
        app.on_focus_change(true);
        assert_eq!(app.mode(), Mode::Outline);
        app.on_focus_change(false);
        assert_eq!(app.mode(), Mode::Outline);
    }
}
