//! orzbrowser — a TUI browser for remote URLs in orzma panes.

mod app;
mod keymap;
mod ui;

use crate::app::{App, Cmd, ScrollAction};
use crate::keymap::KeySet;
use crossbeam_channel::{Receiver, Sender};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui_orzma::{Orzma, OrzmaBackend, OrzmaError, RpcError, Webview, WebviewHandle};
use std::io::stdout;
use std::ops::ControlFlow;
use std::time::Duration;

/// The Vimium-style link-hint engine, supplied to the URL webview as a preload
/// script. Runs after the host's `window.orzma` bridge, which it depends on.
const ORZMA_HINTS_JS: &str = include_str!("orzma_hints.js");

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
        eprintln!("orzbrowser: {e}");
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    let initial_url = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: orzbrowser <url>"))?;

    let orzma = Orzma::connect().map_err(|e| match e {
        OrzmaError::NotInPane(_) => {
            anyhow::anyhow!("{e}. Run orzbrowser inside an orzma pane.")
        }
        _ => anyhow::anyhow!("{e}"),
    })?;

    let (url_tx, url_rx) = crossbeam_channel::unbounded::<String>();
    let (hint_tx, hint_rx) = crossbeam_channel::unbounded::<HintOutcome>();
    let view = register_view(&orzma, &initial_url, url_tx, hint_tx)?;

    enable_raw_mode()?;
    if let Err(e) = execute!(stdout(), EnterAlternateScreen) {
        // NOTE: EnterAlternateScreen failed after raw mode was enabled — undo raw mode
        // to avoid leaving the shell in an unusable state.
        let _ = disable_raw_mode();
        return Err(e.into());
    }
    install_panic_hook();

    let result = event_loop(view, App::new(initial_url), &orzma, &url_rx, &hint_rx);

    let _ = disable_raw_mode();
    let _ = execute!(stdout(), LeaveAlternateScreen);
    result
}

fn event_loop(
    view: WebviewHandle,
    mut app: App,
    orzma: &Orzma,
    url_rx: &Receiver<String>,
    hint_rx: &Receiver<HintOutcome>,
) -> anyhow::Result<()> {
    let backend = OrzmaBackend::new(CrosstermBackend::new(stdout()), orzma);
    let mut terminal = Terminal::new(backend)?;

    loop {
        while let Ok(url) = url_rx.try_recv() {
            app.on_page_url_changed(url);
        }
        while let Ok(outcome) = hint_rx.try_recv() {
            for cmd in app.on_hint_result(&outcome.kind) {
                if run_cmd(cmd, &view, orzma)?.is_break() {
                    return Ok(());
                }
            }
            // A link hint reports its target URL so the host performs a
            // browser-initiated navigation (which builds back/forward history);
            // a page-side el.click() would record no back entry.
            if let Some(url) = outcome.url {
                view.navigate(url)?;
            }
        }
        if apply_focus_changes(&mut app, &view, orzma)?.is_break() {
            return Ok(());
        }

        terminal.draw(|f| {
            ui::draw(f, &mut orzma.frame(), &app, &view.instance_id());
        })?;

        if event::poll(Duration::from_millis(33))?
            && let Event::Key(key) = event::read()?
        {
            if apply_focus_changes(&mut app, &view, orzma)?.is_break() {
                return Ok(());
            }
            let action = keymap::map(app.mode(), key);
            for cmd in app.on_action(action) {
                if run_cmd(cmd, &view, orzma)?.is_break() {
                    return Ok(());
                }
            }
        }
    }
}

fn register_view(
    orzma: &Orzma,
    url: &str,
    url_tx: Sender<String>,
    hint_tx: Sender<HintOutcome>,
) -> anyhow::Result<WebviewHandle> {
    let view = orzma.register(
        Webview::url(url)
            .interactive(true)
            .forward_keys(keymap::forward_chords(KeySet::Normal))
            .preload([ORZMA_HINTS_JS])
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

/// Performs one [`Cmd`]; `Break` when the app should exit.
fn run_cmd(cmd: Cmd, view: &WebviewHandle, orzma: &Orzma) -> anyhow::Result<ControlFlow<()>> {
    match cmd {
        Cmd::Quit => return Ok(ControlFlow::Break(())),
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
            let _ = view.emit("hints:key", &serde_json::json!({ "key": c.to_string() }));
        }
        Cmd::HintBackspace => {
            let _ = view.emit("hints:key", &serde_json::json!({ "backspace": true }));
        }
        Cmd::HintHide => {
            let _ = view.emit("hints:hide", &serde_json::json!({}));
        }
        Cmd::SetForwardKeys(set) => {
            let _ = view.set_forward_keys(keymap::forward_chords(set));
        }
        Cmd::Focus => {
            let _ = view.focus();
        }
        Cmd::Blur => {
            let _ = orzma.blur();
        }
    }
    Ok(ControlFlow::Continue(()))
}

/// Applies the focus changes the host reported since the last call; `Break`
/// when a resulting [`Cmd`] exits the app.
fn apply_focus_changes(
    app: &mut App,
    view: &WebviewHandle,
    orzma: &Orzma,
) -> anyhow::Result<ControlFlow<()>> {
    for change in view.read_focus_changes() {
        for cmd in app.on_focus_change(change.focused) {
            if run_cmd(cmd, view, orzma)?.is_break() {
                return Ok(ControlFlow::Break(()));
            }
        }
    }
    Ok(ControlFlow::Continue(()))
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

#[cfg(test)]
mod tests {
    use super::ORZMA_HINTS_JS;

    #[test]
    fn hint_engine_asset_carries_the_protocol_handlers() {
        assert!(
            ORZMA_HINTS_JS.contains("hints:show"),
            "the moved hint engine must register the hints:show handler"
        );
        assert!(
            ORZMA_HINTS_JS.contains("hintResult"),
            "the moved hint engine must report via hintResult"
        );
    }
}
