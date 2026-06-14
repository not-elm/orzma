//! Run inside an ozmux pane: `cargo run -p ratatui-ozma --example ratatui_webview`.
//!
//! Renders a webview widget in the alternate screen, replies to `ping`, and
//! emits a `tick` event every second. Press `q` to quit.
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event, KeyCode};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::layout::{Constraint, Layout};
use ratatui::widgets::{Block, Paragraph};
use ratatui_ozma::{Ozma, RpcError, Webview, WebviewHandle, WebviewWidget};
use std::io::stdout;
use std::time::{Duration, Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut ozma = Ozma::connect()?;
    let html = concat!(
        "<body style='background:#13131a;color:#8be9fd;font:16px sans-serif;margin:0;padding:8px'>",
        "<h1>ratatui-ozma</h1><div id='out'>calling ping…</div><div id='tick'>no ticks</div>",
        "<script>",
        "window.ozmux.call('ping',['hi']).then(v=>out.textContent='ping → '+v);",
        "window.ozmux.on('tick',n=>tick.textContent='tick #'+n);",
        "</script></body>"
    );
    let view = ozma.register(Webview::inline(html).on("ping", |(arg,): (String,)| {
        Ok::<_, RpcError>(format!("pong:{arg}"))
    }))?;

    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;

    let mut n: u64 = 0;
    let mut last = Instant::now();
    let result = run(&mut terminal, &mut ozma, &view, &mut n, &mut last);

    disable_raw_mode()?;
    execute!(stdout(), LeaveAlternateScreen)?;
    result
}

fn run(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    ozma: &mut Ozma,
    view: &WebviewHandle,
    n: &mut u64,
    last: &mut Instant,
) -> Result<(), Box<dyn std::error::Error>> {
    loop {
        terminal.draw(|f| {
            let rows =
                Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(f.area());
            f.render_widget(Paragraph::new("press q to quit"), rows[0]);
            let cols =
                Layout::horizontal([Constraint::Percentage(60), Constraint::Min(0)]).split(rows[1]);
            f.render_stateful_widget(
                WebviewWidget::new(view.id()).fallback(Block::bordered().title("loading…")),
                cols[0],
                ozma.frame(),
            );
        })?;
        ozma.flush(terminal)?;

        if last.elapsed() >= Duration::from_secs(1) {
            *n += 1;
            let _ = view.emit("tick", n);
            *last = Instant::now();
        }
        if event::poll(Duration::from_millis(50))?
            && let Event::Key(k) = event::read()?
            && k.code == KeyCode::Char('q')
        {
            return Ok(());
        }
    }
}
