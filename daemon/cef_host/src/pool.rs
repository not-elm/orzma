//! `BrowserPool` — owns CEF browser instances on the CEF UI thread (main).
//!
//! `BrowserPool` is `!Send` because it holds raw CEF objects. The Tokio worker
//! thread posts `CefCommand`s to the CEF UI thread via
//! `cef::post_task(ThreadId::UI, ExecuteTask)`; `BrowserPool::execute` runs on
//! the UI thread under the `PoolHandle` mutex.

use crate::frame_buffer_pool::FrameBufferPool;
use crate::handlers::client::OzmuxClient;
use crate::handlers::context_menu::OzmuxContextMenuHandler;
use crate::handlers::display::{NavInner, OzmuxDisplayHandler};
use crate::handlers::lifespan::OzmuxLifeSpanHandler;
use crate::handlers::load::OzmuxLoadHandler;
use crate::handlers::render::{OzmuxRenderHandler, RenderHandlerState};
use crate::post_command::PoolHandle;
use crate::profile::resolve_cache_path;
use cef::{
    Browser, BrowserSettings, CefString, Client, ImplBrowser, ImplBrowserHost, ImplFrame,
    RequestContext, RequestContextSettings, WindowInfo, browser_host_create_browser_sync,
    request_context_create_context,
};
use ozmux_browser_cef_protocol::types::ActivityId;
use ozmux_browser_cef_protocol::wire::{
    BrowserExtraContext, BrowserProfileWire, CefCookieDto, HostEvent, InputEvent,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;

/// A command from the Tokio worker thread to the CEF UI thread.
#[derive(Debug)]
pub enum CefCommand {
    /// Create a new windowless browser for the given activity.
    ///
    /// On receipt, cookies are installed via `CefCookieManager::set_cookie`.
    /// After all cookies complete, a `CreateBrowserAfterCookies` is re-posted
    /// back to the UI thread before calling `browser_host_create_browser_sync`.
    BrowserCreate {
        aid: ActivityId,
        initial_url: String,
        epoch: u32,
        cookies: Vec<CefCookieDto>,
        profile: BrowserProfileWire,
        context: BrowserExtraContext,
    },
    /// Internal: fires after all cookies from `BrowserCreate` have been
    /// committed to `CefCookieManager`. The UI thread then calls
    /// `browser_host_create_browser_sync` so the first navigation carries the
    /// cookies. Posted from the CEF UI thread (after `install_cookies`'s
    /// callback fires) via `post_command::post`.
    CreateBrowserAfterCookies {
        aid: ActivityId,
        initial_url: String,
        epoch: u32,
        profile: BrowserProfileWire,
        context: BrowserExtraContext,
    },
    /// Resize the browser viewport.
    Resize {
        aid: ActivityId,
        css_w: u32,
        css_h: u32,
        dpr: f32,
    },
    /// Close a browser.
    Close { aid: ActivityId },
    /// Shut down the message loop.
    Shutdown,
    /// Forward a user input event to the browser.
    SendInput { aid: ActivityId, event: InputEvent },
    /// Navigate an activity to a new URL.
    Navigate { aid: ActivityId, url: String },
    /// Navigate backward (`delta < 0`) or forward (`delta > 0`) in history.
    NavigateHistory { aid: ActivityId, delta: i64 },
    /// Pause screencast frame production for an activity.
    PauseScreencast { aid: ActivityId },
    /// Resume screencast frame production for an activity and force a keyframe.
    ResumeScreencast { aid: ActivityId },
    /// No-op command. Used by the in-process dispatcher to absorb
    /// daemon-facing `HostCommand` variants that are not meaningful in-process
    /// (the OoP handshake `Ready`, the `BrowserCreate` path that goes through
    /// a dedicated method, and unimplemented variants like `RecreateShm`).
    Noop,
}

/// Holds the live state for one browser activity.
pub struct BrowserEntry {
    pub aid: ActivityId,
    pub epoch: u32,
    pub browser: Browser,
    /// Render-handler state — width / height / dpr / force_keyframe — shared
    /// with the active `OzmuxRenderHandler` so `CefCommand::Resize` can
    /// update the viewport without rebuilding the handler.
    pub render_state: Arc<RenderHandlerState>,
    /// Storage profile this browser was created with. Used on `Close` to
    /// release the named-profile `RequestContext` refcount.
    pub profile: BrowserProfileWire,
}

/// Maximum viewport, in device pixels. The Resize handler clamps to this so
/// a single in-process frame stays within a few hundred MB even on 4K
/// displays.
const MAX_VIEWPORT_W: u32 = 3840;
/// Maximum viewport height in device pixels. See [`MAX_VIEWPORT_W`].
const MAX_VIEWPORT_H: u32 = 2160;

/// Manages all live browser instances on the CEF UI thread.
pub struct BrowserPool {
    browsers: HashMap<ActivityId, BrowserEntry>,
    /// Sender into the cef_host → daemon event channel. Each created browser
    /// receives a clone so it can emit `HostEvent::NavStateChanged` from
    /// `DisplayHandler` and `LoadHandler` callbacks.
    event_tx: mpsc::UnboundedSender<HostEvent>,
    /// Observability flag: set to `true` after a `Shutdown` command is dispatched.
    ///
    /// Does **not** drive the message loop — `cef::quit_message_loop()` does.
    /// Read via [`PoolHandle::snapshot_shutdown_requested`] /
    /// [`PoolHandle::force_shutdown`] so external observers can detect that a
    /// graceful shutdown was requested.
    pub shutdown_requested: bool,
    /// Back-reference to the pool's own handle, planted by `PoolHandle::new`
    /// after construction. Used by the cookie-install callback to re-post
    /// `CreateBrowserAfterCookies` back to the UI thread from the CEF IO thread.
    pub(crate) pool_handle: Option<PoolHandle>,
    /// Daemon-wide session identifier stamped onto every emitted
    /// `HostEvent::FrameProduced`. The matching `FrameRing` is constructed with
    /// the same id so subscribers can detect a daemon restart via mismatch.
    session_id: u64,
    /// Recycler for the BGRA buffers produced by `RenderHandler::on_paint`.
    /// Shared across every browser in the pool so 60 fps × 33 MB-frame loads
    /// reuse allocations instead of stressing the global allocator.
    frame_pool: Arc<FrameBufferPool>,
    /// Disk-persistent named-profile request contexts, keyed by profile name.
    /// Shared across activities naming the same profile; ref-counted by
    /// `named_refcounts` and dropped when the last activity closes.
    named_contexts: HashMap<String, RequestContext>,
    /// Live activity count per named profile, for `named_contexts` GC.
    named_refcounts: HashMap<String, usize>,
    /// `RequestContext`s resolved at `BrowserCreate` time (for cookie seeding)
    /// and held until the matching `CreateBrowserAfterCookies` consumes them.
    /// Required for incognito profiles, where `request_context_for` would
    /// otherwise mint a fresh context and lose the seeded cookies; named
    /// profiles route through here too for uniformity (harmless — they are
    /// cached, so re-resolution returns the same instance).
    pending_contexts: HashMap<ActivityId, RequestContext>,
    /// Absolute CEF `root_cache_path` (parent of every named profile dir).
    root_cache_path: PathBuf,
    /// Whether this daemon owns the data-root lock. Currently unused —
    /// `effective_profile` and `create_request_context` ignore it because
    /// disk persistence is disabled pool-wide. Kept so the lock signal stays
    /// wired for the future Chrome profile-naming work.
    #[allow(
        dead_code,
        reason = "Persistence demotion is paused until Chrome profile-naming is fixed; \
                  the flag stays so the lock plumbing does not need re-introduction later."
    )]
    persistent_profiles_enabled: bool,
}

