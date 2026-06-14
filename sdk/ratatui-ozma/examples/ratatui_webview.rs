//! Run inside an ozmux pane: `cargo run -p ratatui-ozma --example ratatui_webview`.
//!
//! Renders a webview widget and a native status panel side-by-side in the
//! alternate screen. Replies to `ping`, emits a `tick` event every second, and
//! demonstrates a `FocusManager` ring: use the arrow keys to move focus between
//! the webview and the status panel, and press `q` to quit.
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event, KeyCode};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::layout::{Constraint, Layout};
use ratatui::widgets::{Block, Paragraph};
use ratatui_ozma::{
    FocusManager, NavKeymap, Ozma, RpcError, Webview, WebviewHandle, WebviewWidget, focusable,
};
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

    let mut focus = FocusManager::new();
    let view = ozma.register(focusable(
        Webview::inline(html).on("ping", |(arg,): (String,)| {
            Ok::<_, RpcError>(format!("pong:{arg}"))
        }),
        focus.signal_sender(),
    ))?;

    focus.add_webview_at("web", view.clone(), ratatui::layout::Rect::default());
    focus.add_native_at("status", ratatui::layout::Rect::default());

    enable_raw_mode()?;
    if let Err(e) = execute!(stdout(), EnterAlternateScreen) {
        // NOTE: EnterAlternateScreen failed after raw mode was enabled — undo it so
        // the shell isn't left in raw mode.
        let _ = disable_raw_mode();
        return Err(e.into());
    }

    let keymap = NavKeymap::arrows();
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
        let mut n: u64 = 0;
        let mut last = Instant::now();
        run(
            &mut terminal,
            &mut ozma,
            &view,
            &keymap,
            &mut focus,
            &mut n,
            &mut last,
        )
    })();

    let _ = disable_raw_mode();
    let _ = execute!(stdout(), LeaveAlternateScreen);
    result
}

fn run(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    ozma: &mut Ozma,
    view: &WebviewHandle,
    keymap: &NavKeymap,
    focus: &mut FocusManager,
    n: &mut u64,
    last: &mut Instant,
) -> Result<(), Box<dyn std::error::Error>> {
    loop {
        for sync in focus.drain() {
            sync.apply(ozma)?;
        }

        terminal.draw(|f| {
            let rows =
                Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(f.area());
            f.render_widget(Paragraph::new("←/→ to move focus, q to quit"), rows[0]);
            let cols =
                Layout::horizontal([Constraint::Percentage(60), Constraint::Min(0)]).split(rows[1]);
            focus.set_rect("web", cols[0]);
            focus.set_rect("status", cols[1]);
            let web_focused = focus.is_focused("web");
            let status_focused = focus.is_focused("status");
            f.render_stateful_widget(
                WebviewWidget::new(view.id())
                    .focused(web_focused)
                    .fallback(Block::bordered().title("loading…")),
                cols[0],
                ozma.frame(),
            );
            let style = if status_focused {
                ratatui::style::Style::default().fg(ratatui::style::Color::Yellow)
            } else {
                ratatui::style::Style::default()
            };
            f.render_widget(Paragraph::new("status panel").style(style), cols[1]);
        })?;
        ozma.flush(terminal)?;

        if last.elapsed() >= Duration::from_secs(1) {
            *n += 1;
            let _ = view.emit("tick", n);
            *last = Instant::now();
        }

        if event::poll(Duration::from_millis(50))?
            && let Event::Key(k) = event::read()?
        {
            if k.code == KeyCode::Char('q') {
                return Ok(());
            }
            if focus.focused_is_native()
                && let Some(dir) = keymap.match_key(&k)
            {
                focus.navigate(dir).apply(ozma)?;
            }
        }
    }
}
