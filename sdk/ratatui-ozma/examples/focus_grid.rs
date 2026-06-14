//! Manual verification harness for app-owned focus movement.
//!
//! Run inside an ozmux pane:
//! `cargo run -p ratatui-ozma --example focus_grid`.
//!
//! Layout is a 2x2 grid — two native panels (NW, SE) and two webviews (NE, SW):
//!
//! ```text
//! +-------------+-------------+
//! | NW native   | NE webview  |
//! +-------------+-------------+
//! | SW webview  | SE native   |
//! +-------------+-------------+
//! ```
//!
//! What to verify:
//! - Initial focus is the NW native panel (its border is highlighted).
//! - `Alt+h/j/k/l` moves focus spatially across the grid (the highlight and the
//!   bottom `FOCUS →` readout follow the pressed direction's neighbour).
//! - Moving onto a webview tints its background and lets you TYPE into its input
//!   (proving bare keys reach the focused webview natively).
//! - `Alt+h/j/k/l` while a webview is focused escapes back out to a sibling
//!   (proving the page glue forwards the nav chord to the app).
//! - `q` quits — but only while a NATIVE panel is focused (a focused webview
//!   swallows `q` as page input, which is itself part of the verification).

use ratatui::Frame;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event, KeyCode};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Paragraph};
use ratatui_ozma::{FocusManager, Ozma, Webview, WebviewHandle, WebviewWidget, focusable};
use std::io::stdout;
use std::time::Duration;

const CELL_IDS: [&str; 4] = ["nw", "ne", "sw", "se"];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut ozma = Ozma::connect()?;
    let mut focus = FocusManager::new();

    let ne = ozma.register(focusable(
        Webview::inline(webview_html("NE webview", "#8be9fd")),
        focus.signal_sender(),
    ))?;
    let sw = ozma.register(focusable(
        Webview::inline(webview_html("SW webview", "#bd93f9")),
        focus.signal_sender(),
    ))?;

    // Registration order sets the initial focus to the first item: the NW
    // native panel. The host's FocusedWebview starts empty, so a native start
    // keeps app and host in step from frame one.
    focus.add_native_at("nw", Rect::default());
    focus.add_webview_at("ne", ne.clone(), Rect::default());
    focus.add_webview_at("sw", sw.clone(), Rect::default());
    focus.add_native_at("se", Rect::default());

    enable_raw_mode()?;
    if let Err(e) = execute!(stdout(), EnterAlternateScreen) {
        // EnterAlternateScreen failed after raw mode was enabled — undo it so the
        // shell isn't left in raw mode.
        let _ = disable_raw_mode();
        return Err(e.into());
    }

    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
        run(&mut terminal, &mut ozma, &mut focus, &ne, &sw)
    })();

    // Always restore the terminal; ignore teardown errors so the real outcome in
    // `result` surfaces rather than a cleanup error masking it.
    let _ = disable_raw_mode();
    let _ = execute!(stdout(), LeaveAlternateScreen);
    result
}

fn run(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    ozma: &mut Ozma,
    focus: &mut FocusManager,
    ne: &WebviewHandle,
    sw: &WebviewHandle,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut last_focus = String::new();
    loop {
        // Apply focus moves the page glue forwarded while a webview was focused.
        for sync in focus.drain() {
            sync.apply(ozma)?;
        }

        // Push the current focus state to the webviews so the focused one tints
        // its background (a reliable signal independent of OSR DOM focus events).
        let current = focused_id(focus);
        if current != last_focus {
            let _ = ne.emit("focus", &(current == "ne"));
            let _ = sw.emit("focus", &(current == "sw"));
            last_focus = current.clone();
        }

        terminal.draw(|f| draw(f, ozma, focus, ne, sw, &current))?;
        ozma.flush(terminal)?;

        if event::poll(Duration::from_millis(50))?
            && let Event::Key(k) = event::read()?
        {
            if k.code == KeyCode::Char('q') && focus.focused_is_native() {
                return Ok(());
            }
            if focus.focused_is_native()
                && let Some(dir) = FocusManager::nav_key(&k)
            {
                focus.navigate(dir).apply(ozma)?;
            }
        }
    }
}

fn draw(
    f: &mut Frame,
    ozma: &mut Ozma,
    focus: &mut FocusManager,
    ne: &WebviewHandle,
    sw: &WebviewHandle,
    current: &str,
) {
    let outer = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(f.area());
    f.render_widget(
        Paragraph::new("Alt+h/j/k/l: move focus   |   type into a focused webview   |   q: quit (on a native panel)"),
        outer[0],
    );

    let rows =
        Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)]).split(outer[1]);
    let top =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(rows[0]);
    let bottom =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(rows[1]);
    let (nw_area, ne_area, sw_area, se_area) = (top[0], top[1], bottom[0], bottom[1]);

    focus.set_rect("nw", nw_area);
    focus.set_rect("ne", ne_area);
    focus.set_rect("sw", sw_area);
    focus.set_rect("se", se_area);

    let nw_f = focus.is_focused("nw");
    let ne_f = focus.is_focused("ne");
    let sw_f = focus.is_focused("sw");
    let se_f = focus.is_focused("se");

    render_native(f, nw_area, "NW (native)", nw_f);
    render_native(f, se_area, "SE (native)", se_f);

    // Reuse a single frame placement collector for BOTH webviews: `Ozma::frame`
    // clears it, so calling it once per webview would drop the first placement.
    let frame = ozma.frame();
    f.render_stateful_widget(
        WebviewWidget::new(ne.id())
            .focused(ne_f)
            .fallback(focus_block("NE (webview)", ne_f)),
        ne_area,
        &mut *frame,
    );
    f.render_stateful_widget(
        WebviewWidget::new(sw.id())
            .focused(sw_f)
            .fallback(focus_block("SW (webview)", sw_f)),
        sw_area,
        frame,
    );

    f.render_widget(
        Paragraph::new(format!("FOCUS → {}", focus_label(current)))
            .style(Style::default().fg(Color::Yellow)),
        outer[2],
    );
}

/// The id of the currently-focused cell, or an empty string when none is.
fn focused_id(focus: &FocusManager) -> String {
    CELL_IDS
        .into_iter()
        .find(|id| focus.is_focused(id))
        .unwrap_or("")
        .to_owned()
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

/// A bordered block whose border + title light up yellow when focused.
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
        "focused — Alt+h/j/k/l to move, q to quit"
    } else {
        "native panel"
    };
    f.render_widget(Paragraph::new(hint), inner);
}

/// An inline page with an input (to prove keys reach a focused webview) that
/// tints its background when the app reports it focused via `window.ozmux.on`.
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
function setF(f){{document.body.style.background=f?'#16241a':'#10121a';if(f){{i.focus();}}}}\
i.focus();\
window.ozmux.on('focus',setF);\
window.addEventListener('focus',function(){{i.focus();}});\
</script></body>"
    )
}