impl BrowserPool {
    /// Creates an empty pool.
    ///
    /// `event_tx` is an unbounded sender into the cef_host event channel;
    /// it is cloned into each `NavInner` so display and load handlers can
    /// emit `HostEvent::NavStateChanged` to the daemon.
    ///
    /// `pool_handle` is `None` here; `PoolHandle::new` plants the back-reference
    /// after wrapping the pool so cookie-install callbacks can re-post commands.
    ///
    /// `root_cache_path` is the absolute parent directory under which named
    /// profiles get their disk-persistent `RequestContext` cache directories.
    ///
    /// `persistent_profiles_enabled` is `false` when another daemon holds the
    /// data-root lock; named profiles are then demoted to incognito storage.
    ///
    /// `session_id` is the daemon-wide identifier stamped onto every
    /// `HostEvent::FrameProduced`; the matching `BrowserCefRegistry` is built
    /// with the same id.
    ///
    /// `frame_pool` recycles the BGRA buffers each `RenderHandler::on_paint`
    /// allocates, shared across every browser in this pool.
    pub fn new(
        event_tx: mpsc::UnboundedSender<HostEvent>,
        root_cache_path: PathBuf,
        persistent_profiles_enabled: bool,
        session_id: u64,
        frame_pool: Arc<FrameBufferPool>,
    ) -> Self {
        Self {
            browsers: HashMap::new(),
            event_tx,
            shutdown_requested: false,
            pool_handle: None,
            session_id,
            frame_pool,
            named_contexts: HashMap::new(),
            named_refcounts: HashMap::new(),
            pending_contexts: HashMap::new(),
            root_cache_path,
            persistent_profiles_enabled,
        }
    }

