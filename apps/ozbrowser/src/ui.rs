//! Native ratatui chrome around the webview: status bar, address bar, help modal.

use crate::app::App;
use crate::keymap::Mode;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph};
use ratatui_ozma::{FramePlacements, WebviewWidget};

/// Draws the whole frame: status/address bar (1 row) + webview, with help modal overlay.
pub(crate) fn draw(
    frame: &mut Frame<'_>,
    placements: &mut FramePlacements,
    app: &App,
    handle_id: &str,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(frame.area());

    match app.mode() {
        Mode::Address => draw_address_bar(frame, chunks[0], app),
        _ => draw_status_bar(frame, chunks[0], app),
    }

    frame.render_stateful_widget(
        WebviewWidget::new(handle_id).focused(app.mode() == Mode::Insert),
        chunks[1],
        placements,
    );

    if app.mode() == Mode::Help {
        draw_help_modal(frame);
    }
}

fn draw_status_bar(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let mode_label = match app.mode() {
        Mode::Normal => "Normal",
        Mode::Insert => "Insert",
        Mode::Help => "Help",
        Mode::Address => "Address",
        Mode::Hint => "Hint",
    };
    let text = format!("[{mode_label}] {}", app.url());
    frame.render_widget(
        Paragraph::new(text).style(Style::default().add_modifier(Modifier::REVERSED)),
        area,
    );
}

fn draw_address_bar(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let text = format!("> {}_", app.address_buf());
    frame.render_widget(
        Paragraph::new(text).style(Style::default().fg(Color::Yellow)),
        area,
    );
}

fn draw_help_modal(frame: &mut Frame<'_>) {
    let area = centered_rect(62, 85, frame.area());
    let lines = vec![
        Line::from("  Normal Mode Shortcuts"),
        Line::from(""),
        Line::from("  j / ↓          scroll line down"),
        Line::from("  k / ↑          scroll line up"),
        Line::from("  Ctrl-d / Space  scroll half-page down"),
        Line::from("  Ctrl-u         scroll half-page up"),
        Line::from("  Ctrl-f / PgDn  scroll page down"),
        Line::from("  Ctrl-b / PgUp  scroll page up"),
        Line::from("  gg             scroll to top"),
        Line::from("  G              scroll to bottom"),
        Line::from("  H              history back"),
        Line::from("  L              history forward"),
        Line::from("  o / :          open address bar"),
        Line::from("  r              reload"),
        Line::from("  i              insert mode (focus webview)"),
        Line::from("  f              follow link / hint"),
        Line::from("  ?              this help"),
        Line::from("  q / Ctrl-c     quit"),
        Line::from(""),
        Line::from("  Esc / q        close help"),
    ];
    let style = Style::default().bg(Color::Black).fg(Color::White);
    frame.render_widget(
        Paragraph::new(lines)
            .style(style)
            .block(Block::bordered().title(" Help — ozbrowser ").style(style)),
        area,
    );
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
