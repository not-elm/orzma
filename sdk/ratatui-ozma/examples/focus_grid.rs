//! Manual verification harness for app-owned focus movement (H-redesign).
//!
//! Run inside an ozmux pane: `cargo run -p ratatui-ozma --example focus_grid`.
//!
//! A 2x2 grid — two native panels (NW, SE) and two webviews (NE, SW). The app
//! OWNS focus in a `focused` string and moves it spatially with `Alt+h/j/k/l`.
//! The webview cells declare those chords as PASSTHROUGH, so the host forwards
//! them to the PTY (reaching plain `event::read`) and suppresses them from the
//! page even while a webview is focused — that is how focus can leave a focused
//! webview. `WebviewWidget::focused` reports focus to the SDK; the `OzmaBackend`
//! wrapping the terminal emits the control-plane focus op on change (the host gives
//! CEF focus to the focused webview, so bare keys type into its input).
//!
//! Verify:
//! - Initial focus is NW (highlighted border).
//! - `Alt+h/j/k/l` moves focus across the grid; the highlight + `FOCUS →` follow.
//! - On a webview cell, bare keys type into its input; `Alt+hjkl` still escapes.
//! - `q` quits only while a native panel is focused.
use ratatui::Frame;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Paragraph};
use ratatui_ozma::{KeyChord, Ozma, OzmaBackend, Webview, WebviewHandle, WebviewWidget};
use std::io::Stdout;
use std::io::stdout;
use std::time::Duration;

type Backend = OzmaBackend<CrosstermBackend<Stdout>>;

#[derive(Clone, Copy)]
enum Dir {
    Left,
    Down,
    Up,
    Right,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut ozma = Ozma::connect()?;
    let nav = [
        KeyChord {
            mods: KeyModifiers::ALT,
            code: KeyCode::Char('h'),
        },
        KeyChord {
            mods: KeyModifiers::ALT,
            code: KeyCode::Char('j'),
        },
        KeyChord {
            mods: KeyModifiers::ALT,
            code: KeyCode::Char('k'),
        },
        KeyChord {
            mods: KeyModifiers::ALT,
            code: KeyCode::Char('l'),
        },
    ];
    let ne =
        ozma.register(Webview::inline(webview_html("NE webview", "#8be9fd")).passthrough(nav))?;
    let sw =
        ozma.register(Webview::inline(webview_html("SW webview", "#bd93f9")).passthrough(nav))?;

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
        run(&mut terminal, &mut ozma, &ne, &sw)
    })();

    let _ = disable_raw_mode();
    let _ = execute!(stdout(), LeaveAlternateScreen);
    result
}

fn run(
    terminal: &mut Terminal<Backend>,
    ozma: &mut Ozma,
    ne: &WebviewHandle,
    sw: &WebviewHandle,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut focused = String::from("nw");
    let mut last_key = String::from("(none yet)");
    loop {
        terminal.draw(|f| draw(f, ozma, ne, sw, &focused, &last_key))?;

        if event::poll(Duration::from_millis(50))?
            && let Event::Key(k) = event::read()?
        {
            last_key = describe_key(&k);
            if k.modifiers == KeyModifiers::ALT
                && let Some(dir) = dir_from_code(k.code)
            {
                if let Some(next) = move_focus(&focused, dir) {
                    focused = next.to_owned();
                }
                continue;
            }
            if k.modifiers == KeyModifiers::NONE
                && k.code == KeyCode::Char('q')
                && is_native(&focused)
            {
                return Ok(());
            }
        }
    }
}

fn dir_from_code(code: KeyCode) -> Option<Dir> {
    match code {
        KeyCode::Char('h') => Some(Dir::Left),
        KeyCode::Char('j') => Some(Dir::Down),
        KeyCode::Char('k') => Some(Dir::Up),
        KeyCode::Char('l') => Some(Dir::Right),
        _ => None,
    }
}

