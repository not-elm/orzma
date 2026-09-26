//! The CEF render process orzma ships beside its executable, built against the
//! same CEF release orzma links.
#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

use bevy_cef_core::prelude::execute_render_process;

fn main() {
    execute_render_process();
}
