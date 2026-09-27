//! Call/reply RPC between a webview and the app. Run inside an orzma pane:
//! `cargo run -p ratatui_orzma --example rpc`.
//!
//! The page calls two app methods through `window.orzma.call` once a second:
//! - `add` resolves with the sum of its two operands;
//! - `divide` rejects with an `RpcError` when the divisor is zero, which the
//!   page receives as a rejected Promise.
//!
//! Handlers run on the SDK's reader thread, not in the draw loop, so the count
//! of answered calls that the status line shows is shared through an atomic.
//! The view is registered with `.interactive(false)`: a click on the page never
//! takes keyboard focus, so `q` always reaches the app.

#[path = "common/terminal.rs"]
mod common;

use ratatui::crossterm::event::{self, Event, KeyCode};
use ratatui::layout::{Constraint, Layout};
use ratatui::widgets::{Block, Paragraph};
use ratatui_orzma::{Orzma, RpcError, Webview, WebviewWidget};
use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

#[derive(serde::Deserialize)]
struct Operands {
    a: f64,
    b: f64,
}

const HTML: &str = include_str!("rpc.html");

fn main() -> Result<(), Box<dyn Error>> {
    let answered = Arc::new(AtomicU64::new(0));

    let add_answered = Arc::clone(&answered);
    let add = move |Operands { a, b }: Operands| -> Result<f64, RpcError> {
        add_answered.fetch_add(1, Ordering::Relaxed);
        Ok(a + b)
    };

    let divide_answered = Arc::clone(&answered);
    let divide = move |Operands { a, b }: Operands| -> Result<f64, RpcError> {
        divide_answered.fetch_add(1, Ordering::Relaxed);
        if b == 0.0 {
            return Err(RpcError::new("division by zero"));
        }
        Ok(a / b)
    };

    let orzma = Orzma::connect()?;
    let view = orzma.register(
        Webview::inline(HTML)
            .interactive(false)
            .on("add", add)
            .on("divide", divide),
    )?;

    common::run(&orzma, |terminal| {
        loop {
            let calls = answered.load(Ordering::Relaxed);
            terminal.draw(|f| {
                let rows =
                    Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(f.area());
                f.render_widget(
                    Paragraph::new(format!("rpc · q to quit · calls answered: {calls}")),
                    rows[0],
                );
                f.render_stateful_widget(
                    WebviewWidget::new(view.instance_id())
                        .fallback(Block::bordered().title("loading…")),
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