    /// Drains and executes a single command. Must be called from the CEF UI thread.
    pub fn execute(&mut self, cmd: CefCommand) {
        tracing::debug!(?cmd, "execute");
        match cmd {
            CefCommand::BrowserCreate {
                aid,
                initial_url,
                epoch,
                cookies,
                profile,
                context,
            } => self.handle_browser_create(aid, initial_url, epoch, cookies, profile, context),
            CefCommand::CreateBrowserAfterCookies {
                aid,
                initial_url,
                epoch,
                profile,
                context,
            } => self.handle_create_after_cookies(aid, initial_url, epoch, profile, context),
            CefCommand::Resize {
                aid,
                css_w,
                css_h,
                dpr,
            } => self.handle_resize(aid, css_w, css_h, dpr),
            CefCommand::Close { aid } => self.handle_close(aid),
            CefCommand::Shutdown => self.handle_shutdown(),
            CefCommand::SendInput { aid, event } => self.handle_send_input(aid, event),
            CefCommand::Navigate { aid, url } => self.handle_navigate(aid, url),
            CefCommand::NavigateHistory { aid, delta } => {
                self.handle_navigate_history(aid, delta);
            }
            CefCommand::PauseScreencast { aid } => self.handle_pause_screencast(aid),
            CefCommand::ResumeScreencast { aid } => self.handle_resume_screencast(aid),
            CefCommand::Noop => {}
        }
    }

    /// Handles `CefCommand::BrowserCreate` by seeding cookies and posting `CreateBrowserAfterCookies`.
    fn handle_browser_create(
        &mut self,
        aid: ActivityId,
        initial_url: String,
        epoch: u32,
        cookies: Vec<CefCookieDto>,
        profile: BrowserProfileWire,
        context: BrowserExtraContext,
    ) {
        tracing::info!(
            ?aid,
            cookie_count = cookies.len(),
            "BrowserCreate: installing cookies"
        );
        let aid2 = aid.clone();
        let pool_handle = self
            .pool_handle
            .clone()
            .expect("pool_handle not set; PoolHandle::new must plant it before commands arrive");
        let Some(ctx) = self.request_context_for(&profile) else {
            tracing::error!(?aid, "RequestContext unavailable; aborting BrowserCreate");
            return;
        };
        self.pending_contexts.insert(aid.clone(), ctx.clone());
        crate::cookies::install_cookies(cookies, &ctx, move || {
            if let Err(e) = crate::post_command::post(
                &pool_handle,
                CefCommand::CreateBrowserAfterCookies {
                    aid: aid2,
                    initial_url,
                    epoch,
                    profile,
                    context,
                },
            ) {
                tracing::error!(error = %e, "failed to post CreateBrowserAfterCookies");
                // TODO: on post() failure the pending_contexts[aid] entry
                // leaks until process exit; cleaning up requires re-entering
                // the pool from this closure (e.g. another post_command::post
                // of a cleanup command). Out of scope for the cookie-context
                // wiring.
            }
        });
    }

    /// Handles `CefCommand::CreateBrowserAfterCookies` by delegating to `create_browser`.
    fn handle_create_after_cookies(
        &mut self,
        aid: ActivityId,
        initial_url: String,
        epoch: u32,
        profile: BrowserProfileWire,
        context: BrowserExtraContext,
    ) {
        self.create_browser(aid, initial_url, epoch, profile, context);
    }

