//! Minimal orzma webview render. Run inside an orzma pane:
//! `cargo run -p ratatui-orzma --example simple`.
//!
//! Registers a tiny inline HTML page and renders it as a ratatui widget filling
//! the pane below a one-line hint. This is the whole render path: connect →
//! register → draw. Press `q` to quit.

#[path = "common/terminal.rs"]
mod common;

use ratatui::crossterm::event::{self, Event, KeyCode};
use ratatui::layout::{Constraint, Layout};
use ratatui::widgets::{Block, Paragraph};
use ratatui_orzma::{Orzma, Webview, WebviewWidget};
use std::error::Error;
use std::time::Duration;

const HTML: &str = "<body style='margin:0;height:100vh;display:flex;align-items:center;justify-content:center;background:#13131a;color:#8be9fd;font:20px sans-serif'>Hello from an orzma webview</body>";

fn main() -> Result<(), Box<dyn Error>> {
    let orzma = Orzma::connect()?;
    let view = orzma.register(Webview::inline(HTML))?;
    common::run(&orzma, |terminal| {
        loop {
            terminal.draw(|f| {
                let rows =
                    Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(f.area());
                f.render_widget(Paragraph::new("simple webview · q to quit"), rows[0]);
                f.render_stateful_widget(
                    WebviewWidget::new(view.id()).fallback(Block::bordered().title("loading…")),
                    rows[1],
                    &mut *orzma.frame(),
                );
            })?;

            if event::poll(Duration::from_millis(50))?
                && let Event::Key(k) = event::read()?
                && k.code == KeyCode::Char('q')
            {
                return Ok(());
            }
        }
    })
}
