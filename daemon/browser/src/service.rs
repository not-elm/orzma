//! Public service surface (`BrowserService`). Mirrors `TerminalService` in
//! shape but with a `tokio::sync::watch` channel per Activity instead of a
//! `broadcast` delta ring.
//!
//! The registry is `Arc<RwLock<HashMap<ActivityId, BrowserHandle>>>`: the
//! lock is held only long enough to clone or remove a handle; no CDP call
//! ever awaits inside it. Per-Activity work runs in two tasks:
//!  - `bridge::run` (screencast + nav refresh, owns the watch sender),
//!  - `page::run` (actor command loop, owns the page's mpsc sender end).
//!
//! Chromium lifecycle is governed by `ChromiumState` behind a `Mutex`,
//! plus a `Notify` so concurrent `spawn` calls wait for the first launcher.

use crate::bridge::BridgeConfig;
use crate::cookie::import_chrome_default_cookies;
use crate::error::{BrowserError, BrowserResult};
use crate::page::PageCommand;
use crate::snapshot::BrowserSnapshot;
use crate::state::{AttachOutcome, ChromiumState, Phase};
use crate::wire::{BrowserClientMsg, NavCommand};
use chromiumoxide::{Browser, BrowserConfig};
use futures_util::StreamExt;
use ozmux_extension::runtime::RuntimeRoot;
use ozmux_multiplexer::{ActivityId, PaneId, WindowId};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, Notify, RwLock, mpsc, watch};
use tokio_util::sync::CancellationToken;

const DEFAULT_GRACE: Duration = Duration::from_secs(30);

/// Public service for browser activities. Cheap to `clone` — internal state
/// is reference-counted.
#[derive(Clone)]
pub struct BrowserService {
    chromium: Arc<ChromiumProcess>,
    pages: Arc<RwLock<HashMap<ActivityId, BrowserHandle>>>,
}

struct ChromiumProcess {
    state: Mutex<ChromiumState>,
    notify_started: Notify,
    browser: Mutex<Option<Browser>>,
    runtime: Arc<RuntimeRoot>,
}

#[derive(Clone)]
struct BrowserHandle {
    page_tx: mpsc::Sender<PageCommand>,
    snapshot_rx: watch::Receiver<Arc<BrowserSnapshot>>,
    cancel: CancellationToken,
}

impl BrowserService {
    /// Construct a new service. The `RuntimeRoot` is shared with other
    /// services and provides the per-daemon `bin/`, `sock/`, and (here)
    /// `browser/` directory for Chromium's user-data-dir.
    pub fn new(runtime: Arc<RuntimeRoot>) -> Self {
        Self {
            chromium: Arc::new(ChromiumProcess {
                state: Mutex::new(ChromiumState::new(DEFAULT_GRACE)),
                notify_started: Notify::new(),
                browser: Mutex::new(None),
                runtime,
            }),
            pages: Arc::default(),
        }
    }

    /// Create a CDP page for `aid`, optionally navigate to `initial_url`,
    /// start screencast, and stash the handle. Returns `Err` on launch or
    /// page-creation failure so the caller's rollback can run.
    pub async fn spawn(
        &self,
        _wid: &WindowId,
        _pid: &PaneId,
        aid: &ActivityId,
        initial_url: Option<String>,
    ) -> BrowserResult<()> {
        let outcome = self.chromium.state.lock().await.attach();
        match outcome {
            AttachOutcome::MustLaunch => {
                let res = self.launch().await;
                {
                    let mut st = self.chromium.state.lock().await;
                    if res.is_ok() {
                        st.mark_started();
                    } else {
                        // NOTE: undo the Starting transition so the next spawn
                        // can retry from Stopped.
                        st.detach();
                    }
                }
                self.chromium.notify_started.notify_waiters();
                res?;
            }
            AttachOutcome::Wait => {
                self.chromium.notify_started.notified().await;
            }
            AttachOutcome::Reused => {}
        }

        let browser_guard = self.chromium.browser.lock().await;
        let Some(browser) = browser_guard.as_ref() else {
            return Err(BrowserError::Launch("chromium not available".into()));
        };
        let page = browser
            .new_page("about:blank")
            .await
            .map_err(|e| BrowserError::Cdp(e.to_string()))?;
        drop(browser_guard);

        if let Some(url) = initial_url.as_ref() {
            let _ = page.goto(url.as_str()).await;
        }

        let (page_tx, page_rx) = mpsc::channel::<PageCommand>(64);
        let (snapshot_tx, snapshot_rx) = watch::channel(Arc::new(BrowserSnapshot::default()));
        let cancel = CancellationToken::new();

        let bridge_page = page.clone();
        let bridge_cancel = cancel.clone();
        tokio::spawn(async move {
            crate::bridge::run(
                bridge_page,
                snapshot_tx,
                bridge_cancel,
                BridgeConfig::default(),
            )
            .await;
        });

        tokio::spawn(async move {
            let _ = crate::page::run(page, page_rx).await;
        });

        self.pages.write().await.insert(
            aid.clone(),
            BrowserHandle {
                page_tx,
                snapshot_rx,
                cancel,
            },
        );
        Ok(())
    }

    /// Subscribe to the latest-snapshot watch channel for `aid`. The
    /// receiver immediately observes the most recent published snapshot.
    pub async fn watch(&self, aid: &ActivityId) -> Option<watch::Receiver<Arc<BrowserSnapshot>>> {
        self.pages
            .read()
            .await
            .get(aid)
            .map(|h| h.snapshot_rx.clone())
    }

