//! Library entry for the ozmux cef_host crate. Exposes the CEF host machinery
//! (CEF settings, app implementation, browser pool, handlers) consumed by the
//! `ozmux-daemon` binary and the `cef_helper` subprocess.
//!
//! # Code discipline
//!
//! CEF refcount-bearing types (`Browser`, `RequestContext`, `OzmuxRenderHandler`,
//! `PoolHandle`, etc.) must NEVER be captured into a `tokio::spawn`ed future.
//! Their `Drop` may release CEF objects, which must happen on the CEF UI thread.
//! Use `post_command::post` to schedule mutation (including teardown) on the UI
//! thread instead.

use cef::ImplCommandLine;

pub mod browser_app;
pub mod cef_settings;
pub mod cookies;
pub mod frame_buffer_pool;
pub mod handlers;
pub mod input;
pub mod pool;
pub mod post_command;
pub mod profile;
pub mod scheme;

pub use browser_app::BrowserApp;
pub use cef_settings::{acquire_data_root, build_cef_settings, load_cef_framework};
#[cfg(target_os = "macos")]
pub use cef_settings::{in_bundle_framework_dylib, in_bundle_helper};
pub use frame_buffer_pool::FrameBufferPool;

/// Appends a flag-only switch to a CEF command line. Compresses the
/// boilerplate of constructing a `cef::CefString` per call.
pub fn append_flag(cl: &mut cef::CommandLine, name: &str) {
    let n = cef::CefString::from(name);
    cl.append_switch(Some(&n));
}

/// Appends a switch with a value to a CEF command line.
pub fn append_flag_value(cl: &mut cef::CommandLine, name: &str, value: &str) {
    let n = cef::CefString::from(name);
    let v = cef::CefString::from(value);
    cl.append_switch_with_value(Some(&n), Some(&v));
}
