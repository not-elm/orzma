//! `BrowserPool` — owns CEF browser instances on the CEF UI thread (main).
//!
//! `BrowserPool` is `!Send` because it holds raw CEF objects. The Tokio worker
//! thread posts `CefCommand`s to the CEF UI thread via
//! `cef::post_task(ThreadId::UI, ExecuteTask)`; `BrowserPool::execute` runs on
//! the UI thread under the `PoolHandle` mutex.

use crate::handlers::client::OzmuxClient;
use crate::handlers::context_menu::OzmuxContextMenuHandler;
use crate::handlers::display::{NavInner, OzmuxDisplayHandler};
use crate::handlers::lifespan::OzmuxLifeSpanHandler;
use crate::handlers::load::OzmuxLoadHandler;
use crate::handlers::render::{OzmuxRenderHandler, RenderHandlerState};
use crate::post_command::PoolHandle;
use crate::profile::resolve_cache_path;
use crate::shm_writer::ShmWriter;
use cef::{
    Browser, BrowserSettings, CefString, ImplBrowser, ImplBrowserHost, ImplFrame, RequestContext,
    RequestContextSettings, WindowInfo, browser_host_create_browser_sync,
    request_context_create_context,
};
use ozmux_browser_cef_protocol::types::ActivityId;
use ozmux_browser_cef_protocol::wire::{BrowserProfileWire, CefCookieDto, HostEvent, InputEvent};
use std::collections::HashMap;
use std::os::fd::RawFd;
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
    /// `shm_fd` arrives per-BrowserCreate via SCM_RIGHTS.
    BrowserCreate {
        aid: ActivityId,
        initial_url: String,
        epoch: u32,
        shm_fd: RawFd,
        cookies: Vec<CefCookieDto>,
        profile: BrowserProfileWire,
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
        shm_fd: RawFd,
        profile: BrowserProfileWire,
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
}

/// Holds the live state for one browser activity.
pub struct BrowserEntry {
    pub aid: ActivityId,
    pub epoch: u32,
    pub shm_fd: RawFd,
    pub browser: Browser,
    pub shm: Arc<ShmWriter>,
    /// Render-handler state — width / height / dpr / force_keyframe — shared
    /// with the active `OzmuxRenderHandler` so `CefCommand::Resize` can
    /// update the viewport without rebuilding the handler.
    pub render_state: Arc<RenderHandlerState>,
    /// Storage profile this browser was created with. Used on `Close` to
    /// release the named-profile `RequestContext` refcount.
    pub profile: BrowserProfileWire,
}

/// Per-slot payload budget: a 4K (3840×2160) BGRA frame + 4 KiB slack.
/// MUST stay byte-identical to `ozmux_browser::shm_alloc::SLOT_PAYLOAD_MAX`.
const SLOT_PAYLOAD_MAX: usize = 3840 * 2160 * 4 + 4096;

