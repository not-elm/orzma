//! Library entry for the ozmux cef_host crate. Exposes internal modules so
//! tests can exercise them. The actual binary (`bin/cef_host`) and helper
//! (`bin/cef_helper`) use these via the crate path.

use cef::ImplCommandLine;

pub mod control;
pub mod cookies;
pub mod handlers;
pub mod input;
pub mod pool;
pub mod post_command;
pub mod profile;
pub mod shm_writer;

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
