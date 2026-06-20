//! ozbrowser — a TUI browser for remote URLs in ozmux panes.

mod app;
mod history;
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
    let view = register_view(&ozma, &initial_url, url_tx.clone())?;

    enable_raw_mode()?;
    if let Err(e) = execute!(stdout(), EnterAlternateScreen) {
        // NOTE: EnterAlternateScreen failed after raw mode was enabled — undo raw mode
        // to avoid leaving the shell in an unusable state.
        let _ = disable_raw_mode();
        return Err(e.into());
    }
    install_panic_hook();

    let result = event_loop(view, App::new(initial_url), &ozma, url_tx, &url_rx);

    let _ = disable_raw_mode();
    let _ = execute!(stdout(), LeaveAlternateScreen);
    result
}

fn event_loop(
    mut view: WebviewHandle,
    mut app: App,
    ozma: &Ozma,
    url_tx: Sender<String>,
    url_rx: &Receiver<String>,
) -> anyhow::Result<()> {
    let backend = OzmaBackend::new(CrosstermBackend::new(stdout()), ozma);
    let mut terminal = Terminal::new(backend)?;

    loop {
        while let Ok(url) = url_rx.try_recv() {
            app.set_url(url);
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
                    Cmd::Navigate(url) => {
                        let url = app.navigate(url);
                        view = register_view(ozma, &url, url_tx.clone())?;
                    }
                    Cmd::HistoryBack => {
                        if let Some(url) = app.go_back() {
                            view = register_view(ozma, &url, url_tx.clone())?;
                        }
                    }
                    Cmd::HistoryForward => {
                        if let Some(url) = app.go_forward() {
                            view = register_view(ozma, &url, url_tx.clone())?;
                        }
                    }
                    Cmd::Reload => {
                        view = register_view(ozma, app.url(), url_tx.clone())?;
                    }
                    Cmd::Scroll(action) => {
                        let _ = view.emit("scroll", &scroll_payload(action));
                    }
                }
            }
        }
    }
}

// TODO: each call to register_view mints a new WebviewHandle registration that is never
// unregistered — the old handle is dropped but the server-side entry persists because the
// SDK has no unregister/Drop path yet. Fix this when the SDK exposes one.
fn register_view(ozma: &Ozma, url: &str, url_tx: Sender<String>) -> anyhow::Result<WebviewHandle> {
    let pass = [
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
    let view = ozma.register(Webview::url(url).interactive(true).passthrough(pass).on(
        "urlChanged",
        move |args: serde_json::Value| -> Result<(), RpcError> {
            if let Some(u) = args["url"].as_str() {
                let _ = url_tx.send(u.to_owned());
            }
            Ok(())
        },
    ))?;
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
