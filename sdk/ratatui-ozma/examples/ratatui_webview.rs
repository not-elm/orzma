//! Run inside an ozmux pane: `cargo run -p ratatui-ozma --example ratatui_webview`.
//!
//! Renders a webview and a native status line. The app OWNS focus in a simple
//! `web_focused` bool: `Alt+l` focuses the webview, `Alt+h` returns focus to the
//! app, and `q` quits while the app is focused. `Alt+h`/`Alt+l` are declared as
//! passthrough chords, so the host forwards them to the PTY (reaching plain
//! `event::read`) and suppresses them from the page even while the webview is
//! focused. `WebviewWidget::focused` tells the SDK the current focus; the
//! `OzmaBackend` wrapping the terminal emits the control-plane focus op on change.
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::layout::{Constraint, Layout};
use ratatui::widgets::{Block, Paragraph};
use ratatui_ozma::{KeyChord, Ozma, OzmaBackend, RpcError, Webview, WebviewHandle, WebviewWidget};
use std::io::Stdout;
use std::io::stdout;
use std::time::{Duration, Instant};

type Backend = OzmaBackend<CrosstermBackend<Stdout>>;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut ozma = Ozma::connect()?;
    let html = concat!(
        "<body style='background:#13131a;color:#8be9fd;font:16px sans-serif;margin:0;padding:8px'>",
        "<h1>ratatui-ozma</h1><div id='out'>calling ping…</div><div id='tick'>no ticks</div>",
        "<input id='in' placeholder='type here…' style='font:14px monospace;padding:6px'>",
        "<script>",
        "window.ozma.call('ping','hi').then(v=>out.textContent='ping → '+v);",
        "window.ozma.on('tick',n=>tick.textContent='tick #'+n);",
        "document.getElementById('in').focus();",
        "</script></body>"
    );

    let view = ozma.register(
        Webview::inline(html)
            .passthrough([
                KeyChord {
                    mods: KeyModifiers::ALT,
                    code: KeyCode::Char('h'),
                },
                KeyChord {
                    mods: KeyModifiers::ALT,
                    code: KeyCode::Char('l'),
                },
            ])
            .on("ping", |arg: String| {
                Ok::<_, RpcError>(format!("pong:{arg}"))
            }),
    )?;

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
        run(&mut terminal, &mut ozma, &view)
    })();

    let _ = disable_raw_mode();
    let _ = execute!(stdout(), LeaveAlternateScreen);
    result
}

fn run(
    terminal: &mut Terminal<Backend>,
    ozma: &mut Ozma,
    view: &WebviewHandle,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut web_focused = false;
    let mut n: u64 = 0;
    let mut last = Instant::now();
    loop {
        terminal.draw(|f| {
            let rows =
                Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(f.area());
            f.render_widget(
                Paragraph::new("Alt+l focus webview · Alt+h leave · q quit (when not focused)"),
                rows[0],
            );
            f.render_stateful_widget(
                WebviewWidget::new(view.id())
                    .focused(web_focused)
                    .fallback(Block::bordered().title("loading…")),
                rows[1],
                &mut *ozma.frame(),
            );
        })?;

        if last.elapsed() >= Duration::from_secs(1) {
            n += 1;
            let _ = view.emit("tick", &n);
            last = Instant::now();
        }

        if event::poll(Duration::from_millis(50))?
            && let Event::Key(k) = event::read()?
        {
            match (k.modifiers, k.code) {
                (KeyModifiers::ALT, KeyCode::Char('l')) => web_focused = true,
                (KeyModifiers::ALT, KeyCode::Char('h')) => web_focused = false,
                (KeyModifiers::NONE, KeyCode::Char('q')) if !web_focused => return Ok(()),
                _ => {}
            }
        }
    }
}