fn move_focus(current: &str, dir: Dir) -> Option<&'static str> {
    Some(match (current, dir) {
        ("nw", Dir::Right) => "ne",
        ("nw", Dir::Down) => "sw",
        ("ne", Dir::Left) => "nw",
        ("ne", Dir::Down) => "se",
        ("sw", Dir::Up) => "nw",
        ("sw", Dir::Right) => "se",
        ("se", Dir::Left) => "sw",
        ("se", Dir::Up) => "ne",
        _ => return None,
    })
}

fn is_native(id: &str) -> bool {
    id == "nw" || id == "se"
}

fn draw(
    f: &mut Frame,
    ozma: &mut Ozma,
    ne: &WebviewHandle,
    sw: &WebviewHandle,
    focused: &str,
    last_key: &str,
) {
    let outer = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(f.area());
    f.render_widget(
        Paragraph::new(
            "Alt+h/j/k/l: move focus   |   type into a focused webview   |   q: quit (on a native panel)",
        ),
        outer[0],
    );

    let rows =
        Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)]).split(outer[1]);
    let top =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(rows[0]);
    let bottom =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(rows[1]);
    let (nw_area, ne_area, sw_area, se_area) = (top[0], top[1], bottom[0], bottom[1]);

    render_native(f, nw_area, "NW (native)", focused == "nw");
    render_native(f, se_area, "SE (native)", focused == "se");

    // NOTE: Ozma::frame clears the collector, so call it once before rendering both
    // webviews — calling it twice would drop the first placement.
    let mut frame = ozma.frame();
    f.render_stateful_widget(
        WebviewWidget::new(&ne.id())
            .focused(focused == "ne")
            .fallback(focus_block("NE (webview)", focused == "ne")),
        ne_area,
        &mut *frame,
    );
    f.render_stateful_widget(
        WebviewWidget::new(&sw.id())
            .focused(focused == "sw")
            .fallback(focus_block("SW (webview)", focused == "sw")),
        sw_area,
        &mut *frame,
    );

    f.render_widget(
        Paragraph::new(format!("FOCUS → {}", focus_label(focused)))
            .style(Style::default().fg(Color::Yellow)),
        outer[2],
    );
    f.render_widget(
        Paragraph::new(format!("last key: {last_key}")).style(Style::default().fg(Color::DarkGray)),
        outer[3],
    );
}

fn focus_label(id: &str) -> &'static str {
    match id {
        "nw" => "NW (native)",
        "ne" => "NE (webview)",
        "sw" => "SW (webview)",
        "se" => "SE (native)",
        _ => "(none)",
    }
}

fn describe_key(k: &KeyEvent) -> String {
    format!("{:?} mods={:?}", k.code, k.modifiers)
}

fn focus_block(title: &str, focused: bool) -> Block<'static> {
    let block = Block::bordered().title(title.to_owned());
    if focused {
        block
            .border_style(Style::default().fg(Color::Yellow))
            .title_style(Style::default().fg(Color::Yellow))
    } else {
        block
    }
}

fn render_native(f: &mut Frame, area: Rect, title: &str, focused: bool) {
    let block = focus_block(title, focused);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let hint = if focused {
        "focused — move with Alt+h/j/k/l, q to quit"
    } else {
        "native panel"
    };
    f.render_widget(Paragraph::new(hint), inner);
}

fn webview_html(label: &str, accent: &str) -> String {
    format!(
        "<body style='margin:0;height:100vh;box-sizing:border-box;background:#10121a;\
color:{accent};font:14px sans-serif;display:flex;flex-direction:column;gap:8px;padding:10px'>\
<div style='font-weight:700'>{label}</div>\
<div>type here — bare keys reach the focused webview:</div>\
<input id='in' placeholder='...' style='font:14px monospace;padding:6px;background:#1b1e2b;\
color:#e7e7ef;border:1px solid {accent};border-radius:4px'>\
<div style='opacity:.7'>Alt+h/j/k/l leaves this webview</div>\
<script>\
var i=document.getElementById('in');\
i.focus();\
window.addEventListener('focus',function(){{i.focus();}});\
</script></body>"
    )
}