    /// Handles `CefCommand::Resize` by clamping to `MAX_VIEWPORT_W`/`MAX_VIEWPORT_H` and updating render state.
    fn handle_resize(&mut self, aid: ActivityId, css_w: u32, css_h: u32, dpr: f32) {
        let Some(entry) = self.browsers.get(&aid) else {
            tracing::warn!(?aid, "Resize: unknown activity");
            return;
        };
        let dpr = if dpr > 0.0 { dpr } else { 1.0 };
        let max_css_w = ((MAX_VIEWPORT_W as f32 / dpr) as u32).max(1);
        let max_css_h = ((MAX_VIEWPORT_H as f32 / dpr) as u32).max(1);
        let view_w = css_w.clamp(1, max_css_w);
        let view_h = css_h.clamp(1, max_css_h);
        if view_w != css_w || view_h != css_h {
            tracing::warn!(
                ?aid,
                css_w,
                css_h,
                view_w,
                view_h,
                "Resize clamped so css×dpr fits the 4K shm budget"
            );
        }
        entry.render_state.width.set(view_w);
        entry.render_state.height.set(view_h);
        entry.render_state.dpr.set(dpr);
        entry.render_state.force_keyframe.set(true);
        if let Some(host) = entry.browser.host() {
            host.was_resized();
            host.notify_screen_info_changed();
        }
        tracing::debug!(?aid, css_w, css_h, dpr, "Resize dispatched");
    }

    /// Handles `CefCommand::Close` by tearing down the browser and releasing its profile refcount.
    fn handle_close(&mut self, aid: ActivityId) {
        tracing::info!(?aid, "Close");
        self.pending_contexts.remove(&aid);
        if let Some(entry) = self.browsers.remove(&aid) {
            let host = entry.browser.host();
            if let Some(h) = host {
                h.close_browser(1);
            }
            self.release_profile(&entry.profile);
        }
    }

    /// Handles `CefCommand::Shutdown` by quitting the CEF message loop.
    fn handle_shutdown(&mut self) {
        tracing::info!("Shutdown requested");
        cef::quit_message_loop();
        self.shutdown_requested = true;
    }

    /// Handles `CefCommand::SendInput` by dispatching the event to the target browser.
    fn handle_send_input(&self, aid: ActivityId, event: InputEvent) {
        if let Some(entry) = self.browsers.get(&aid) {
            crate::input::dispatch(&entry.browser, &aid, event);
        } else {
            tracing::warn!(?aid, "SendInput: unknown activity");
        }
    }

    /// Handles `CefCommand::Navigate` by loading `url` in the browser's main frame.
    fn handle_navigate(&self, aid: ActivityId, url: String) {
        let Some(entry) = self.browsers.get(&aid) else {
            tracing::warn!(?aid, "Navigate: unknown activity");
            return;
        };
        let Some(frame) = entry.browser.main_frame() else {
            tracing::warn!(?aid, "Navigate: no main frame");
            return;
        };
        tracing::debug!(?aid, %url, "Navigate");
        frame.load_url(Some(&CefString::from(url.as_str())));
    }

    /// Handles `CefCommand::NavigateHistory` by going back or forward based on `delta`'s sign.
    fn handle_navigate_history(&self, aid: ActivityId, delta: i64) {
        let Some(entry) = self.browsers.get(&aid) else {
            tracing::warn!(?aid, "NavigateHistory: unknown activity");
            return;
        };
        match delta.signum() {
            -1 => {
                if entry.browser.can_go_back() != 0 {
                    tracing::debug!(?aid, "NavigateHistory back");
                    entry.browser.go_back();
                } else {
                    tracing::debug!(?aid, "NavigateHistory back: no back history");
                }
            }
            1 => {
                if entry.browser.can_go_forward() != 0 {
                    tracing::debug!(?aid, "NavigateHistory forward");
                    entry.browser.go_forward();
                } else {
                    tracing::debug!(?aid, "NavigateHistory forward: no forward history");
                }
            }
            _ => {
                tracing::warn!(?aid, delta, "NavigateHistory: delta is zero, no-op");
            }
        }
    }

    /// Handles `CefCommand::PauseScreencast` by hiding the browser so CEF stops producing frames.
    fn handle_pause_screencast(&self, aid: ActivityId) {
        let Some(entry) = self.browsers.get(&aid) else {
            tracing::warn!(?aid, "PauseScreencast: unknown activity");
            return;
        };
        if let Some(host) = entry.browser.host() {
            tracing::debug!(?aid, "PauseScreencast");
            host.was_hidden(1);
        }
    }

