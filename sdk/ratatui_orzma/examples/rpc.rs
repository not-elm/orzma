//! Event round-trip between the app and a webview (no call/reply RPC, no focus).
//! Run inside an orzma pane: `cargo run -p ratatui_orzma --example rpc`.
//!
//! Two one-way event channels form a loop:
//! - app → page: the app emits a `tick` counter each second; the page's
//!   `window.orzma.on('tick', …)` shows it.
//! - page → app: the page's `setInterval` calls `window.orzma.emit('hello', …)`;
//!   the app drains `view.read_events::<Hello>()` into its status line.
//!
//! No keyboard focus is involved — the page's JS, `window.orzma.on`, and
//! `window.orzma.emit` all run regardless of focus, so the app keeps the keyboard
//! and `q` quits immediately. The widget is still rendered every frame: that is what
//! keeps the page MOUNTED, and both `emit` directions are mount-scoped (a no-op when
//! nothing is mounted).

#[path = "common/terminal.rs"]
mod common;

use ratatui::crossterm::event::{self, Event, KeyCode};
use ratatui::layout::{Constraint, Layout};
use ratatui::widgets::{Block, Paragraph};
use ratatui_orzma::{Orzma, Webview, WebviewWidget};
use std::error::Error;
use std::time::{Duration, Instant};

#[derive(serde::Deserialize)]
struct Hello {
    message: String,
}

const HTML: &str = include_str!("rpc.html");

fn main() -> Result<(), Box<dyn Error>> {
    let orzma = Orzma::connect()?;
    let view = orzma.register(Webview::inline(HTML).add_event::<Hello>("hello"))?;

    let mut last_msg = String::from("(none yet)");
    let mut n: u64 = 0;
    let mut last_tick = Instant::now();
    common::run(&orzma, |terminal| {
        loop {
            for Hello { message } in view.read_events::<Hello>() {
                last_msg = message;
            }

            terminal.draw(|f| {
                let rows =
                    Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(f.area());
                f.render_widget(
                    Paragraph::new(format!("rpc events · q to quit · last: {last_msg}")),
                    rows[0],
                );
                f.render_stateful_widget(
                    WebviewWidget::new(view.id()).fallback(Block::bordered().title("loading…")),
                    rows[1],
                    &mut *orzma.frame(),
                );
            })?;

            if last_tick.elapsed() >= Duration::from_secs(1) {
                n += 1;
                let _ = view.emit("tick", &n);
                last_tick = Instant::now();
            }

            if event::poll(Duration::from_millis(50))?
                && let Event::Key(k) = event::read()?
                && k.code == KeyCode::Char('q')
            {
                return Ok(());
            }
        }
    })
}
