//! orzmd — a rich Markdown viewer TUI for orzma panes.

mod app;
mod assets;
mod chrome;
mod document;
mod local_assets;
mod protocol;
mod watcher;

use crate::app::{App, Cmd};
use crate::chrome::{Chrome, Toast, ToastKind};
use crate::document::Document;
use crate::protocol::{
    Content, NavigateRequest, OpenExternal, OpenPath, PageEvent, ScrollTo, StageAssetsRequest,
    StageAssetsResponse,
};
use crate::watcher::FileWatcher;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui_orzma::{
    Orzma, OrzmaBackend, OrzmaError, RpcError, Webview, WebviewHandle, WebviewWidget,
};
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
    toast: Option<Toast>,
    toast_seq: u64,
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
            toast: None,
            toast_seq: 0,
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
                    ratio: request.ratio.clamp(0.0, 1.0),
                };
                let scroll = request
                    .fragment
                    .map_or(ScrollTo::Top, |slug| ScrollTo::Slug { slug });
                if self.load_and_show(&target, scroll, shared, view, reload_tx) {
                    self.history.push(previous);
                }
            }
            _ => self.show_error(format!("cannot open {}", request.path)),
        }
    }

    fn back(
        &mut self,
        shared: &Arc<Mutex<Document>>,
        view: &WebviewHandle,
        reload_tx: &mpsc::Sender<()>,
    ) {
        let Some(entry) = self.history.pop() else {
            self.show_toast(ToastKind::Info, "no previous page");
            return;
        };
        let scroll = ScrollTo::Ratio { ratio: entry.ratio };
        if !self.load_and_show(&entry.path, scroll, shared, view, reload_tx) {
            self.history.push(entry);
        }
    }

    /// Loads and shows `target` as a document the user moved to, and returns
    /// whether the file could be read and watched.
    fn load_and_show(
        &mut self,
        target: &Path,
        scroll_to: ScrollTo,
        shared: &Arc<Mutex<Document>>,
        view: &WebviewHandle,
        reload_tx: &mpsc::Sender<()>,
    ) -> bool {
        let Ok(doc) = document::load(target) else {
            self.show_error(format!("cannot open {}", target.display()));
            return false;
        };
        match watcher::watch(target, reload_tx.clone()) {
            Ok(w) => self.current_watcher = w,
            Err(_) => {
                self.show_error("watch failed");
                return false;
            }
        }
        self.current_path = target.to_path_buf();
        self.file_name = file_name_of(target);
        self.last_fp = document::fingerprint(target).ok();
        self.missing = false;
        self.toast = None;
        let content = Content::of_document(&doc, scroll_to, true);
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
        let content = Content::of_document(&doc, ScrollTo::Preserve, false);
        if let Ok(mut guard) = shared.lock() {
            *guard = doc;
        }
        let _ = view.emit("content", &content);
    }

    /// Takes a `ready` from the page: the next chrome is sent even if
    /// unchanged, and the commands the first `ready` needs are returned.
    fn on_ready(&mut self) -> Vec<Cmd> {
        self.last_chrome = None;
        self.state.on_ready()
    }

    /// Sends the page the chrome when it differs from the last one sent.
    fn sync_chrome(&mut self, view: &WebviewHandle) {
        let chrome = Chrome::build(&self.file_name, self.missing, self.toast.as_ref());
        if self.last_chrome.as_ref() == Some(&chrome) {
            return;
        }
        if view.emit("chrome", &chrome).is_ok() {
            self.last_chrome = Some(chrome);
        }
    }

    /// Shows `text` as an error toast.
    fn show_error(&mut self, text: impl Into<String>) {
        self.show_toast(ToastKind::Error, text);
    }

    /// Shows `text` as a toast of `kind`, under an id no earlier toast of this
    /// session had.
    fn show_toast(&mut self, kind: ToastKind, text: impl Into<String>) {
        self.toast_seq += 1;
        self.toast = Some(Toast::new(self.toast_seq, kind, text, Instant::now()));
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

/// The handles and page signals the event loop and the commands need besides
/// the session.
struct Ctx<'a> {
    orzma: &'a Orzma,
    view: &'a WebviewHandle,
    shared: &'a Arc<Mutex<Document>>,
    reload_tx: &'a mpsc::Sender<()>,
    /// Raised by the `ready` handler each time the page asks for its content.
    page_ready: &'a AtomicBool,
}