    /// Handles `CefCommand::ResumeScreencast` by un-hiding the browser and forcing a keyframe.
    fn handle_resume_screencast(&self, aid: ActivityId) {
        let Some(entry) = self.browsers.get(&aid) else {
            tracing::warn!(?aid, "ResumeScreencast: unknown activity");
            return;
        };
        if let Some(host) = entry.browser.host() {
            tracing::debug!(?aid, "ResumeScreencast");
            host.was_hidden(0);
        }
        crate::input::invalidate_view(&entry.browser, &aid);
    }

    fn create_browser(
        &mut self,
        aid: ActivityId,
        initial_url: String,
        epoch: u32,
        profile: BrowserProfileWire,
        // TODO: Task 4 — forward `context` into CEF `extra_info` so the render
        // process can build `window.ozmux.context` in `on_context_created`.
        _context: BrowserExtraContext,
    ) {
        tracing::info!(?aid, %initial_url, epoch, "BrowserCreate");

        let effective_profile = self.effective_profile(&profile);

        let mut request_context = match self.take_pending_context(&aid) {
            Some(c) => c,
            None => {
                tracing::info!(
                    ?aid,
                    "pending RequestContext evicted by Close; aborting BrowserCreate"
                );
                self.discard_unretained_context(&effective_profile);
                return;
            }
        };

        let render_state = Arc::new(RenderHandlerState::new(1280, 800, 1.0));
        let mut client = build_client(
            aid.clone(),
            self.event_tx.clone(),
            Arc::clone(&render_state),
            Arc::clone(&self.frame_pool),
            self.session_id,
            epoch,
        );
        let window_info = build_window_info();
        let browser_settings = build_browser_settings();
        let url_str = CefString::from(initial_url.as_str());

        let browser = browser_host_create_browser_sync(
            Some(&window_info),
            Some(&mut client),
            Some(&url_str),
            Some(&browser_settings),
            None,
            Some(&mut request_context),
        );

        match browser {
            Some(b) => {
                tracing::info!(?aid, "browser created successfully");
                if let Some(host) = b.host() {
                    host.was_hidden(0);
                    host.notify_screen_info_changed();
                }
                self.retain_profile(&effective_profile);
                self.browsers.insert(
                    aid.clone(),
                    BrowserEntry {
                        aid,
                        epoch,
                        browser: b,
                        render_state,
                        profile: effective_profile,
                    },
                );
            }
            None => {
                tracing::error!(?aid, "browser_host_create_browser_sync returned None");
                self.discard_unretained_context(&effective_profile);
            }
        }
    }

    /// Obtains the `RequestContext` for `profile`. Named profiles are created
    /// once and cached in `named_contexts` (shared across activities); the
    /// caller must call `retain_profile` to bump the refcount. Incognito
    /// profiles get a fresh in-memory context every call (never cached).
    ///
    /// Returns `None` if context creation fails — the caller MUST treat this
    /// as a `BrowserCreate` failure and NOT fall back to the global context.
    fn request_context_for(&mut self, profile: &BrowserProfileWire) -> Option<RequestContext> {
        // NOTE: validate the profile name even though we ignore the resolved
        // cache_path — keeps untrusted input checks active and surfaces a
        // failure early if a malformed Named arrives over the wire.
        if let Err(e) = resolve_cache_path(&self.root_cache_path, profile) {
            tracing::error!(error = %e, "invalid browser profile; rejecting BrowserCreate");
            return None;
        }
        match profile {
            BrowserProfileWire::Named { name } => {
                if let Some(ctx) = self.named_contexts.get(name) {
                    return Some(ctx.clone());
                }
                let ctx = create_request_context()?;
                self.named_contexts.insert(name.clone(), ctx.clone());
                Some(ctx)
            }
            BrowserProfileWire::Incognito => create_request_context(),
        }
    }

    /// Consumes the `RequestContext` stashed by `CefCommand::BrowserCreate` for
    /// `aid`. `None` means `Close` already evicted the stash (the activity is
    /// being torn down); the caller must abort browser creation.
    fn take_pending_context(&mut self, aid: &ActivityId) -> Option<RequestContext> {
        self.pending_contexts.remove(aid)
    }

    /// Resolves the profile a browser is effectively created with. Currently a
    /// no-op clone: with disk persistence disabled across the pool, Named and
    /// Incognito differ only in whether the in-memory `RequestContext` is
    /// shared by name across activities — there is no on-disk state to gate
    /// behind `persistent_profiles_enabled`. The hook stays for future
    /// re-enablement once Chrome profile-naming is solved.
    fn effective_profile(&self, requested: &BrowserProfileWire) -> BrowserProfileWire {
        requested.clone()
    }

