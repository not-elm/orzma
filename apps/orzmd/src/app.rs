//! The pure App state machine. `on_action` is the single entry point; it
//! returns the side-effect [`Cmd`]s for `main.rs` to execute. No SDK or I/O here.

use crate::keymap::{Action, Mode};
use crate::outline::Heading;
use crate::protocol::{ScrollAction, SearchDir};

/// A side effect for `main.rs` to perform after `on_action`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Cmd {
    /// Scroll the page.
    Scroll(ScrollAction),
    /// Scroll heading `index` (the `id="h{index}"` anchor) into view.
    ScrollToHeading(usize),
    /// Run an in-page search for `query`.
    Search(String),
    /// Navigate to the next/previous search match.
    SearchNav(SearchDir),
    /// Clear the in-page search highlight.
    ClearSearch,
    /// Re-read the file from disk and push new content.
    Reload,
    /// Pop the navigation back stack.
    Back,
    /// Exit the app.
    Quit,
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
    search_query: String,
    search_active: bool,
}

impl App {
    /// The current input mode.
    pub(crate) fn mode(&self) -> Mode {
        self.mode
    }

    /// Whether the outline panel is open.
    pub(crate) fn outline_open(&self) -> bool {
        self.outline_open
    }

    /// The selected outline index.
    pub(crate) fn selected(&self) -> usize {
        self.outline_selected
    }

    /// The current search query buffer.
    pub(crate) fn query(&self) -> &str {
        &self.search_query
    }

    /// The headings to draw in the outline panel.
    pub(crate) fn outline(&self) -> &[Heading] {
        &self.outline
    }

    /// Replaces the outline (called after a (re)load), clamping the selection.
    pub(crate) fn set_outline(&mut self, outline: Vec<Heading>) {
        self.outline = outline;
        if self.outline_selected >= self.outline.len() {
            self.outline_selected = self.outline.len().saturating_sub(1);
        }
    }

    /// Records the heading index nearest the viewport top (from `scrollState`).
    pub(crate) fn set_current_heading_index(&mut self, index: Option<usize>) {
        self.current_heading_index = index;
    }

    /// Whether a search is active (matches highlighted, awaiting clear).
    pub(crate) fn search_active(&self) -> bool {
        self.search_active
    }

    /// Processes an [`Action`], returning the side effects to perform.
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
                self.mode = Mode::Search;
                self.search_query.clear();
                vec![]
            }
            Action::SearchChar(c) => {
                self.search_query.push(c);
                vec![]
            }
            Action::SearchBackspace => {
                self.search_query.pop();
                vec![]
            }
            Action::SearchConfirm => {
                self.mode = Mode::Normal;
                self.search_active = true;
                vec![Cmd::Search(self.search_query.clone())]
            }
            Action::SearchNext if self.search_active => vec![Cmd::SearchNav(SearchDir::Next)],
            Action::SearchPrev if self.search_active => vec![Cmd::SearchNav(SearchDir::Prev)],
            Action::SearchNext | Action::SearchPrev => vec![],
            Action::Escape => {
                self.outline_open = false;
                self.mode = Mode::Normal;
                if self.search_active {
                    self.search_active = false;
                    self.search_query.clear();
                    vec![Cmd::ClearSearch]
                } else {
                    vec![]
                }
            }
            Action::Ignore => vec![],
        }
    }

    /// Clears search state when the viewed document changes (matches/bar are stale).
    pub(crate) fn clear_search_state(&mut self) {
        self.search_active = false;
        self.search_query.clear();
        if self.mode == Mode::Search {
            self.mode = Mode::Normal;
        }
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
    fn search_flow_enters_types_confirms_returns_to_normal() {
        let mut app = App::default();
        app.on_action(Action::EnterSearch);
        assert_eq!(app.mode(), Mode::Search);
        app.on_action(Action::SearchChar('f'));
        app.on_action(Action::SearchChar('o'));
        assert_eq!(
            app.on_action(Action::SearchConfirm),
            vec![Cmd::Search("fo".into())]
        );
        assert_eq!(app.mode(), Mode::Normal);
    }

    #[test]
    fn n_navigates_only_when_search_active() {
        let mut app = App::default();
        assert_eq!(app.on_action(Action::SearchNext), vec![]);
        app.on_action(Action::EnterSearch);
        app.on_action(Action::SearchChar('x'));
        app.on_action(Action::SearchConfirm);
        assert_eq!(
            app.on_action(Action::SearchNext),
            vec![Cmd::SearchNav(SearchDir::Next)]
        );
    }

    #[test]
    fn escape_clears_active_search() {
        let mut app = App::default();
        app.on_action(Action::EnterSearch);
        app.on_action(Action::SearchChar('x'));
        app.on_action(Action::SearchConfirm);
        assert_eq!(app.on_action(Action::Escape), vec![Cmd::ClearSearch]);
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
    fn search_active_reflects_lifecycle() {
        let mut app = App::default();
        assert!(!app.search_active());
        app.on_action(Action::EnterSearch);
        app.on_action(Action::SearchChar('x'));
        app.on_action(Action::SearchConfirm);
        assert!(app.search_active());
        app.on_action(Action::Escape);
        assert!(!app.search_active());
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
}
