//! Minimal remote-URL webview demo. Run inside an ozmux pane:
//! `cargo run -p ratatui-ozma --example ratatui_remote_url`.
//!
//! Mounts a display-only remote page (no `window.ozmux` bridge — `Webview::url`
//! defaults to display-only) filling the pane, and quits on `q`. This is the
//! manual end-to-end check for the `url` content source: the remote page should
//! render inline where the widget is placed.
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event, KeyCode};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::layout::{Constraint, Layout};
use ratatui::widgets::{Block, Paragraph};
use ratatui_ozma::{Ozma, OzmaBackend, Webview, WebviewWidget};
use std::io::stdout;
use std::time::Duration;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let ozma = Ozma::connect()?;
    let view = ozma.register(Webview::url("https://github.com/not-elm?tab=repositories"))?;

    enable_raw_mode()?;
    if let Err(e) = execute!(stdout(), EnterAlternateScreen) {
        // NOTE: EnterAlternateScreen failed after raw mode was enabled — undo it so
        // the shell isn't left in raw mode.
        let _ = disable_raw_mode();
        return Err(e.into());
    }

    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let backend = OzmaBackend::new(CrosstermBackend::new(stdout()), &ozma);
        let mut terminal = Terminal::new(backend)?;
        loop {
            terminal.draw(|f| {
                let rows =
                    Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(f.area());
                f.render_widget(Paragraph::new("remote url demo · q to quit"), rows[0]);
                f.render_stateful_widget(
                    WebviewWidget::new(view.id()).fallback(Block::bordered().title("loading…")),
                    rows[1],
                    &mut *ozma.frame(),
                );
            })?;

            if event::poll(Duration::from_millis(50))?
                && let Event::Key(k) = event::read()?
                && k.code == KeyCode::Char('q')
            {
                return Ok(());
            }
        }
    })();

    let _ = disable_raw_mode();
    let _ = execute!(stdout(), LeaveAlternateScreen);
    result
}
