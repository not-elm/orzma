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
//! Navigation scheme is chosen by `FOCUS_NAV` (default `arrows`):
//! - `arrows` — bare Arrow keys move focus (most reliable across terminals).
//! - `alt` — `Alt+h/j/k/l` (the SDK's real default; needs the terminal to
//!   deliver the Alt modifier, which ozmux does not yet do for PTY keys — see
//!   the bottom debug line for what actually arrives).
//! - `ctrl` — `Ctrl+h/j/k/l`.
//!
//! The chosen scheme is pushed to the webviews via `__ozma.keys`, so the page
//! glue intercepts the same chord. The bottom debug line shows the raw key event
//! the app receives while a NATIVE panel is focused (a focused webview gets the
//! keys instead, so the debug line stays put — itself part of the verification).
//!
//! What to verify:
//! - Initial focus is the NW native panel (highlighted border).
//! - The nav chord moves focus spatially across the grid; the highlight and the
//!   `FOCUS →` readout follow the pressed direction's neighbour.
//! - Moving onto a webview tints its background and lets you TYPE into its input
//!   (bare keys reach the focused webview natively).
//! - The nav chord while a webview is focused escapes back out (the page glue
//!   forwards it to the app).
//! - `q` quits — but only while a NATIVE panel is focused.

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
use ratatui_ozma::{
    Direction, FocusManager, Ozma, Webview, WebviewHandle, WebviewWidget, focusable,
};
use std::io::stdout;
use std::time::Duration;

const CELL_IDS: [&str; 4] = ["nw", "ne", "sw", "se"];

/// Which key scheme drives focus movement.
#[derive(Clone, Copy, PartialEq, Eq)]
enum NavScheme {
    Arrows,
    Alt,
    Ctrl,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let scheme = nav_scheme();
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
        run(&mut terminal, &mut ozma, &mut focus, &ne, &sw, scheme)
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
    scheme: NavScheme,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut last_focus = String::new();
    let mut last_key = String::from("(none yet)");
    loop {
        // Apply focus moves the page glue forwarded while a webview was focused.
        for sync in focus.drain() {
            sync.apply(ozma)?;
        }

        // On focus change, push the focus tint AND the chosen nav keymap to the
        // webviews so the page glue intercepts the same chord this example uses.
        let current = focused_id(focus);
        if current != last_focus {
            let keymap = keymap_payload(scheme);
            let _ = ne.emit("focus", &(current == "ne"));
            let _ = sw.emit("focus", &(current == "sw"));
            let _ = ne.emit("__ozma.keys", &keymap);
            let _ = sw.emit("__ozma.keys", &keymap);
            last_focus = current.clone();
        }

        terminal.draw(|f| draw(f, ozma, focus, ne, sw, &current, scheme, &last_key))?;
        ozma.flush(terminal)?;

        if event::poll(Duration::from_millis(50))?
            && let Event::Key(k) = event::read()?
        {
            last_key = describe_key(&k);
            if k.code == KeyCode::Char('q') && focus.focused_is_native() {
                return Ok(());
            }
            if focus.focused_is_native()
                && let Some(dir) = match_nav(scheme, &k)
            {
                focus.navigate(dir).apply(ozma)?;
            }
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "a self-contained example draw fn"
)]
fn draw(
    f: &mut Frame,
    ozma: &mut Ozma,
    focus: &mut FocusManager,
    ne: &WebviewHandle,
    sw: &WebviewHandle,
    current: &str,
    scheme: NavScheme,
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
        Paragraph::new(format!(
            "{}: move focus   |   type into a focused webview   |   q: quit (on a native panel)",
            scheme_chord(scheme)
        )),
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
    f.render_widget(
        Paragraph::new(format!(
            "scheme={}   last native key: {}   (set FOCUS_NAV=arrows|alt|ctrl)",
            scheme_name(scheme),
            last_key
        ))
        .style(Style::default().fg(Color::DarkGray)),
        outer[3],
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

fn nav_scheme() -> NavScheme {
    match std::env::var("FOCUS_NAV").ok().as_deref() {
        Some("alt") => NavScheme::Alt,
        Some("ctrl") => NavScheme::Ctrl,
        _ => NavScheme::Arrows,
    }
}

fn scheme_name(scheme: NavScheme) -> &'static str {
    match scheme {
        NavScheme::Arrows => "arrows",
        NavScheme::Alt => "alt",
        NavScheme::Ctrl => "ctrl",
    }
}

fn scheme_chord(scheme: NavScheme) -> &'static str {
    match scheme {
        NavScheme::Arrows => "Arrow keys",
        NavScheme::Alt => "Alt+h/j/k/l",
        NavScheme::Ctrl => "Ctrl+h/j/k/l",
    }
}

/// The raw key event, for the debug line.
fn describe_key(k: &KeyEvent) -> String {
    format!("{:?} mods={:?}", k.code, k.modifiers)
}

/// Interprets a native-side key event into a focus [`Direction`] per `scheme`.
fn match_nav(scheme: NavScheme, k: &KeyEvent) -> Option<Direction> {
    let from_char = |c: char| match c.to_ascii_lowercase() {
        'h' => Some(Direction::Left),
        'j' => Some(Direction::Down),
        'k' => Some(Direction::Up),
        'l' => Some(Direction::Right),
        _ => None,
    };
    match scheme {
        NavScheme::Arrows => match k.code {
            KeyCode::Left => Some(Direction::Left),
            KeyCode::Down => Some(Direction::Down),
            KeyCode::Up => Some(Direction::Up),
            KeyCode::Right => Some(Direction::Right),
            _ => None,
        },
        NavScheme::Alt => match k.code {
            KeyCode::Char(c) if k.modifiers.contains(KeyModifiers::ALT) => from_char(c),
            _ => None,
        },
        NavScheme::Ctrl => match k.code {
            KeyCode::Char(c) if k.modifiers.contains(KeyModifiers::CONTROL) => from_char(c),
            _ => None,
        },
    }
}

/// The `__ozma.keys` payload that makes the page glue intercept `scheme`'s chord.
fn keymap_payload(scheme: NavScheme) -> serde_json::Value {
    match scheme {
        NavScheme::Arrows => serde_json::json!({
            "mods": [],
            "keys": {"arrowleft": "left", "arrowdown": "down", "arrowup": "up", "arrowright": "right"}
        }),
        NavScheme::Alt => serde_json::json!({
            "mods": ["alt"],
            "keys": {"h": "left", "j": "down", "k": "up", "l": "right"}
        }),
        NavScheme::Ctrl => serde_json::json!({
            "mods": ["ctrl"],
            "keys": {"h": "left", "j": "down", "k": "up", "l": "right"}
        }),
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
        "focused — move with the nav chord, q to quit"
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
<div style='opacity:.7'>the nav chord leaves this webview</div>\
<script>\
var i=document.getElementById('in');\
function setF(f){{document.body.style.background=f?'#16241a':'#10121a';if(f){{i.focus();}}}}\
i.focus();\
window.ozmux.on('focus',setF);\
window.addEventListener('focus',function(){{i.focus();}});\
</script></body>"
    )
}
