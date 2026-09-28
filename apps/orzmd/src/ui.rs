//! Native ratatui chrome around the webview preview: the outline panel and the search line.

use crate::app::App;
use crate::keymap::Mode;
use crate::protocol::SearchCount;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui_orzma::{FramePlacements, WebviewWidget};

/// Draws the whole frame: the optional outline panel beside the webview, and
/// the optional search line.
pub(crate) fn draw(
    frame: &mut Frame<'_>,
    placements: &mut FramePlacements,
    app: &App,
    instance_id: &str,
    search: Option<SearchCount>,
) {
    let search_open = app.mode() == Mode::Search || app.search_active();
    let vchunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(if search_open { 1 } else { 0 }),
        ])
        .split(frame.area());

    draw_body(frame, placements, vchunks[0], app, instance_id);
    if search_open {
        draw_search(frame, vchunks[1], app, search);
    }
}

fn draw_body(
    frame: &mut Frame<'_>,
    placements: &mut FramePlacements,
    area: Rect,
    app: &App,
    instance_id: &str,
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
    frame.render_stateful_widget(WebviewWidget::new(instance_id), webview_area, placements);
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

fn draw_search(frame: &mut Frame<'_>, area: Rect, app: &App, search: Option<SearchCount>) {
    let count = match search {
        Some(c) if c.total > 0 => format!("   {}/{}", c.current, c.total),
        _ => String::new(),
    };
    frame.render_widget(Paragraph::new(format!("/{}{count}", app.query())), area);
}
