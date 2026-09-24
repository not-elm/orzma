//! Forwarding keys out of a focused webview. Run inside an orzma pane:
//! `cargo run -p ratatui_orzma --example forward_keys`.
//!
//! A webview plus a native status line, with app-owned focus in a `web_focused`
//! bool. `Alt+l` focuses the webview (bare keys then type into its input); `Alt+h`
//! returns focus to the app; `q` quits while the app is focused.
//!
//! Only `Alt+h` is declared as a forward-key, and that asymmetry is the point:
//! `Alt+h` is pressed WHILE the page holds keyboard focus, so without forwarding it
//! would be swallowed by the page and focus could never leave the webview — the host
//! forwards the declared chord to the app, and only to the app, so `event::read`
//! sees it even while the page is focused. `Alt+l` is pressed while the app still
//! owns the keyboard, so it already reaches `event::read` and needs no declaration.
//!
//! `WebviewHandle::focus` and `Orzma::blur` send the control-plane focus op, and
//! `WebviewHandle::read_focus_changes` reports a click that focused the page.

#[path = "common/terminal.rs"]
mod common;

use ratatui::crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::layout::{Constraint, Layout};
use ratatui::widgets::{Block, Paragraph};
use ratatui_orzma::{KeyChord, Orzma, Webview, WebviewWidget};
use std::error::Error;
use std::time::Duration;

const HTML: &str = include_str!("forward_keys.html");

fn main() -> Result<(), Box<dyn Error>> {
    let orzma = Orzma::connect()?;
    let view = orzma.register(Webview::inline(HTML).forward_keys([KeyChord {
        mods: KeyModifiers::ALT,
        code: KeyCode::Char('h'),
    }]))?;

    let mut web_focused = false;
    common::run(&orzma, |terminal| {
        loop {
            for change in view.read_focus_changes() {
                web_focused = change.focused;
            }

            terminal.draw(|f| {
                let rows =
                    Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(f.area());
                f.render_widget(
                    Paragraph::new(format!(
                        "Alt+l focus webview · Alt+h leave · q quit · focus: {}",
                        if web_focused { "webview" } else { "app" }
                    )),
                    rows[0],
                );
                f.render_stateful_widget(
                    WebviewWidget::new(view.instance_id())
                        .fallback(Block::bordered().title("webview")),
                    rows[1],
                    &mut *orzma.frame(),
                );
            })?;

            if event::poll(Duration::from_millis(50))?
                && let Event::Key(k) = event::read()?
            {
                match (k.modifiers, k.code) {
                    (KeyModifiers::ALT, KeyCode::Char('l')) => {
                        view.focus()?;
                    }
                    (KeyModifiers::ALT, KeyCode::Char('h')) => {
                        orzma.blur()?;
                    }
                    (KeyModifiers::NONE, KeyCode::Char('q')) if !web_focused => return Ok(()),
                    _ => {}
                }
            }
        }
    })
}
