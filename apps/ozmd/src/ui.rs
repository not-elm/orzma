//! Native ratatui chrome around the webview preview.

use crate::app::App;
use crate::keymap::Mode;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;
use ratatui_ozma::{FramePlacements, WebviewWidget};

/// Whether live-reload is currently healthy.
#[derive(Debug, Clone, Copy)]
pub(crate) enum LiveStatus {
    /// Watching the file; updates flowing.
    Watching,
    /// The file is missing (deleted); last content retained.
    Missing,
}

/// Draws the whole frame: status line, optional outline panel + webview, and the
/// optional search line.
pub(crate) fn draw(
    frame: &mut Frame<'_>,
    placements: &mut FramePlacements,
    app: &App,
    handle_id: &str,
    file_name: &str,
    live: LiveStatus,
    scroll_percent: u16,
) {
    let search_open = app.mode() == Mode::Search;
    let vchunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(if search_open { 1 } else { 0 }),
        ])
        .split(frame.area());

    draw_status(frame, vchunks[0], file_name, live, scroll_percent);
    draw_body(frame, placements, vchunks[1], app, handle_id);
    if search_open {
        draw_search(frame, vchunks[2], app);
    }
}

fn draw_status(
    frame: &mut Frame<'_>,
    area: Rect,
    file_name: &str,
    live: LiveStatus,
    scroll_percent: u16,
) {
    let dot = match live {
        LiveStatus::Watching => "● live",
        LiveStatus::Missing => "○ missing",
    };
    let line = Line::from(format!("ozmd · {file_name}    {dot}    {scroll_percent}%"));
    frame.render_widget(
        Paragraph::new(line).style(Style::default().add_modifier(Modifier::REVERSED)),
        area,
    );
}

fn draw_body(
    frame: &mut Frame<'_>,
    placements: &mut FramePlacements,
    area: Rect,
    app: &App,
    handle_id: &str,
) {
    let webview_area = if app.outline_open() {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(28), Constraint::Min(1)])
            .split(area);
        draw_outline(frame, cols[0], app);
        cols[1]
    } else {
        area
    };
    frame.render_stateful_widget(
        WebviewWidget::new(handle_id).fallback(Block::bordered().title("loading…")),
        webview_area,
        placements,
    );
}

fn draw_outline(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let items: Vec<ListItem> = app
        .outline()
        .iter()
        .map(|h| {
            let indent = "  ".repeat(h.level.saturating_sub(1) as usize);
            ListItem::new(format!("{indent}{}", h.text))
        })
        .collect();
    let mut state = ListState::default();
    if !app.outline().is_empty() {
        state.select(Some(app.selected()));
    }
    let list = List::new(items)
        .block(Block::default().borders(Borders::RIGHT).title("Outline"))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_search(frame: &mut Frame<'_>, area: Rect, app: &App) {
    frame.render_widget(Paragraph::new(format!("/{}", app.query())), area);
}
