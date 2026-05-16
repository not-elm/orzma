//! `BrowserPool` — owns CEF browser instances on the CEF UI thread (main).
//!
//! `BrowserPool` is `!Send` because it holds raw CEF objects. The Tokio worker
//! thread enqueues `CefCommand`s via the shared queue; the main thread drains
//! and executes them between `do_message_loop_work()` calls.
//!
//! PoC scope: stub `execute` for BrowserCreate (just stores the activity in a
//! map). Real CefBrowserHost::CreateBrowserSync is wired in Task 18.

use ozmux_browser_cef_protocol::types::ActivityId;
use std::collections::HashMap;
use std::os::fd::RawFd;

#[derive(Debug)]
pub enum CefCommand {
    BrowserCreate {
        aid: ActivityId,
        initial_url: String,
        epoch: u32,
        shm_fd: RawFd,
    },
    Resize {
        aid: ActivityId,
        css_w: u32,
        css_h: u32,
        dpr: f32,
    },
    Close {
        aid: ActivityId,
    },
    Shutdown,
}

pub struct BrowserEntry {
    pub aid: ActivityId,
    pub epoch: u32,
    pub shm_fd: RawFd,
    // NOTE: `browser: cef::Browser` field is added in Task 18 once render
    // handler + CreateBrowserSync are wired.
}

pub struct BrowserPool {
    browsers: HashMap<ActivityId, BrowserEntry>,
    pub shutdown_requested: bool,
}

impl BrowserPool {
    pub fn new() -> Self {
        Self {
            browsers: HashMap::new(),
            shutdown_requested: false,
        }
    }

    pub fn execute(&mut self, cmd: CefCommand) {
        tracing::debug!(?cmd, "execute");
        match cmd {
            CefCommand::BrowserCreate {
                aid,
                initial_url,
                epoch,
                shm_fd,
            } => {
                tracing::info!(
                    ?aid,
                    ?initial_url,
                    epoch,
                    shm_fd,
                    "BrowserCreate (PoC stub)"
                );
                self.browsers
                    .insert(aid.clone(), BrowserEntry { aid, epoch, shm_fd });
            }
            CefCommand::Resize {
                aid,
                css_w,
                css_h,
                dpr,
            } => {
                tracing::debug!(?aid, css_w, css_h, dpr, "Resize (PoC stub)");
            }
            CefCommand::Close { aid } => {
                tracing::info!(?aid, "Close");
                self.browsers.remove(&aid);
            }
            CefCommand::Shutdown => {
                tracing::info!("Shutdown requested");
                self.shutdown_requested = true;
            }
        }
    }

    pub fn browser_count(&self) -> usize {
        self.browsers.len()
    }
}

impl Default for BrowserPool {
    fn default() -> Self {
        Self::new()
    }
}
