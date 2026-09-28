//! orzmd — a rich Markdown viewer TUI for orzma panes.

mod app;
mod assets;
mod chrome;
mod document;
mod keymap;
mod local_assets;
mod outline;
mod protocol;
mod ui;
mod watcher;

use crate::app::{App, Cmd};
use crate::chrome::{Chrome, Toast};
use crate::document::Document;
use crate::keymap::Action;
use crate::protocol::{
    Content, NavigateRequest, OpenExternal, OpenPath, Scroll, ScrollState, ScrollTo, Search,
    SearchCount, SearchNav, StageAssetsRequest, StageAssetsResponse,
};
use crate::watcher::FileWatcher;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui_orzma::{Orzma, OrzmaBackend, OrzmaError, RpcError, Webview, WebviewHandle};
use std::borrow::Cow;
use std::ffi::OsStr;
use std::io::{self, stdout};
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
    missing: bool,
    latest_ratio: f64,
    toast: Option<Toast>,
    search_status: Option<SearchCount>,
    last_chrome: Option<Chrome>,
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
            missing: false,
            latest_ratio: 0.0,
            toast: None,
            search_status: None,
            last_chrome: None,
        }
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
            _ => {
                self.toast = Some(Toast::error(
                    format!("cannot open {}", request.path),
                    Instant::now(),
                ))
            }
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
            None => self.toast = Some(Toast::info("no previous page", Instant::now())),
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
                self.toast = Some(Toast::error(
                    format!("cannot open {}", target.display()),
                    Instant::now(),
                ));
                return false;
            }
        };
        match watcher::watch(target, reload_tx.clone()) {
            Ok(w) => self.current_watcher = w,
            Err(_) => {
                self.toast = Some(Toast::error("watch failed", Instant::now()));
                return false;
            }
        }
        self.current_path = target.to_path_buf();
        self.file_name = file_name_of(target);
        self.last_fp = document::fingerprint(target).ok();
        self.missing = false;
        self.toast = None;
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
                self.missing = true;
                return;
            }
        };
        if Some(fp) == self.last_fp {
            return;
        }
        let doc = match document::load(&self.current_path) {
            Ok(d) => d,
            Err(_) => {
                self.missing = true;
                return;
            }
        };
        // NOTE: record the fingerprint only after a successful load — setting it
        // before would let a transient read failure poison the skip-check and
        // permanently suppress a later reload with the same fingerprint.
        self.last_fp = Some(fp);
        self.missing = false;
        self.toast = None;
        self.state.set_outline(doc.outline.clone());
        let content = content_for(&doc, ScrollTo::Preserve);
        if let Ok(mut guard) = shared.lock() {
            *guard = doc;
        }
        let _ = view.emit("content", &content);
    }

    /// Applies the focus changes the host reported since the last call.
    fn apply_focus_changes(&mut self, view: &WebviewHandle) {
        for change in view.read_focus_changes() {
            self.state.on_focus_change(change.focused);
        }
    }

    /// Sends the page the chrome when it differs from the last one sent, or
    /// unconditionally once the page has asked for its content again.
    fn sync_chrome(&mut self, view: &WebviewHandle, stale: &AtomicBool) {
        if stale.swap(false, Ordering::AcqRel) {
            self.last_chrome = None;
        }
        let chrome = Chrome::build(
            &self.state,
            &self.file_name,
            self.missing,
            self.toast.as_ref(),
        );
        if self.last_chrome.as_ref() == Some(&chrome) {
            return;
        }
        if view.emit("chrome", &chrome).is_ok() {
            self.last_chrome = Some(chrome);
        }
    }

    /// Drops the toast once it has been on screen for its whole lifetime.
    fn expire_toast(&mut self, now: Instant) {
        if self
            .toast
            .as_ref()
            .is_some_and(|toast| toast.is_expired(now))
        {
            self.toast = None;
        }
    }
}

