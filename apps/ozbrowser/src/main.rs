//! ozbrowser — a TUI browser for remote URLs in ozmux panes.

mod app;
mod keymap;
mod ui;

use crate::app::{App, Cmd, ScrollAction};
use crossbeam_channel::{Receiver, Sender};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui_ozma::{KeyChord, Ozma, OzmaBackend, OzmaError, RpcError, Webview, WebviewHandle};
use std::io::stdout;
use std::time::Duration;

/// A hint activation reported by the page over `hintResult`: the outcome `kind`
/// (`navigated`/`clicked`/`focusedInput`/`empty`) plus, for a real http(s) link,
/// the URL to load via a host browser-initiated navigation (so back/forward
/// history is built — a page-side `el.click()` would not record a back entry).
struct HintOutcome {
    kind: String,
    url: Option<String>,
}

fn main() {
    if let Err(e) = run() {
        eprintln!("ozbrowser: {e}");
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    let initial_url = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: ozbrowser <url>"))?;

    let ozma = Ozma::connect().map_err(|e| match e {
        OzmaError::NotInPane(_) => {
            anyhow::anyhow!("{e}. Run ozbrowser inside an ozmux pane.")
        }
        _ => anyhow::anyhow!("{e}"),
    })?;

    let (url_tx, url_rx) = crossbeam_channel::unbounded::<String>();
    let (hint_tx, hint_rx) = crossbeam_channel::unbounded::<HintOutcome>();
    let view = register_view(&ozma, &initial_url, url_tx, hint_tx)?;

    enable_raw_mode()?;
    if let Err(e) = execute!(stdout(), EnterAlternateScreen) {
        // NOTE: EnterAlternateScreen failed after raw mode was enabled — undo raw mode
        // to avoid leaving the shell in an unusable state.
        let _ = disable_raw_mode();
        return Err(e.into());
    }
    install_panic_hook();

    let result = event_loop(view, App::new(initial_url), &ozma, &url_rx, &hint_rx);

    let _ = disable_raw_mode();
    let _ = execute!(stdout(), LeaveAlternateScreen);
    result
}

fn event_loop(
    view: WebviewHandle,
    mut app: App,
    ozma: &Ozma,
    url_rx: &Receiver<String>,
    hint_rx: &Receiver<HintOutcome>,
) -> anyhow::Result<()> {
    let backend = OzmaBackend::new(CrosstermBackend::new(stdout()), ozma);
    let mut terminal = Terminal::new(backend)?;

    loop {
        while let Ok(url) = url_rx.try_recv() {
            app.on_page_url_changed(url);
        }
        while let Ok(outcome) = hint_rx.try_recv() {
            app.on_hint_result(&outcome.kind);
            // A link hint reports its target URL so the host performs a
            // browser-initiated navigation (which builds back/forward history);
            // a page-side el.click() would record no back entry.
            if let Some(url) = outcome.url {
                view.navigate(url)?;
            }
        }

        terminal.draw(|f| {
            ui::draw(f, &mut ozma.frame(), &app, &view.id());
        })?;

        if event::poll(Duration::from_millis(33))?
            && let Event::Key(key) = event::read()?
        {
            let action = keymap::map(app.mode(), key);
            for cmd in app.on_action(action) {
                match cmd {
                    Cmd::Quit => return Ok(()),
                    Cmd::Navigate(url) => view.navigate(url)?,
                    Cmd::HistoryBack => view.go_back()?,
                    Cmd::HistoryForward => view.go_forward()?,
                    Cmd::Reload => view.reload()?,
                    Cmd::Scroll(action) => {
                        let _ = view.emit("scroll", &scroll_payload(action));
                    }
                    Cmd::HintShow => {
                        let _ = view.emit("hints:show", &serde_json::json!({}));
                    }
                    Cmd::HintKey(c) => {
                        let _ =
                            view.emit("hints:key", &serde_json::json!({ "key": c.to_string() }));
                    }
                    Cmd::HintBackspace => {
                        let _ = view.emit("hints:key", &serde_json::json!({ "backspace": true }));
                    }
                    Cmd::HintHide => {
                        let _ = view.emit("hints:hide", &serde_json::json!({}));
                    }
                }
            }
        }
    }
}

fn register_view(
    ozma: &Ozma,
    url: &str,
    url_tx: Sender<String>,
    hint_tx: Sender<HintOutcome>,
) -> anyhow::Result<WebviewHandle> {
    let forward = [
        KeyChord {
            mods: KeyModifiers::NONE,
            code: KeyCode::Esc,
        },
        KeyChord {
            mods: KeyModifiers::NONE,
            code: KeyCode::Char('j'),
        },
        KeyChord {
            mods: KeyModifiers::NONE,
            code: KeyCode::Down,
        },
        KeyChord {
            mods: KeyModifiers::NONE,
            code: KeyCode::Char('k'),
        },
        KeyChord {
            mods: KeyModifiers::NONE,
            code: KeyCode::Up,
        },
        KeyChord {
            mods: KeyModifiers::NONE,
            code: KeyCode::Char(' '),
        },
        KeyChord {
            mods: KeyModifiers::NONE,
            code: KeyCode::PageDown,
        },
        KeyChord {
            mods: KeyModifiers::NONE,
            code: KeyCode::PageUp,
        },
        KeyChord {
            mods: KeyModifiers::CONTROL,
            code: KeyCode::Char('d'),
        },
        KeyChord {
            mods: KeyModifiers::CONTROL,
            code: KeyCode::Char('u'),
        },
        KeyChord {
            mods: KeyModifiers::CONTROL,
            code: KeyCode::Char('f'),
        },
        KeyChord {
            mods: KeyModifiers::CONTROL,
            code: KeyCode::Char('b'),
        },
        KeyChord {
            mods: KeyModifiers::NONE,
            code: KeyCode::Char('g'),
        },
        KeyChord {
            mods: KeyModifiers::SHIFT,
            code: KeyCode::Char('g'),
        },
        KeyChord {
            mods: KeyModifiers::SHIFT,
            code: KeyCode::Char('h'),
        },
        KeyChord {
            mods: KeyModifiers::SHIFT,
            code: KeyCode::Char('l'),
        },
        KeyChord {
            mods: KeyModifiers::NONE,
            code: KeyCode::Char('o'),
        },
        KeyChord {
            mods: KeyModifiers::NONE,
            code: KeyCode::Char('r'),
        },
        KeyChord {
            mods: KeyModifiers::NONE,
            code: KeyCode::Char('i'),
        },
        KeyChord {
            mods: KeyModifiers::NONE,
            code: KeyCode::Char('q'),
        },
        KeyChord {
            mods: KeyModifiers::CONTROL,
            code: KeyCode::Char('c'),
        },
    ];
    let view = ozma.register(
        Webview::url(url)
            .interactive(true)
            .forward_keys(forward)
            .on(
                "urlChanged",
                move |args: serde_json::Value| -> Result<(), RpcError> {
                    if let Some(u) = args["url"].as_str() {
                        let _ = url_tx.send(u.to_owned());
                    }
                    Ok(())
                },
            )
            .on(
                "hintResult",
                move |args: serde_json::Value| -> Result<(), RpcError> {
                    if let Some(kind) = args["kind"].as_str() {
                        let _ = hint_tx.send(HintOutcome {
                            kind: kind.to_owned(),
                            url: args["url"].as_str().map(str::to_owned),
                        });
                    }
                    Ok(())
                },
            ),
    )?;
    Ok(view)
}

fn scroll_payload(action: ScrollAction) -> serde_json::Value {
    let name = match action {
        ScrollAction::Down => "down",
        ScrollAction::Up => "up",
        ScrollAction::HalfDown => "halfDown",
        ScrollAction::HalfUp => "halfUp",
        ScrollAction::PageDown => "pageDown",
        ScrollAction::PageUp => "pageUp",
        ScrollAction::Top => "top",
        ScrollAction::Bottom => "bottom",
    };
    serde_json::json!({ "action": name })
}

fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), LeaveAlternateScreen);
        prev(info);
    }));
}
