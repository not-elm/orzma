//! orzmd — a rich Markdown viewer TUI for orzma panes.

mod app;
mod assets;
mod document;
mod keymap;
mod local_assets;
mod outline;
mod protocol;
mod ui;
mod watcher;

use crate::app::{App, Cmd};
use crate::document::Document;
use crate::protocol::{
    Content, NavigateRequest, OpenExternal, OpenPath, Scroll, ScrollState, ScrollTo, Search,
    SearchCount, SearchNav, StageAssetsRequest, StageAssetsResponse,
};
use crate::ui::LiveStatus;
use crate::watcher::FileWatcher;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui_orzma::{KeyChord, Orzma, OrzmaBackend, OrzmaError, RpcError, Webview, WebviewHandle};
use std::ffi::OsStr;
use std::io::stdout;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct HistoryEntry {
    path: PathBuf,
    ratio: f64,
}

struct Session {
    state: App,
    current_path: PathBuf,
    history: Vec<HistoryEntry>,
    current_watcher: FileWatcher,
    file_name: String,
    last_fp: Option<document::Fingerprint>,
    live: LiveStatus,
    latest_ratio: f64,
    flash: Option<String>,
    search_status: Option<SearchCount>,
}

impl Session {
    fn new(current_path: PathBuf, current_watcher: FileWatcher) -> Self {
        let last_fp = document::fingerprint(&current_path).ok();
        let file_name = file_name_of(&current_path);
        Self {
            state: App::default(),
            current_path,
            history: Vec::new(),
            current_watcher,
            file_name,
            last_fp,
            live: LiveStatus::Watching,
            latest_ratio: 0.0,
            flash: None,
            search_status: None,
        }
    }

    fn scroll_percent(&self) -> u16 {
        (self.latest_ratio * 100.0).round() as u16
    }

    fn base_dir(&self) -> &Path {
        self.current_path.parent().unwrap_or_else(|| Path::new("."))
    }

    fn navigate(
        &mut self,
        request: NavigateRequest,
        shared: &Arc<Mutex<Document>>,
        view: &WebviewHandle,
        reload_tx: &mpsc::Sender<()>,
    ) {
        let base = self.base_dir();
        match document::resolve_link(base, &request.path) {
            Ok(target) if document::is_markdown(&target) => {
                let previous = HistoryEntry {
                    path: self.current_path.clone(),
                    ratio: self.latest_ratio,
                };
                let scroll = request
                    .fragment
                    .map_or(ScrollTo::Top, |slug| ScrollTo::Slug { slug });
                if self.load_and_show(&target, scroll, shared, view, reload_tx) {
                    self.history.push(previous);
                }
            }
            _ => self.flash = Some(format!("cannot open {}", request.path)),
        }
    }

    fn back(
        &mut self,
        shared: &Arc<Mutex<Document>>,
        view: &WebviewHandle,
        reload_tx: &mpsc::Sender<()>,
    ) {
        match self.history.pop() {
            Some(entry) => {
                if !self.load_and_show(
                    &entry.path,
                    ScrollTo::Ratio { ratio: entry.ratio },
                    shared,
                    view,
                    reload_tx,
                ) {
                    self.history.push(entry);
                }
            }
            None => self.flash = Some("no previous page".to_owned()),
        }
    }

    fn load_and_show(
        &mut self,
        target: &Path,
        scroll_to: ScrollTo,
        shared: &Arc<Mutex<Document>>,
        view: &WebviewHandle,
        reload_tx: &mpsc::Sender<()>,
    ) -> bool {
        let doc = match document::load(target) {
            Ok(d) => d,
            Err(_) => {
                self.flash = Some(format!("cannot open {}", target.display()));
                return false;
            }
        };
        match watcher::watch(target, reload_tx.clone()) {
            Ok(w) => self.current_watcher = w,
            Err(_) => {
                self.flash = Some("watch failed".to_owned());
                return false;
            }
        }
        self.current_path = target.to_path_buf();
        self.file_name = file_name_of(target);
        self.last_fp = document::fingerprint(target).ok();
        self.live = LiveStatus::Watching;
        self.flash = None;
        self.search_status = None;
        self.state.clear_search_state();
        self.state.set_outline(doc.outline.clone());
        let content = content_for(&doc, scroll_to);
        if let Ok(mut guard) = shared.lock() {
            *guard = doc;
        }
        let _ = view.emit("content", &content);
        true
    }

