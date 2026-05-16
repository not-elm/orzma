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
use crate::shm_writer::ShmWriter;
use cef::{
    Browser, BrowserSettings, CefString, ImplBrowser, ImplBrowserHost, ImplFrame, WindowInfo,
    browser_host_create_browser_sync,
};
use ozmux_browser_cef_protocol::types::ActivityId;
use ozmux_browser_cef_protocol::wire::{CefCookieDto, HostEvent, InputEvent};
use std::collections::HashMap;
use std::os::fd::RawFd;
use std::sync::Arc;
use tokio::sync::mpsc;

/// A command from the Tokio worker thread to the CEF UI thread.
#[derive(Debug)]
pub enum CefCommand {
    /// Create a new windowless browser for the given activity.
    ///
    /// `cookies` is forwarded from the wire schema; actual installation is
    /// deferred to Task B12. `shm_fd` arrives per-BrowserCreate via SCM_RIGHTS.
    BrowserCreate {
        aid: ActivityId,
        initial_url: String,
        epoch: u32,
        shm_fd: RawFd,
        cookies: Vec<CefCookieDto>,
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
}

/// The total size in bytes of the mmap region for PoC (1280×800 BGRA + slack).
const POC_SLOT_PAYLOAD_MAX: usize = 1280 * 800 * 4 + 4096;

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
}

impl BrowserPool {
    /// Creates an empty pool.
    ///
    /// `event_tx` is an unbounded sender into the cef_host event channel;
    /// it is cloned into each `NavInner` so display and load handlers can
    /// emit `HostEvent::NavStateChanged` to the daemon.
    pub fn new(event_tx: mpsc::UnboundedSender<HostEvent>) -> Self {
        Self {
            browsers: HashMap::new(),
            event_tx,
            shutdown_requested: false,
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
            } => {
                if !cookies.is_empty() {
                    tracing::info!(
                        ?aid,
                        cookie_count = cookies.len(),
                        "BrowserCreate (Task B12 installs cookies; ignored here)"
                    );
                }
                self.create_browser(aid, initial_url, epoch, shm_fd);
            }
            CefCommand::Resize {
                aid,
                css_w,
                css_h,
                dpr,
            } => {
                // NOTE: full resize (recreate browser) is Plan 2; for now just log.
                tracing::debug!(?aid, css_w, css_h, dpr, "Resize (PoC stub)");
            }
            CefCommand::Close { aid } => {
                tracing::info!(?aid, "Close");
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
                        tracing::debug!(?aid, "NavigateHistory back");
                        entry.browser.go_back();
                    }
                    1 => {
                        tracing::debug!(?aid, "NavigateHistory forward");
                        entry.browser.go_forward();
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
    fn create_browser(&mut self, aid: ActivityId, initial_url: String, epoch: u32, shm_fd: RawFd) {
        tracing::info!(?aid, %initial_url, epoch, shm_fd, "BrowserCreate");

        let total_size = ShmWriter::required_region_size(POC_SLOT_PAYLOAD_MAX);
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
        let shm = Arc::new(unsafe { ShmWriter::from_mmap(ptr as *mut u8, POC_SLOT_PAYLOAD_MAX) });

        let state = Arc::new(RenderHandlerState::new(1280, 800, 1.0));
        let render_handler = OzmuxRenderHandler::new(aid.clone(), shm.clone(), state);
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
            None,
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
                self.browsers.insert(
                    aid.clone(),
                    BrowserEntry {
                        aid,
                        epoch,
                        shm_fd,
                        browser: b,
                        shm,
                    },
                );
            }
            None => {
                tracing::error!(?aid, "browser_host_create_browser_sync returned None");
                // SAFETY: mmap was established above; munmap is safe here since we own ptr.
                unsafe { libc::munmap(ptr, total_size) };
            }
        }
    }

    /// Returns the number of active browsers.
    pub fn browser_count(&self) -> usize {
        self.browsers.len()
    }
}
