//! orzbrowser — a TUI browser for remote URLs in orzma panes.

mod address;
mod app;
mod assets;
mod chrome;
mod focus;
mod keymap;
mod protocol;
mod ui;

use crate::address::{AddressTarget, SearchEngine};
use crate::app::{App, Cmd, ScrollAction};
use crate::chrome::{Chrome, ChromeSync};
use crate::focus::{FocusDrain, Target};
use crate::protocol::{AddressRequest, PageEvent, Preview};
use anyhow::{anyhow, bail};
use crossbeam_channel::{Receiver, Sender, unbounded};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui_orzma::{Orzma, OrzmaBackend, OrzmaError, RpcError, Webview, WebviewHandle};
use serde_json::{Value, json};
use std::env;
use std::io::stdout;
use std::ops::ControlFlow;
use std::panic;
use std::process;
use std::time::Duration;

/// The Vimium-style link-hint engine, supplied to the page as a preload
/// script. Runs after the host's `window.orzma` bridge, which it depends on.
const ORZMA_HINTS_JS: &str = include_str!("orzma_hints.js");

/// A hint activation reported by the page over `hintResult`: the outcome
/// `kind` (`navigated`/`clicked`/`focusedInput`/`empty`) plus, for a real
/// http(s) link, the URL to load through a host navigation.
struct HintOutcome {
    kind: String,
    url: Option<String>,
}

/// The two registered webviews.
struct Views {
    page: WebviewHandle,
    chrome: WebviewHandle,
}

/// The receiving ends of the reports the webviews' handlers send.
struct Inbox {
    urls: Receiver<String>,
    hints: Receiver<HintOutcome>,
    ready: Receiver<()>,
    targets: Receiver<AddressTarget>,
}

impl Views {
    /// Registers the page and the chrome, and returns them with the receiving
    /// ends of their handlers' reports.
    fn register(
        orzma: &Orzma,
        app: &App,
        chrome_html: String,
        engine: &SearchEngine,
    ) -> anyhow::Result<(Self, Inbox)> {
        let (url_tx, urls) = unbounded();
        let (hint_tx, hints) = unbounded();
        let (ready_tx, ready) = unbounded();
        let (target_tx, targets) = unbounded();
        let page = register_page(orzma, app, url_tx, hint_tx)?;
        let chrome = register_chrome(orzma, app, chrome_html, engine, ready_tx, target_tx)?;
        Ok((
            Self { page, chrome },
            Inbox {
                urls,
                hints,
                ready,
                targets,
            },
        ))
    }

    fn of(&self, target: Target) -> &WebviewHandle {
        match target {
            Target::Page => &self.page,
            Target::Chrome => &self.chrome,
        }
    }
}