/// Performs `cmds` in order; `Break` when one of them quits the app.
fn run_cmds(cmds: Vec<Cmd>, ctx: &Ctx<'_>) -> ControlFlow<()> {
    for cmd in cmds {
        match cmd {
            Cmd::Quit => return ControlFlow::Break(()),
            Cmd::Focus => {
                let _ = ctx.view.focus();
            }
            Cmd::Relay(key) => {
                let _ = ctx.view.emit("key", &key);
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

    let page_ready = Arc::new(AtomicBool::new(false));
    let view = register_view(
        &orzma,
        &asset_dir,
        Arc::clone(&shared),
        Arc::clone(&page_ready),
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
        page_ready: &page_ready,
    };
    let result = event_loop(watcher, path, &ctx, &reload_rx);

    let _ = disable_raw_mode();
    let _ = execute!(stdout(), LeaveAlternateScreen);
    result
}

fn register_view(
    orzma: &Orzma,
    asset_dir: &tempfile::TempDir,
    shared: Arc<Mutex<Document>>,
    page_ready: Arc<AtomicBool>,
) -> anyhow::Result<WebviewHandle> {
    let ready_doc = Arc::clone(&shared);
    let stage_doc = Arc::clone(&shared);
    let local_root = asset_dir.path().to_path_buf();
    let view = orzma.register(
        Webview::dir(asset_dir.path(), "index.html")
            .interactive(true)
            .on("ready", move |(): ()| -> Result<Content, RpcError> {
                page_ready.store(true, Ordering::Release);
                let doc = ready_doc.lock().map_err(|_| RpcError::new("poisoned"))?;
                Ok(Content::of_document(&doc, ScrollTo::Preserve, false))
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
            .add_event::<PageEvent>("page")
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
) -> anyhow::Result<()> {
    let (orzma, view, shared) = (ctx.orzma, ctx.view, ctx.shared);
    let backend = OrzmaBackend::new(CrosstermBackend::new(stdout()), orzma);
    let mut terminal = Terminal::new(backend)?;

    let mut session = Session::new(start_path, current_watcher);
    loop {
        if ctx.page_ready.swap(false, Ordering::AcqRel) {
            let cmds = session.on_ready();
            if run_cmds(cmds, ctx).is_break() {
                return Ok(());
            }
        }
        for event in view.read_events::<PageEvent>() {
            match event {
                PageEvent::Quit => return Ok(()),
                PageEvent::Reload => session.reload(shared, view),
                PageEvent::Back => session.back(shared, view, ctx.reload_tx),
            }
        }
        for request in view.read_events::<NavigateRequest>() {
            session.navigate(request, shared, view, ctx.reload_tx);
        }
        for ext in view.read_events::<OpenExternal>() {
            if allowed_external_url(&ext.url) && spawn_open(&ext.url).is_err() {
                session.show_error(format!("cannot open {}", ext.url));
            }
        }
        for op in view.read_events::<OpenPath>() {
            let base = session.base_dir();
            let opened = document::resolve_link(base, &op.path)
                .ok()
                .is_some_and(|target| spawn_open(shell_path(&target).as_os_str()).is_ok());
            if !opened {
                session.show_error(format!("cannot open {}", op.path));
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
        session.sync_chrome(view);
        terminal.draw(|f| {
            f.render_stateful_widget(
                WebviewWidget::new(view.instance_id()),
                f.area(),
                &mut orzma.frame(),
            );
        })?;

        // NOTE: key releases (reported on Windows) must be dropped here: a
        // release relayed to the page would run as a second tap of its key.
        if event::poll(Duration::from_millis(33))?
            && let Event::Key(key) = event::read()?
            && key.kind != KeyEventKind::Release
        {
            let cmds = session.state.on_key(key);
            if run_cmds(cmds, ctx).is_break() {
                return Ok(());
            }
        }
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
