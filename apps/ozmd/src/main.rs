//! ozmd — a rich Markdown viewer TUI for ozmux panes.

mod app;
mod assets;
mod document;
mod keymap;
mod outline;
mod protocol;
mod ui;
mod watcher;

use crate::app::{App, Cmd};
use crate::protocol::{Content, Scroll, Search, SearchCount, SearchNav, ScrollState};
use crate::ui::LiveStatus;
use crossbeam_channel::{Receiver, Sender};
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::Terminal;
use ratatui_ozma::{Ozma, OzmaBackend, RpcError, Webview, WebviewHandle};
use std::io::stdout;
use std::path::Path;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Messages forwarded from page→controller `.on` handlers to the main loop.
enum PageMsg {
    SearchCount(SearchCount),
    ScrollState(ScrollState),
}

fn main() {
    if let Err(e) = run() {
        eprintln!("ozmd: {e}");
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    let arg = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: ozmd <markdown-file>"))?;
    let path = document::resolve_path(&arg).map_err(|e| anyhow::anyhow!("cannot open {arg}: {e}"))?;

    let doc = document::load(&path)?;
    let shared = Arc::new(Mutex::new(doc));

    let ozma = Ozma::connect().map_err(|e| anyhow::anyhow!("{e}. Run ozmd inside an ozmux pane."))?;

    let asset_dir = assets::materialize()?;

    let (page_tx, page_rx) = crossbeam_channel::unbounded::<PageMsg>();
    let view = register_view(&ozma, &asset_dir, Arc::clone(&shared), page_tx)?;

    let (reload_tx, reload_rx) = mpsc::channel::<()>();
    let _watcher = watcher::watch(&path, reload_tx)?;

    enable_raw_mode()?;
    if let Err(e) = execute!(stdout(), EnterAlternateScreen) {
        let _ = disable_raw_mode();
        return Err(e.into());
    }
    install_panic_hook();

    let result = event_loop(&ozma, &view, &shared, &path, &page_rx, &reload_rx);

    let _ = disable_raw_mode();
    let _ = execute!(stdout(), LeaveAlternateScreen);
    result
}

fn register_view(
    ozma: &Ozma,
    asset_dir: &tempfile::TempDir,
    shared: Arc<Mutex<document::Document>>,
    page_tx: Sender<PageMsg>,
) -> anyhow::Result<WebviewHandle> {
    let ready_doc = Arc::clone(&shared);
    let count_tx = page_tx.clone();
    let state_tx = page_tx;
    let view = ozma.register(
        Webview::dir(asset_dir.path(), "index.html")
            .interactive(false)
            .on("ready", move |(): ()| -> Result<Content, RpcError> {
                let doc = ready_doc.lock().map_err(|_| RpcError::new("poisoned"))?;
                Ok(Content {
                    markdown: doc.text.clone(),
                    base_dir: doc.base_dir.display().to_string(),
                })
            })
            .on("searchCount", move |c: SearchCount| -> Result<(), RpcError> {
                let _ = count_tx.send(PageMsg::SearchCount(c));
                Ok(())
            })
            .on("scrollState", move |s: ScrollState| -> Result<(), RpcError> {
                let _ = state_tx.send(PageMsg::ScrollState(s));
                Ok(())
            }),
    )?;
    Ok(view)
}

fn event_loop(
    ozma: &Ozma,
    view: &WebviewHandle,
    shared: &Arc<Mutex<document::Document>>,
    path: &Path,
    page_rx: &Receiver<PageMsg>,
    reload_rx: &mpsc::Receiver<()>,
) -> anyhow::Result<()> {
    let backend = OzmaBackend::new(CrosstermBackend::new(stdout()), ozma);
    let mut terminal = Terminal::new(backend)?;

    let mut state = App::default();
    state.set_outline(shared.lock().unwrap().outline.clone());
    let mut live = LiveStatus::Watching;
    let mut scroll_percent: u16 = 0;
    let mut last_fp = document::fingerprint(path).ok();
    let mut search_status: Option<SearchCount> = None;
    let file_name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();

    loop {
        while let Ok(msg) = page_rx.try_recv() {
            match msg {
                PageMsg::SearchCount(c) => search_status = Some(c),
                PageMsg::ScrollState(s) => {
                    scroll_percent = (s.ratio.clamp(0.0, 1.0) * 100.0).round() as u16;
                    state.set_current_heading_index(s.current_heading_index);
                }
            }
        }

        let mut reload = false;
        while reload_rx.try_recv().is_ok() {
            reload = true;
        }
        if reload {
            apply_reload(&mut state, &mut live, &mut last_fp, view, shared, path)?;
        }

        terminal.draw(|f| {
            ui::draw(
                f,
                &mut *ozma.frame(),
                &state,
                view.id(),
                &file_name,
                live,
                scroll_percent,
                search_status,
            );
        })?;

        if event::poll(Duration::from_millis(33))?
            && let Event::Key(key) = event::read()?
        {
            let action = keymap::map(state.mode(), key);
            for cmd in state.on_action(action) {
                match cmd {
                    Cmd::Quit => return Ok(()),
                    Cmd::Reload => {
                        apply_reload(&mut state, &mut live, &mut last_fp, view, shared, path)?;
                    }
                    Cmd::Scroll(action) => {
                        let _ = view.emit("scroll", &Scroll { action });
                    }
                    Cmd::ScrollToHeading(index) => {
                        let _ = view.emit("scrollToHeading", &serde_json::json!({ "index": index }));
                    }
                    Cmd::Search(query) => {
                        let _ = view.emit("search", &Search { query });
                    }
                    Cmd::SearchNav(dir) => {
                        let _ = view.emit("searchNav", &SearchNav { dir });
                    }
                    Cmd::ClearSearch => {
                        search_status = None;
                        let _ = view.emit("clearSearch", &());
                    }
                }
            }
        }
    }
}

fn apply_reload(
    state: &mut App,
    live: &mut LiveStatus,
    last_fp: &mut Option<document::Fingerprint>,
    view: &WebviewHandle,
    shared: &Arc<Mutex<document::Document>>,
    path: &Path,
) -> anyhow::Result<()> {
    match document::fingerprint(path) {
        Ok(fp) => {
            if Some(fp) == *last_fp {
                return Ok(());
            }
            *last_fp = Some(fp);
        }
        Err(_) => {
            *live = LiveStatus::Missing;
            return Ok(());
        }
    }
    let doc = match document::load(path) {
        Ok(d) => d,
        Err(_) => {
            *live = LiveStatus::Missing;
            return Ok(());
        }
    };
    *live = LiveStatus::Watching;
    state.set_outline(doc.outline.clone());
    let content = Content {
        markdown: doc.text.clone(),
        base_dir: doc.base_dir.display().to_string(),
    };
    {
        let mut guard = shared.lock().map_err(|_| anyhow::anyhow!("state poisoned"))?;
        *guard = doc;
    }
    let _ = view.emit("content", &content);
    Ok(())
}

fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), LeaveAlternateScreen);
        prev(info);
    }));
}