    /// Increments the live-activity refcount for a named profile.
    fn retain_profile(&mut self, profile: &BrowserProfileWire) {
        if let BrowserProfileWire::Named { name } = profile {
            *self.named_refcounts.entry(name.clone()).or_insert(0) += 1;
        }
    }

    /// Decrements the refcount; drops the cached in-memory `RequestContext`
    /// at zero. No on-disk state to clean up — disk persistence is currently
    /// disabled (see `create_request_context`).
    fn release_profile(&mut self, profile: &BrowserProfileWire) {
        if let BrowserProfileWire::Named { name } = profile
            && let Some(c) = self.named_refcounts.get_mut(name)
        {
            *c = c.saturating_sub(1);
            if *c == 0 {
                self.named_refcounts.remove(name);
                self.named_contexts.remove(name);
                tracing::debug!(profile = %name, "named RequestContext dropped (refcount 0)");
            }
        }
    }

    /// Drops a named `RequestContext` created for a `BrowserCreate` that then
    /// failed before any activity retained it. A context still referenced by
    /// a live activity (refcount entry present) is left intact.
    fn discard_unretained_context(&mut self, profile: &BrowserProfileWire) {
        if let BrowserProfileWire::Named { name } = profile
            && !self.named_refcounts.contains_key(name)
        {
            self.named_contexts.remove(name);
        }
    }

    /// Returns the number of active browsers.
    pub fn browser_count(&self) -> usize {
        self.browsers.len()
    }
}

/// Builds the `OzmuxClient` wrapping every per-browser handler.
fn build_client(
    aid: ActivityId,
    event_tx: mpsc::UnboundedSender<HostEvent>,
    render_state: Arc<RenderHandlerState>,
    frame_pool: Arc<FrameBufferPool>,
    session_id: u64,
    epoch: u32,
) -> Client {
    let render_handler = OzmuxRenderHandler::new(
        aid.clone(),
        render_state,
        event_tx.clone(),
        frame_pool,
        session_id,
        epoch,
    );
    let life_span_handler = OzmuxLifeSpanHandler::new(aid.clone());
    let nav_inner = NavInner::new(aid, event_tx);
    let display_handler = OzmuxDisplayHandler::new(nav_inner.clone());
    let load_handler = OzmuxLoadHandler::new(nav_inner);
    let context_menu_handler = OzmuxContextMenuHandler::new();
    OzmuxClient::new(
        render_handler,
        life_span_handler,
        display_handler,
        load_handler,
        context_menu_handler,
    )
}

/// Builds the `WindowInfo` for a windowless (offscreen-rendered) browser.
#[expect(
    clippy::field_reassign_with_default,
    reason = "WindowInfo::default() uses unsafe zeroed() with size field; struct-literal form is impractical due to raw pointer fields"
)]
fn build_window_info() -> WindowInfo {
    let mut window_info = WindowInfo::default();
    window_info.windowless_rendering_enabled = 1;
    window_info
}

/// Builds the `BrowserSettings` applied to every CEF browser this host owns.
fn build_browser_settings() -> BrowserSettings {
    BrowserSettings {
        // TODO: expose windowless_frame_rate via daemon config.
        windowless_frame_rate: 30,
        ..BrowserSettings::default()
    }
}

/// Creates a fresh in-memory `RequestContext` (empty `cache_path`, CEF
/// "incognito" storage). Per-activity isolation works via the distinct
/// `RequestContext` objects regardless of disk persistence.
///
/// Disk persistence via `RequestContextSettings.cache_path` is intentionally
/// disabled. Setting a per-`RequestContext` `cache_path` under
/// `CefSettings.root_cache_path` caused CEF's Chrome runtime to log
/// `Cannot create profile at path .../profiles/<name>` and silently funnel
/// storage back to the shared `<root>/Default/` directory, defeating
/// per-activity isolation. Re-enabling persistence needs Chrome's
/// profile-naming convention (`Default`/`Profile N` registered in
/// `Local State`) — tracked as a future task.
/// `RequestContextSettings::default()` populates `size` to
/// `size_of::<_cef_request_context_settings_t>()`; no manual sizing needed.
fn create_request_context() -> Option<RequestContext> {
    let settings = RequestContextSettings::default();
    let ctx = request_context_create_context(Some(&settings), None);
    if ctx.is_none() {
        tracing::error!("request_context_create_context returned None");
    }
    ctx
}