    fn reload(&mut self, shared: &Arc<Mutex<Document>>, view: &WebviewHandle) {
        let fp = match document::fingerprint(&self.current_path) {
            Ok(fp) => fp,
            Err(_) => {
                self.live = LiveStatus::Missing;
                return;
            }
        };
        if Some(fp) == self.last_fp {
            return;
        }
        let doc = match document::load(&self.current_path) {
            Ok(d) => d,
            Err(_) => {
                self.live = LiveStatus::Missing;
                return;
            }
        };
        // NOTE: record the fingerprint only after a successful load — setting it
        // before would let a transient read failure poison the skip-check and
        // permanently suppress a later reload with the same fingerprint.
        self.last_fp = Some(fp);
        self.live = LiveStatus::Watching;
        self.flash = None;
        self.state.set_outline(doc.outline.clone());
        let content = content_for(&doc, ScrollTo::Preserve);
        if let Ok(mut guard) = shared.lock() {
            *guard = doc;
        }
        let _ = view.emit("content", &content);
    }
}

fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Whether `url` carries one of the schemes safe to hand to the OS opener.
fn allowed_external_url(url: &str) -> bool {
    matches!(
        url.split_once(':'),
        Some((scheme, _))
            if scheme.eq_ignore_ascii_case("http")
                || scheme.eq_ignore_ascii_case("https")
                || scheme.eq_ignore_ascii_case("mailto")
                || scheme.eq_ignore_ascii_case("tel")
    )
}

// NOTE: `open` launches each target with the user's own authority — the same as
// double-clicking it in Finder. Some regular-file types auto-execute or redirect
// (.command/.terminal/.tool run scripts; .webloc/.fileloc open an embedded URL),
// so orzmd is for viewing TRUSTED local documents and does not sandbox link
// targets (see the design's non-goals).
/// Opens `target` (a URL or absolute path) with the macOS default handler.
/// No shell is involved, so `target` is not interpreted.
fn spawn_open(target: impl AsRef<OsStr>) {
    let _ = Command::new("open").arg(target).spawn();
}