fn main() {
    if let Err(e) = run() {
        eprintln!("orzbrowser: {e}");
        process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    let engine = SearchEngine::from_env()?;
    let input = env::args_os()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ");
    let app = match AddressTarget::parse(&input, &engine) {
        AddressTarget::Empty => App::with_address_open(engine.home().to_owned()),
        AddressTarget::Open(url) | AddressTarget::Search(url) => App::new(url),
        AddressTarget::Invalid(reason) => bail!("{reason}"),
    };
    let chrome_html = assets::chrome_html()?;

    let orzma = Orzma::connect().map_err(|e| match e {
        OrzmaError::NotInPane(_) => {
            anyhow!("{e}. Run orzbrowser inside an orzma pane.")
        }
        _ => anyhow!("{e}"),
    })?;
    let (views, inbox) = Views::register(&orzma, &app, chrome_html, &engine)?;

    enable_raw_mode()?;
    if let Err(e) = execute!(stdout(), EnterAlternateScreen) {
        // NOTE: EnterAlternateScreen failed after raw mode was enabled — undo raw mode
        // to avoid leaving the shell in an unusable state.
        let _ = disable_raw_mode();
        return Err(e.into());
    }
    install_panic_hook();

    let result = event_loop(app, &views, &inbox, &orzma);

    let _ = disable_raw_mode();
    let _ = execute!(stdout(), LeaveAlternateScreen);
    result
}

/// Runs the app until it quits. Each pass reads the focus changes first, then
/// the webviews' reports, then one key.
fn event_loop(mut app: App, views: &Views, inbox: &Inbox, orzma: &Orzma) -> anyhow::Result<()> {
    let backend = OrzmaBackend::new(CrosstermBackend::new(stdout()), orzma);
    let mut terminal = Terminal::new(backend)?;
    let mut sync = ChromeSync::default();

    loop {
        if apply_focus_changes(&mut app, views, orzma)?.is_break() {
            return Ok(());
        }
        if apply_reports(&mut app, &mut sync, views, inbox, orzma)?.is_break() {
            return Ok(());
        }
        push_chrome(&mut sync, &views.chrome, &app);

        let mut page_placed = true;
        let chrome_instance = views.chrome.instance_id();
        let page_instance = views.page.instance_id();
        terminal.draw(|f| {
            page_placed = ui::draw(
                f,
                &mut orzma.frame(),
                &app,
                &chrome_instance,
                &page_instance,
            );
        })?;
        app.set_page_placed(page_placed);

        // NOTE: a key release is dropped here rather than mapped to
        // `Action::Ignore`: any action clears a pending `g`, so the release
        // ConPTY reports after each press would keep `gg` from completing.
        if event::poll(Duration::from_millis(33))?
            && let Event::Key(key) = event::read()?
            && key.kind != KeyEventKind::Release
        {
            if apply_focus_changes(&mut app, views, orzma)?.is_break() {
                return Ok(());
            }
            let action = keymap::map(app.mode(), key);
            if run_cmds(app.on_action(action), views, orzma)?.is_break() {
                return Ok(());
            }
        }
    }
}

/// Applies the focus changes both webviews reported since the last call;
/// `Break` when a resulting [`Cmd`] exits the app.
fn apply_focus_changes(
    app: &mut App,
    views: &Views,
    orzma: &Orzma,
) -> anyhow::Result<ControlFlow<()>> {
    // NOTE: read the page's focus changes before the chrome's; `FocusDrain`
    // relies on this order when both webviews end a pass on a gain.
    let drain = FocusDrain::from_changes(
        views.page.read_focus_changes(),
        views.chrome.read_focus_changes(),
    );
    run_cmds(app.on_focus_drain(drain), views, orzma)
}

/// Applies the reports the webviews sent since the last pass; `Break` when a
/// resulting [`Cmd`] exits the app.
fn apply_reports(
    app: &mut App,
    sync: &mut ChromeSync,
    views: &Views,
    inbox: &Inbox,
    orzma: &Orzma,
) -> anyhow::Result<ControlFlow<()>> {
    while inbox.ready.try_recv().is_ok() {
        sync.forget();
        if run_cmds(app.on_chrome_ready(), views, orzma)?.is_break() {
            return Ok(ControlFlow::Break(()));
        }
    }
    while let Ok(url) = inbox.urls.try_recv() {
        app.on_page_url_changed(url);
    }
    while let Ok(outcome) = inbox.hints.try_recv() {
        if run_cmds(app.on_hint_result(&outcome.kind), views, orzma)?.is_break() {
            return Ok(ControlFlow::Break(()));
        }
        // NOTE: a link hint reports its target URL so the host performs a
        // browser-initiated navigation, which builds back/forward history; a
        // page-side el.click() would record no back entry.
        if let Some(url) = outcome.url {
            views.page.navigate(url)?;
        }
    }
    for event in views.chrome.read_events::<PageEvent>() {
        if run_cmds(app.on_page_event(event), views, orzma)?.is_break() {
            return Ok(ControlFlow::Break(()));
        }
    }
    while let Ok(target) = inbox.targets.try_recv() {
        if run_cmds(app.on_address_target(target), views, orzma)?.is_break() {
            return Ok(ControlFlow::Break(()));
        }
    }
    Ok(ControlFlow::Continue(()))
}

/// Sends the chrome page the chrome when it changed since the last send.
fn push_chrome(sync: &mut ChromeSync, chrome_view: &WebviewHandle, app: &App) {
    let chrome = Chrome::build(app);
    if sync.is_stale(&chrome) && chrome_view.emit("chrome", &chrome).is_ok() {
        sync.mark_sent(chrome);
    }
}

fn register_page(
    orzma: &Orzma,
    app: &App,
    url_tx: Sender<String>,
    hint_tx: Sender<HintOutcome>,
) -> anyhow::Result<WebviewHandle> {
    let view = orzma.register(
        Webview::url(app.url())
            .interactive(true)
            .forward_keys(keymap::forward_chords(app.page_keys()))
            .preload([ORZMA_HINTS_JS])
            .on("urlChanged", move |args: Value| -> Result<(), RpcError> {
                if let Some(u) = args["url"].as_str() {
                    let _ = url_tx.send(u.to_owned());
                }
                Ok(())
            })
            .on("hintResult", move |args: Value| -> Result<(), RpcError> {
                if let Some(kind) = args["kind"].as_str() {
                    let _ = hint_tx.send(HintOutcome {
                        kind: kind.to_owned(),
                        url: args["url"].as_str().map(str::to_owned),
                    });
                }
                Ok(())
            }),
    )?;
    Ok(view)
}

fn register_chrome(
    orzma: &Orzma,
    app: &App,
    html: String,
    engine: &SearchEngine,
    ready_tx: Sender<()>,
    target_tx: Sender<AddressTarget>,
) -> anyhow::Result<WebviewHandle> {
    let preview_engine = engine.clone();
    let submit_engine = engine.clone();
    let view = orzma.register(
        Webview::inline(html)
            .interactive(true)
            .forward_keys(keymap::forward_chords(app.chrome_keys()))
            .on("ready", move |(): ()| -> Result<(), RpcError> {
                let _ = ready_tx.send(());
                Ok(())
            })
            .on(
                "preview",
                move |request: AddressRequest| -> Result<Preview, RpcError> {
                    let target = AddressTarget::parse(&request.text, &preview_engine);
                    Ok(Preview::of(&target, &preview_engine))
                },
            )
            .on(
                "submit",
                move |request: AddressRequest| -> Result<Preview, RpcError> {
                    let target = AddressTarget::parse(&request.text, &submit_engine);
                    let preview = Preview::of(&target, &submit_engine);
                    if !matches!(target, AddressTarget::Invalid(_)) {
                        let _ = target_tx.send(target);
                    }
                    Ok(preview)
                },
            )
            .add_event::<PageEvent>("page"),
    )?;
    Ok(view)
}

fn run_cmds(cmds: Vec<Cmd>, views: &Views, orzma: &Orzma) -> anyhow::Result<ControlFlow<()>> {
    for cmd in cmds {
        if run_cmd(cmd, views, orzma)?.is_break() {
            return Ok(ControlFlow::Break(()));
        }
    }
    Ok(ControlFlow::Continue(()))
}

/// Performs one [`Cmd`]; `Break` when the app should exit.
fn run_cmd(cmd: Cmd, views: &Views, orzma: &Orzma) -> anyhow::Result<ControlFlow<()>> {
    match cmd {
        Cmd::Quit => return Ok(ControlFlow::Break(())),
        Cmd::Navigate(url) => views.page.navigate(url)?,
        Cmd::HistoryBack => views.page.go_back()?,
        Cmd::HistoryForward => views.page.go_forward()?,
        Cmd::Reload => views.page.reload()?,
        Cmd::Scroll(action) => {
            let _ = views.page.emit("scroll", &scroll_payload(action));
        }
        Cmd::HintShow => {
            let _ = views.page.emit("hints:show", &json!({}));
        }
        Cmd::HintKey(c) => {
            let _ = views
                .page
                .emit("hints:key", &json!({ "key": c.to_string() }));
        }
        Cmd::HintBackspace => {
            let _ = views.page.emit("hints:key", &json!({ "backspace": true }));
        }
        Cmd::HintHide => {
            let _ = views.page.emit("hints:hide", &json!({}));
        }
        Cmd::SetForwardKeys(target, set) => {
            let _ = views
                .of(target)
                .set_forward_keys(keymap::forward_chords(set));
        }
        Cmd::FocusPage => {
            let _ = views.page.focus();
        }
        Cmd::FocusChrome => {
            let _ = views.chrome.focus();
        }
        Cmd::Blur => {
            let _ = orzma.blur();
        }
    }
    Ok(ControlFlow::Continue(()))
}

fn scroll_payload(action: ScrollAction) -> Value {
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
    json!({ "action": name })
}

fn install_panic_hook() {
    let prev = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), LeaveAlternateScreen);
        prev(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::ORZMA_HINTS_JS;

    /// Asserts that the hint engine asset carries the handlers the protocol
    /// needs.
    ///
    /// Case: the page loads the preload script and the user presses `f`.
    #[test]
    fn hint_engine_asset_carries_the_protocol_handlers() {
        assert!(
            ORZMA_HINTS_JS.contains("hints:show"),
            "the hint engine must register the hints:show handler"
        );
        assert!(
            ORZMA_HINTS_JS.contains("hintResult"),
            "the hint engine must report via hintResult"
        );
    }
}