/// The handles a command needs besides the session.
struct Ctx<'a> {
    orzma: &'a Orzma,
    view: &'a WebviewHandle,
    shared: &'a Arc<Mutex<Document>>,
    reload_tx: &'a mpsc::Sender<()>,
}

/// Performs `cmds` in order; `Break` when one of them quits the app.
fn run_cmds(session: &mut Session, cmds: Vec<Cmd>, ctx: &Ctx<'_>) -> ControlFlow<()> {
    for cmd in cmds {
        match cmd {
            Cmd::Quit => return ControlFlow::Break(()),
            Cmd::Reload => session.reload(ctx.shared, ctx.view),
            Cmd::Back => session.back(ctx.shared, ctx.view, ctx.reload_tx),
            Cmd::Scroll(action) => {
                let _ = ctx.view.emit("scroll", &Scroll { action });
            }
            Cmd::ScrollToHeading(index) => {
                let _ = ctx
                    .view
                    .emit("scrollToHeading", &serde_json::json!({ "index": index }));
            }
            Cmd::Search(query) => {
                let _ = ctx.view.emit("search", &Search { query });
            }
            Cmd::SearchNav(dir) => {
                let _ = ctx.view.emit("searchNav", &SearchNav { dir });
            }
            Cmd::ClearSearch => {
                session.search_status = None;
                let _ = ctx.view.emit("clearSearch", &());
            }
            Cmd::Focus => {
                let _ = ctx.view.focus();
            }
            Cmd::Blur => {
                let _ = ctx.orzma.blur();
            }
        }
    }
    ControlFlow::Continue(())
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

// NOTE: the platform opener launches each target with the user's own
// authority, the same as double-clicking it in Finder or Explorer. Some
// regular-file types run or redirect when opened (.command/.terminal/.tool
// scripts and .webloc/.fileloc links on macOS; .exe/.bat/.lnk on Windows), so
// orzmd is for viewing trusted local documents and does not sandbox link
// targets.
/// Opens `target` (a URL or absolute path) with the platform's default
/// handler. No command interpreter parses `target`, but on Windows the shell
/// substitutes a URL into the command line its scheme's handler registers.
///
/// # Errors
///
/// Returns the opener's error when the opener cannot be started. On macOS and
/// Linux the opener runs detached, so a handler that fails after it starts is
/// not reported.
fn spawn_open(target: impl AsRef<OsStr>) -> io::Result<()> {
    open::that_detached(target)
}

/// `path` in a form the Windows shell accepts: a `\\?\C:\` verbatim prefix is
/// stripped when `dunce` judges it safe, and a `\\?\UNC\server\share` prefix
/// becomes `\\server\share`. Returns `path` unchanged elsewhere.
#[cfg(windows)]
fn shell_path(path: &Path) -> Cow<'_, Path> {
    let simplified = dunce::simplified(path);
    match simplified
        .to_str()
        .and_then(|s| s.strip_prefix(r"\\?\UNC\"))
    {
        Some(share) => Cow::Owned(PathBuf::from(format!(r"\\{share}"))),
        None => Cow::Borrowed(simplified),
    }
}

#[cfg(not(windows))]
fn shell_path(path: &Path) -> Cow<'_, Path> {
    Cow::Borrowed(path)
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

    let chrome_stale = Arc::new(AtomicBool::new(false));
    let view = register_view(
        &orzma,
        &asset_dir,
        Arc::clone(&shared),
        Arc::clone(&chrome_stale),
    )?;

    let (reload_tx, reload_rx) = mpsc::channel::<()>();
    let watcher = watcher::watch(&path, reload_tx.clone())?;

    enable_raw_mode()?;
    if let Err(e) = execute!(stdout(), EnterAlternateScreen) {
        let _ = disable_raw_mode();
        return Err(e.into());
    }
    install_panic_hook();

    let ctx = Ctx {
        orzma: &orzma,
        view: &view,
        shared: &shared,
        reload_tx: &reload_tx,
    };
    let result = event_loop(watcher, path, &ctx, &reload_rx, &chrome_stale);

    let _ = disable_raw_mode();
    let _ = execute!(stdout(), LeaveAlternateScreen);
    result
}

fn register_view(
    orzma: &Orzma,
    asset_dir: &tempfile::TempDir,
    shared: Arc<Mutex<Document>>,
    chrome_stale: Arc<AtomicBool>,
) -> anyhow::Result<WebviewHandle> {
    let ready_doc = Arc::clone(&shared);
    let stage_doc = Arc::clone(&shared);
    let local_root = asset_dir.path().to_path_buf();
    let view = orzma.register(
        Webview::dir(asset_dir.path(), "index.html")
            .interactive(true)
            .forward_keys(keymap::forward_chords())
            .on("ready", move |(): ()| -> Result<Content, RpcError> {
                chrome_stale.store(true, Ordering::Release);
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
    start_path: PathBuf,
    ctx: &Ctx<'_>,
    reload_rx: &mpsc::Receiver<()>,
    chrome_stale: &AtomicBool,
) -> anyhow::Result<()> {
    let (orzma, view, shared) = (ctx.orzma, ctx.view, ctx.shared);
    let backend = OrzmaBackend::new(CrosstermBackend::new(stdout()), orzma);
    let mut terminal = Terminal::new(backend)?;

    let mut session = Session::new(start_path, current_watcher);
    if let Ok(doc) = shared.lock() {
        session.state.set_outline(doc.outline.clone());
    }
    loop {
        session.apply_focus_changes(view);
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
            session.navigate(request, shared, view, ctx.reload_tx);
        }
        for ext in view.read_events::<OpenExternal>() {
            if allowed_external_url(&ext.url) && spawn_open(&ext.url).is_err() {
                session.toast = Some(Toast::error(
                    format!("cannot open {}", ext.url),
                    Instant::now(),
                ));
            }
        }
        for op in view.read_events::<OpenPath>() {
            let base = session.base_dir();
            let opened = document::resolve_link(base, &op.path)
                .ok()
                .is_some_and(|target| spawn_open(shell_path(&target).as_os_str()).is_ok());
            if !opened {
                session.toast = Some(Toast::error(
                    format!("cannot open {}", op.path),
                    Instant::now(),
                ));
            }
        }

        let mut reload = false;
        while reload_rx.try_recv().is_ok() {
            reload = true;
        }
        if reload {
            session.reload(shared, view);
        }

        session.expire_toast(Instant::now());
        session.sync_chrome(view, chrome_stale);
        terminal.draw(|f| {
            ui::draw(
                f,
                &mut orzma.frame(),
                &session.state,
                &view.instance_id(),
                session.search_status,
            );
        })?;

        if event::poll(Duration::from_millis(33))?
            && let Event::Key(key) = event::read()?
        {
            session.apply_focus_changes(view);
            let action = keymap::map(session.state.mode(), key);
            if action != Action::Ignore {
                session.toast = None;
            }
            let cmds = session.state.on_action(action);
            if run_cmds(&mut session, cmds, ctx).is_break() {
                return Ok(());
            }
            session.sync_chrome(view, chrome_stale);
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

    /// Asserts that a verbatim disk path loses its prefix and a verbatim UNC
    /// path becomes a plain `\\server\share` path.
    ///
    /// Case: a user clicks a link in a document on the local disk, and
    /// another in a document on a mapped network drive.
    #[cfg(windows)]
    #[test]
    fn shell_path_strips_verbatim_prefixes() {
        assert_eq!(
            &*shell_path(Path::new(r"\\?\C:\docs\spec.pdf")),
            Path::new(r"C:\docs\spec.pdf")
        );
        assert_eq!(
            &*shell_path(Path::new(r"\\?\UNC\srv\team\docs\spec.pdf")),
            Path::new(r"\\srv\team\docs\spec.pdf")
        );
    }

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