fn main() {
    if let Err(e) = run() {
        eprintln!("orzmd: {e}");
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    let arg = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: orzmd <markdown-file>"))?;
    let path =
        document::resolve_path(&arg).map_err(|e| anyhow::anyhow!("cannot open {arg}: {e}"))?;

    let doc = document::load(&path)?;
    let shared = Arc::new(Mutex::new(doc));

    let orzma = Orzma::connect().map_err(|e| match e {
        OrzmaError::NotInPane(_) => anyhow::anyhow!("{e}. Run orzmd inside an orzma pane."),
        _ => anyhow::anyhow!("{e}"),
    })?;

    let asset_dir = assets::materialize()?;

    let view = register_view(&orzma, &asset_dir, Arc::clone(&shared))?;

    let (reload_tx, reload_rx) = mpsc::channel::<()>();
    let watcher = watcher::watch(&path, reload_tx.clone())?;

    enable_raw_mode()?;
    if let Err(e) = execute!(stdout(), EnterAlternateScreen) {
        let _ = disable_raw_mode();
        return Err(e.into());
    }
    install_panic_hook();

    let result = event_loop(watcher, &orzma, &view, &shared, path, reload_tx, &reload_rx);

    let _ = disable_raw_mode();
    let _ = execute!(stdout(), LeaveAlternateScreen);
    result
}

fn register_view(
    orzma: &Orzma,
    asset_dir: &tempfile::TempDir,
    shared: Arc<Mutex<Document>>,
) -> anyhow::Result<WebviewHandle> {
    let ready_doc = Arc::clone(&shared);
    let stage_doc = Arc::clone(&shared);
    let local_root = asset_dir.path().to_path_buf();
    let view = orzma.register(
        Webview::dir(asset_dir.path(), "index.html")
            .interactive(true)
            .forward_keys([
                KeyChord {
                    mods: KeyModifiers::NONE,
                    code: KeyCode::Backspace,
                },
                KeyChord {
                    mods: KeyModifiers::CONTROL,
                    code: KeyCode::Char('o'),
                },
            ])
            .on("ready", move |(): ()| -> Result<Content, RpcError> {
                let doc = ready_doc.lock().map_err(|_| RpcError::new("poisoned"))?;
                Ok(content_for(&doc, ScrollTo::Preserve))
            })
            .on(
                "stageAssets",
                move |req: StageAssetsRequest| -> Result<StageAssetsResponse, RpcError> {
                    let base_dir = {
                        let doc = stage_doc.lock().map_err(|_| RpcError::new("poisoned"))?;
                        doc.base_dir.clone()
                    };
                    let urls = req
                        .paths
                        .iter()
                        .map(|p| local_assets::stage(&local_root, &base_dir, p))
                        .collect();
                    Ok(StageAssetsResponse { urls })
                },
            )
            .add_event::<SearchCount>("searchCount")
            .add_event::<ScrollState>("scrollState")
            .add_event::<NavigateRequest>("navigate")
            .add_event::<OpenExternal>("openExternal")
            .add_event::<OpenPath>("openPath"),
    )?;
    Ok(view)
}

fn event_loop(
    current_watcher: FileWatcher,
    orzma: &Orzma,
    view: &WebviewHandle,
    shared: &Arc<Mutex<Document>>,
    start_path: PathBuf,
    reload_tx: mpsc::Sender<()>,
    reload_rx: &mpsc::Receiver<()>,
) -> anyhow::Result<()> {
    let backend = OrzmaBackend::new(CrosstermBackend::new(stdout()), orzma);
    let mut terminal = Terminal::new(backend)?;

    let mut session = Session::new(start_path, current_watcher);
    session
        .state
        .set_outline(shared.lock().unwrap().outline.clone());
    loop {
        for c in view.read_events::<SearchCount>() {
            session.search_status = Some(c);
        }
        for s in view.read_events::<ScrollState>() {
            session.latest_ratio = s.ratio.clamp(0.0, 1.0);
            session
                .state
                .set_current_heading_index(s.current_heading_index);
        }
        for request in view.read_events::<NavigateRequest>() {
            session.navigate(request, shared, view, &reload_tx);
        }
        for ext in view.read_events::<OpenExternal>() {
            if allowed_external_url(&ext.url) {
                spawn_open(&ext.url);
            }
        }
        for op in view.read_events::<OpenPath>() {
            let base = session.base_dir();
            match document::resolve_link(base, &op.path) {
                Ok(target) => spawn_open(&target),
                Err(_) => session.flash = Some(format!("cannot open {}", op.path)),
            }
        }

        let mut reload = false;
        while reload_rx.try_recv().is_ok() {
            reload = true;
        }
        if reload {
            session.reload(shared, view);
        }

        let percent = session.scroll_percent();
        terminal.draw(|f| {
            ui::draw(
                f,
                &mut orzma.frame(),
                &session.state,
                &view.id(),
                &session.file_name,
                session.live,
                percent,
                session.search_status,
                session.flash.as_deref(),
            );
        })?;

        if event::poll(Duration::from_millis(33))?
            && let Event::Key(key) = event::read()?
        {
            let action = keymap::map(session.state.mode(), key);
            for cmd in session.state.on_action(action) {
                match cmd {
                    Cmd::Quit => return Ok(()),
                    Cmd::Reload => session.reload(shared, view),
                    Cmd::Back => session.back(shared, view, &reload_tx),
                    Cmd::Scroll(action) => {
                        let _ = view.emit("scroll", &Scroll { action });
                    }
                    Cmd::ScrollToHeading(index) => {
                        let _ =
                            view.emit("scrollToHeading", &serde_json::json!({ "index": index }));
                    }
                    Cmd::Search(query) => {
                        let _ = view.emit("search", &Search { query });
                    }
                    Cmd::SearchNav(dir) => {
                        let _ = view.emit("searchNav", &SearchNav { dir });
                    }
                    Cmd::ClearSearch => {
                        session.search_status = None;
                        let _ = view.emit("clearSearch", &());
                    }
                }
            }
        }
    }
}

fn content_for(doc: &Document, scroll_to: ScrollTo) -> Content {
    Content {
        markdown: doc.text.clone(),
        base_dir: doc.base_dir.display().to_string(),
        scroll_to,
    }
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
    use super::*;

    #[test]
    fn allowed_external_url_accepts_web_schemes_only() {
        assert!(allowed_external_url("https://example.com"));
        assert!(allowed_external_url("http://x"));
        assert!(allowed_external_url("mailto:a@b.com"));
        assert!(allowed_external_url("tel:+1"));
        assert!(!allowed_external_url("javascript:alert(1)"));
        assert!(!allowed_external_url("data:text/html,x"));
        assert!(!allowed_external_url("file:///etc/passwd"));
        assert!(!allowed_external_url("not a url"));
    }
}