/// Maximum viewport the fixed shm slot can hold, in device pixels. The Resize
/// handler clamps to this; a pane larger than 4K device pixels renders clipped.
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
    /// Whether this daemon owns the data-root lock. When `false`, another
    /// daemon holds it, so named profiles are demoted to incognito storage.
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
    pub fn new(
        event_tx: mpsc::UnboundedSender<HostEvent>,
        root_cache_path: PathBuf,
        persistent_profiles_enabled: bool,
    ) -> Self {
        Self {
            browsers: HashMap::new(),
            event_tx,
            shutdown_requested: false,
            pool_handle: None,
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
                shm_fd,
                cookies,
                profile,
            } => {
                tracing::info!(
                    ?aid,
                    cookie_count = cookies.len(),
                    "BrowserCreate: installing cookies"
                );
                let aid2 = aid.clone();
                let pool_handle = self.pool_handle.clone().expect(
                    "pool_handle not set; PoolHandle::new must plant it before commands arrive",
                );
                let Some(ctx) = self.request_context_for(&profile) else {
                    tracing::error!(?aid, "RequestContext unavailable; aborting BrowserCreate");
                    // SAFETY: shm_fd was duped from the daemon side via
                    // SCM_RIGHTS and is owned here; close it before bailing.
                    unsafe { libc::close(shm_fd) };
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
                            shm_fd,
                            profile,
                        },
                    ) {
                        tracing::error!(error = %e, "failed to post CreateBrowserAfterCookies");
                        // TODO: on post() failure both shm_fd and the
                        // pending_contexts[aid] entry leak until process exit;
                        // cleaning up requires re-entering the pool from this
                        // closure (e.g. another post_command::post of a cleanup
                        // command). Out of scope for the cookie-context wiring.
                    }
                });
            }
            CefCommand::CreateBrowserAfterCookies {
                aid,
                initial_url,
                epoch,
                shm_fd,
                profile,
            } => {
                self.create_browser(aid, initial_url, epoch, shm_fd, profile);
            }
            CefCommand::Resize {
                aid,
                css_w,
                css_h,
                dpr,
            } => {
                let Some(entry) = self.browsers.get(&aid) else {
                    tracing::warn!(?aid, "Resize: unknown activity");
                    return;
                };
                // NOTE: CefRenderHandler::view_rect must report DIP (CSS)
                // pixels; CEF multiplies by device_scale_factor (the dpr we
                // return from screen_info) to size the OnPaint buffer. So
                // render_state stores CSS px, NOT css×dpr — passing device
                // pixels here would double-apply the dpr.
                let dpr = if dpr > 0.0 { dpr } else { 1.0 };
                // The shm slot holds a 4K *physical* frame, so cap the CSS
                // viewport such that css×dpr stays within MAX_VIEWPORT_*.
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
                // Force a fresh keyframe so the renderer rebuilds at the new size.
                entry.render_state.force_keyframe.set(true);
                if let Some(host) = entry.browser.host() {
                    host.was_resized();
                    host.notify_screen_info_changed();
                }
                tracing::debug!(?aid, css_w, css_h, dpr, "Resize dispatched");
            }
            CefCommand::Close { aid } => {
                tracing::info!(?aid, "Close");
                // NOTE: drop any context stashed by a still-in-flight
                // BrowserCreate whose CreateBrowserAfterCookies has not fired
                // yet. Evicting the stash makes the late CreateBrowserAfterCookies
                // observe `None` from `take_pending_context` and abort cleanly.
                // No `release_profile` is needed for the in-flight case:
                // `retain_profile` only fires inside `create_browser` after the
                // `BrowserEntry` is inserted, which has not happened yet here.
                self.pending_contexts.remove(&aid);
                if let Some(entry) = self.browsers.remove(&aid) {
                    // NOTE: CloseBrowser triggers OnBeforeClose which drops the CEF handle.
                    let host = entry.browser.host();
                    if let Some(h) = host {
                        h.close_browser(1);
                    }
                    // SAFETY: shm_fd was duped from the daemon side and is owned here.
                    unsafe {
                        libc::close(entry.shm_fd);
                    }
                    self.release_profile(&entry.profile);
                }
            }
            CefCommand::Shutdown => {
                tracing::info!("Shutdown requested");
                // NOTE: execute() always runs on the CEF UI thread (via ExecuteTask),
                // so calling quit_message_loop() directly is safe and avoids an extra
                // post_task round-trip that would be needed from a non-UI caller.
                cef::quit_message_loop();
                // NOTE: shutdown_requested is still set so snapshot_shutdown_requested
                // observers can detect the graceful shutdown, even though it no longer
                // drives the message loop.
                self.shutdown_requested = true;
            }
            CefCommand::SendInput { aid, event } => {
                if let Some(entry) = self.browsers.get(&aid) {
                    crate::input::dispatch(&entry.browser, &aid, event);
                } else {
                    tracing::warn!(?aid, "SendInput: unknown activity");
                }
            }
            CefCommand::Navigate { aid, url } => {
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
            CefCommand::NavigateHistory { aid, delta } => {
                let Some(entry) = self.browsers.get(&aid) else {
                    tracing::warn!(?aid, "NavigateHistory: unknown activity");
                    return;
                };
                match delta.signum() {
                    -1 => {
                        // NOTE: guard on can_go_back — calling go_back with no
                        // back entry is a wasted round-trip into Chromium.
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
            CefCommand::PauseScreencast { aid } => {
                let Some(entry) = self.browsers.get(&aid) else {
                    tracing::warn!(?aid, "PauseScreencast: unknown activity");
                    return;
                };
                if let Some(host) = entry.browser.host() {
                    tracing::debug!(?aid, "PauseScreencast");
                    host.was_hidden(1);
                }
            }
            CefCommand::ResumeScreencast { aid } => {
                let Some(entry) = self.browsers.get(&aid) else {
                    tracing::warn!(?aid, "ResumeScreencast: unknown activity");
                    return;
                };
                if let Some(host) = entry.browser.host() {
                    tracing::debug!(?aid, "ResumeScreencast");
                    host.was_hidden(0);
                }
                // NOTE: invalidate forces a fresh keyframe after the browser is
                // marked visible again; without this the first frame may not arrive
                // until the next scheduled repaint.
                crate::input::invalidate_view(&entry.browser, &aid);
            }
        }
    }

    #[expect(
        clippy::field_reassign_with_default,
        reason = "WindowInfo::default() uses unsafe zeroed() with size field; struct-literal form is impractical due to raw pointer fields"
    )]
    fn create_browser(
        &mut self,
        aid: ActivityId,
        initial_url: String,
        epoch: u32,
        shm_fd: RawFd,
        profile: BrowserProfileWire,
    ) {
        tracing::info!(?aid, %initial_url, epoch, shm_fd, "BrowserCreate");

        // NOTE: when persistence is disabled a Named profile behaves exactly
        // like Incognito. The effective profile is what gets refcounted and
        // stored in BrowserEntry so retain/release stay consistent with the
        // contexts actually cached in `named_contexts`.
        let effective_profile = self.effective_profile(&profile);

        let total_size = ShmWriter::required_region_size(SLOT_PAYLOAD_MAX);
        // SAFETY: shm_fd is a valid mmap-able fd received from the daemon side
        // via SCM_RIGHTS in `control::recv_command_with_fd` (per-BrowserCreate
        // since Task A5). We map it shared so the daemon can read frames
        // written by the CEF UI thread.
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                total_size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                shm_fd,
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            tracing::error!(?aid, "mmap failed for shm_fd={shm_fd}");
            return;
        }

        // SAFETY: ptr is a valid mmap region of total_size bytes, writable,
        // and we are the sole writer on the CEF UI thread.
        let shm = Arc::new(unsafe { ShmWriter::from_mmap(ptr as *mut u8, SLOT_PAYLOAD_MAX) });

        let mut request_context = match self.take_pending_context(&aid) {
            Some(c) => c,
            None => {
                // NOTE: stash absent means `Close` arrived while cookies were
                // installing and evicted `pending_contexts[aid]`. The activity
                // is being torn down — abort browser creation cleanly instead
                // of minting a fresh context that would produce a ghost
                // browser nothing tracks.
                tracing::info!(
                    ?aid,
                    "pending RequestContext evicted by Close; aborting BrowserCreate"
                );
                // SAFETY: ptr was mmap'd above; unmap before bailing.
                unsafe { libc::munmap(ptr, total_size) };
                // SAFETY: shm_fd was duped from the daemon side and is owned here.
                unsafe { libc::close(shm_fd) };
                // NOTE: if this BrowserCreate cached a fresh named-profile
                // RequestContext in `named_contexts` that no other activity
                // has retained, drop it here. `discard_unretained_context`
                // is a no-op for Incognito and for already-retained Named
                // profiles, so this is safe to call unconditionally.
                self.discard_unretained_context(&effective_profile);
                return;
            }
        };

        let render_state = Arc::new(RenderHandlerState::new(1280, 800, 1.0));
        let render_handler = OzmuxRenderHandler::new(
            aid.clone(),
            shm.clone(),
            render_state.clone(),
            self.event_tx.clone(),
        );
        let life_span_handler = OzmuxLifeSpanHandler::new(aid.clone());
        let nav_inner = NavInner::new(aid.clone(), self.event_tx.clone());
        let display_handler = OzmuxDisplayHandler::new(nav_inner.clone());
        let load_handler = OzmuxLoadHandler::new(nav_inner);
        let context_menu_handler = OzmuxContextMenuHandler::new();
        let mut client = OzmuxClient::new(
            render_handler,
            life_span_handler,
            display_handler,
            load_handler,
            context_menu_handler,
        );

        let mut window_info = WindowInfo::default();
        // NOTE: windowless_rendering_enabled = 1 enables OSR (off-screen rendering),
        // which is required for on_paint callbacks to fire without a native window.
        window_info.windowless_rendering_enabled = 1;

        let browser_settings = BrowserSettings {
            // NOTE: windowless_frame_rate tells CEF how often to schedule OnPaint callbacks.
            // 30 fps is sufficient for the PoC; Plan 2 will expose this as a config option.
            windowless_frame_rate: 30,
            ..BrowserSettings::default()
        };
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
                // NOTE: In OSR mode, CEF starts hidden. Call was_hidden(0) to mark the browser
                // visible and trigger the first OnPaint once the page loads.
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
                        shm_fd,
                        browser: b,
                        shm,
                        render_state,
                        profile: effective_profile,
                    },
                );
            }
            None => {
                tracing::error!(?aid, "browser_host_create_browser_sync returned None");
                // SAFETY: ptr was mmap'd above; we own it and no BrowserEntry took it.
                unsafe { libc::munmap(ptr, total_size) };
                // SAFETY: shm_fd was duped from the daemon side; no BrowserEntry owns it on this path.
                unsafe { libc::close(shm_fd) };
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
        let cache_path = match resolve_cache_path(&self.root_cache_path, profile) {
            Ok(p) => p,
            Err(e) => {
                tracing::error!(error = %e, "invalid browser profile; rejecting BrowserCreate");
                return None;
            }
        };
        match profile {
            BrowserProfileWire::Named { name } => {
                if !self.persistent_profiles_enabled {
                    tracing::warn!(profile = %name, "persistence disabled; using incognito");
                    return create_request_context(None);
                }
                if let Some(ctx) = self.named_contexts.get(name) {
                    return Some(ctx.clone());
                }
                let dir = cache_path.expect("named profile yields Some cache_path");
                if let Err(e) = std::fs::create_dir_all(&dir) {
                    tracing::error!(error = %e, dir = %dir.display(), "create profile dir failed");
                    return None;
                }
                let ctx = create_request_context(Some(&dir))?;
                self.named_contexts.insert(name.clone(), ctx.clone());
                Some(ctx)
            }
            BrowserProfileWire::Incognito => create_request_context(None),
        }
    }

    /// Consumes the `RequestContext` stashed by `CefCommand::BrowserCreate` for
    /// `aid`. `None` means `Close` already evicted the stash (the activity is
    /// being torn down); the caller must abort browser creation.
    fn take_pending_context(&mut self, aid: &ActivityId) -> Option<RequestContext> {
        self.pending_contexts.remove(aid)
    }

    /// Resolves the profile a browser is effectively created with. A `Named`
    /// profile is demoted to `Incognito` when persistence is disabled; every
    /// other case clones the requested profile unchanged.
    fn effective_profile(&self, requested: &BrowserProfileWire) -> BrowserProfileWire {
        match requested {
            BrowserProfileWire::Named { .. } if !self.persistent_profiles_enabled => {
                BrowserProfileWire::Incognito
            }
            other => other.clone(),
        }
    }

    /// Increments the live-activity refcount for a named profile.
    fn retain_profile(&mut self, profile: &BrowserProfileWire) {
        if let BrowserProfileWire::Named { name } = profile {
            *self.named_refcounts.entry(name.clone()).or_insert(0) += 1;
        }
    }

    /// Decrements the refcount; drops the cached `RequestContext` at zero.
    /// The on-disk directory is left in place (persistent).
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

/// Creates a `RequestContext`. `cache_dir = Some` → disk-persistent;
/// `None` → empty `cache_path`, i.e. CEF in-memory ("incognito") mode.
fn create_request_context(cache_dir: Option<&std::path::Path>) -> Option<RequestContext> {
    // NOTE: RequestContextSettings::default() already sets `size` to
    // size_of::<_cef_request_context_settings_t>(); no manual sizing needed.
    let mut settings = RequestContextSettings::default();
    if let Some(dir) = cache_dir {
        settings.cache_path = CefString::from(dir.to_string_lossy().as_ref());
        // NOTE: persist_session_cookies = 1 so imported host-Chrome session
        // cookies survive a daemon restart.
        settings.persist_session_cookies = 1;
    }
    let ctx = request_context_create_context(Some(&settings), None);
    if ctx.is_none() {
        tracing::error!("request_context_create_context returned None");
    }
    ctx
}
