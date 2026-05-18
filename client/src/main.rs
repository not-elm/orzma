//! ozmux-client — Tauri launcher for ozmux's daemon UI.
//!
//! Usage: `ozmux-client [URL_OR_SESSION_ID]`. With no arg, opens the
//! daemon's root page; with a `http(s)://...` arg, opens that URL
//! directly; otherwise treats the arg as a session id and opens
//! `<daemon_base>/?session=<arg>`.

// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    ozmux_client_lib::run()
}
