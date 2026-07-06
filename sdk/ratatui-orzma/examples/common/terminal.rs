//! Shared terminal setup/teardown for the ratatui-orzma examples.

use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui_orzma::{Orzma, OrzmaBackend};
use std::error::Error;
use std::io::{Stdout, stdout};

/// The concrete backend the examples draw through: orzma wrapping crossterm.
pub(crate) type Backend = OrzmaBackend<CrosstermBackend<Stdout>>;

/// Runs `body` with a live orzma-backed terminal, restoring the terminal on exit.
pub(crate) fn run<F>(orzma: &Orzma, body: F) -> Result<(), Box<dyn Error>>
where
    F: FnOnce(&mut Terminal<Backend>) -> Result<(), Box<dyn Error>>,
{
    enable_raw_mode()?;
    let _guard = TerminalGuard;
    execute!(stdout(), EnterAlternateScreen)?;
    let backend = OrzmaBackend::new(CrosstermBackend::new(stdout()), orzma);
    let mut terminal = Terminal::new(backend)?;
    body(&mut terminal)
}

/// Restores the terminal (raw mode off, leave alternate screen) on drop, so
/// teardown runs unconditionally — including when a fallible setup step or `body`
/// errors or panics.
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), LeaveAlternateScreen);
    }
}