    /// Forward an input message to `aid`'s page actor. Missing-ok.
    pub async fn send_input(&self, aid: &ActivityId, msg: BrowserClientMsg) {
        if let Some(h) = self.pages.read().await.get(aid).cloned() {
            let _ = h.page_tx.send(PageCommand::Input(msg)).await;
        }
    }

    /// Drive a navigation command for `aid`. Missing-ok.
    pub async fn navigate(&self, aid: &ActivityId, n: NavCommand) {
        if let Some(h) = self.pages.read().await.get(aid).cloned() {
            let _ = h.page_tx.send(PageCommand::Nav(n)).await;
        }
    }

    /// Resize the page's emulated viewport for `aid`. Missing-ok.
    pub async fn resize(&self, aid: &ActivityId, width: u32, height: u32) {
        if let Some(h) = self.pages.read().await.get(aid).cloned() {
            let _ = h.page_tx.send(PageCommand::Resize { width, height }).await;
        }
    }

    /// Ask the page actor for `aid` to pause its screencast. Used when the
    /// activity becomes inactive in the UI so Chromium stops encoding frames
    /// nobody is watching. Missing-ok.
    pub async fn pause_screencast(&self, aid: &ActivityId) {
        if let Some(h) = self.pages.read().await.get(aid).cloned() {
            let _ = h.page_tx.send(PageCommand::PauseScreencast).await;
        }
    }

    /// Resume screencast for `aid`. Phase 2 leaves this as a no-op on the
    /// bridge side (the page actor receives the command but does not restart
    /// screencast itself); the bridge wiring lands in Phase 3.
    /// Missing-ok.
    pub async fn resume_screencast(&self, aid: &ActivityId) {
        if let Some(h) = self.pages.read().await.get(aid).cloned() {
            let _ = h.page_tx.send(PageCommand::ResumeScreencast).await;
        }
    }

    /// Idempotent, missing-ok close. Stops the bridge and page actor,
    /// then decrements the Chromium refcount (possibly scheduling teardown).
    pub async fn close(&self, aid: &ActivityId) {
        let Some(h) = self.pages.write().await.remove(aid) else {
            return;
        };
        h.cancel.cancel();
        let _ = h.page_tx.send(PageCommand::Close).await;

        let when_to_check = {
            let mut st = self.chromium.state.lock().await;
            st.detach();
            match st.snapshot() {
                Phase::StoppingAfter(when) => Some(when),
                _ => None,
            }
        };
        if let Some(when) = when_to_check {
            let grace = when.saturating_duration_since(std::time::Instant::now());
            let svc = self.clone();
            tokio::spawn(async move {
                tokio::time::sleep(grace).await;
                svc.maybe_teardown().await;
            });
        }
    }

    /// Check whether the grace period has elapsed; if so, tear Chromium
    /// down. Idempotent — safe to call after a re-attach has cancelled the
    /// pending shutdown.
    async fn maybe_teardown(&self) {
        let mut st = self.chromium.state.lock().await;
        if !st.grace_elapsed() {
            return;
        }
        drop(st);
        let mut browser_guard = self.chromium.browser.lock().await;
        if let Some(mut b) = browser_guard.take() {
            let _ = b.close().await;
            let _ = b.wait().await;
        }
    }

    async fn launch(&self) -> BrowserResult<()> {
        let user_data = self.chromium.runtime.root().join("browser");
        std::fs::create_dir_all(&user_data)?;
        let cfg = BrowserConfig::builder()
            .user_data_dir(user_data)
            // NOTE: required for screencast (Phase 0 finding).
            .new_headless_mode()
            .build()
            .map_err(BrowserError::Launch)?;
        let (browser, mut handler) = Browser::launch(cfg)
            .await
            .map_err(|e| BrowserError::Launch(e.to_string()))?;
        tokio::spawn(async move { while handler.next().await.is_some() {} });

        match import_chrome_default_cookies().await {
            Ok(cookies) if !cookies.is_empty() => {
                let count = cookies.len();
                match browser.set_cookies(cookies).await {
                    Ok(_) => tracing::info!(imported = count, "cookies imported"),
                    Err(e) => tracing::warn!(error = %e, "set_cookies failed"),
                }
            }
            Ok(_) => tracing::info!("no cookies to import"),
            Err(e) => tracing::warn!(error = %e, "cookie import failed; starting without cookies"),
        }

        *self.chromium.browser.lock().await = Some(browser);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn spawn_then_close_with_real_chromium() {
        if std::env::var("OZMUX_TEST_REAL_CHROME").ok().as_deref() != Some("1") {
            eprintln!("skipping; set OZMUX_TEST_REAL_CHROME=1 to run");
            return;
        }
        let tmp = tempdir().unwrap();
        let runtime = Arc::new(RuntimeRoot::new_in(tmp.path(), std::process::id()).unwrap());
        let svc = BrowserService::new(runtime);
        let aid = ActivityId::new();
        let wid = WindowId::new();
        let pid = PaneId::new();
        svc.spawn(&wid, &pid, &aid, Some("https://example.com".into()))
            .await
            .unwrap();
        let mut rx = svc.watch(&aid).await.unwrap();
        let _ = tokio::time::timeout(Duration::from_secs(10), rx.changed())
            .await
            .expect("first frame");
        svc.close(&aid).await;
    }
}
