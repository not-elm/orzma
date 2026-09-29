//! Native ratatui parts of orzbrowser: where the two webviews sit, and the
//! help modal.

use crate::app::App;
use crate::keymap::Mode;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui_orzma::{FramePlacements, WebviewWidget};

/// Rows the chrome takes at the top of the pane.
const CHROME_ROWS: u16 = 2;

/// Width of the help modal's key column, in cells.
const HELP_KEY_WIDTH: usize = 16;

const HELP_BG: Color = Color::Rgb(0x16, 0x1b, 0x22);
const HELP_BORDER: Color = Color::Rgb(0x30, 0x36, 0x3d);
const HELP_KEY: Color = Color::Rgb(0xf0, 0xf6, 0xfc);
const HELP_TEXT: Color = Color::Rgb(0x91, 0x98, 0xa1);

/// The help modal's sections: a title, then key and description pairs.
const HELP_SECTIONS: &[(&str, &[(&str, &str)])] = &[
    (
        "Normal",
        &[
            ("j / ↓", "scroll line down"),
            ("k / ↑", "scroll line up"),
            ("Ctrl-d / Space", "scroll half-page down"),
            ("Ctrl-u", "scroll half-page up"),
            ("Ctrl-f / PgDn", "scroll page down"),
            ("Ctrl-b / PgUp", "scroll page up"),
            ("gg", "scroll to top"),
            ("G", "scroll to bottom"),
            ("H", "history back"),
            ("L", "history forward"),
            ("o / :", "open the address bar"),
            ("r", "reload"),
            ("i", "insert mode (type into the page)"),
            ("f", "follow a link (hints)"),
            ("?", "this help"),
            ("q / Ctrl-c", "quit"),
        ],
    ),
    (
        "Address bar",
        &[
            ("Enter", "open the address, or search for the words"),
            ("Esc", "cancel"),
            ("? words", "search even when the words look like an address"),
        ],
    ),
];

/// Where the chrome and the page sit in the pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PaneLayout {
    /// The chrome's rows at the top.
    pub(crate) chrome: Rect,
    /// The page's rows below the chrome; zero rows when the pane is too short.
    pub(crate) page: Rect,
}

impl PaneLayout {
    /// Gives the chrome the top two rows of `area`, or all of `area` when it
    /// is shorter, and the page the rest.
    pub fn split(area: Rect) -> Self {
        let [chrome, page] =
            Layout::vertical([Constraint::Length(CHROME_ROWS), Constraint::Min(0)]).areas(area);
        Self { chrome, page }
    }
}

/// Draws the chrome, the page, and in Help mode the help modal over the page,
/// and returns whether the page has any rows.
pub(crate) fn draw(
    frame: &mut Frame<'_>,
    placements: &mut FramePlacements,
    app: &App,
    chrome_instance: &str,
    page_instance: &str,
) -> bool {
    let layout = PaneLayout::split(frame.area());
    frame.render_stateful_widget(
        WebviewWidget::new(chrome_instance),
        layout.chrome,
        placements,
    );
    let page_placed = layout.page.height > 0;
    if page_placed {
        frame.render_stateful_widget(WebviewWidget::new(page_instance), layout.page, placements);
        if app.mode() == Mode::Help {
            draw_help_modal(frame, layout.page);
        }
    }
    page_placed
}

fn draw_help_modal(frame: &mut Frame<'_>, area: Rect) {
    let style = Style::default().bg(HELP_BG).fg(HELP_TEXT);
    let block = Block::bordered()
        .title(" orzbrowser help ")
        .border_style(Style::default().fg(HELP_BORDER))
        .style(style);
    frame.render_widget(
        Paragraph::new(help_lines()).style(style).block(block),
        centered_rect(62, 85, area),
    );
}

fn help_lines() -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for (title, entries) in HELP_SECTIONS {
        lines.push(Line::from(Span::styled(
            format!("  {title}"),
            Style::default().fg(HELP_KEY).add_modifier(Modifier::BOLD),
        )));
        for &(key, text) in entries.iter() {
            lines.push(help_row(key, text));
        }
        lines.push(Line::from(""));
    }
    lines.push(help_row("Esc / q", "close help"));
    lines
}

fn help_row(key: &str, text: &'static str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("  {key:<width$}", width = HELP_KEY_WIDTH),
            Style::default().fg(HELP_KEY),
        ),
        Span::styled(text, Style::default().fg(HELP_TEXT)),
    ])
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let pad_v = (100 - percent_y) / 2;
    let pad_h = (100 - percent_x) / 2;
    let vert = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(pad_v),
            Constraint::Percentage(percent_y),
            Constraint::Percentage(pad_v),
        ])
        .split(area);
    let horiz = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(pad_h),
            Constraint::Percentage(percent_x),
            Constraint::Percentage(pad_h),
        ])
        .split(vert[1]);
    horiz[1]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(height: u16) -> Rect {
        Rect {
            x: 0,
            y: 3,
            width: 80,
            height,
        }
    }

    /// Asserts that the chrome takes the top two rows and the page the rest,
    /// and that a short pane leaves the page no rows.
    ///
    /// Case: the pane is 24 rows tall, then shrunk to two rows and to one.
    #[test]
    fn the_chrome_takes_the_top_two_rows() {
        let tall = PaneLayout::split(rect(24));
        assert_eq!((tall.chrome.y, tall.chrome.height), (3, 2));
        assert_eq!((tall.page.y, tall.page.height), (5, 22));

        let two = PaneLayout::split(rect(2));
        assert_eq!((two.chrome.height, two.page.height), (2, 0));

        let one = PaneLayout::split(rect(1));
        assert_eq!((one.chrome.height, one.page.height), (1, 0));
    }

    /// Asserts that the help lists the address-bar keys.
    ///
    /// Case: the user opens the help to learn how to force a search.
    #[test]
    fn the_help_lists_the_address_bar_keys() {
        let text: Vec<String> = help_lines().iter().map(ToString::to_string).collect();
        assert!(text.iter().any(|line| line.contains("Address bar")));
        assert!(text.iter().any(|line| line.contains("? words")));
    }
}
