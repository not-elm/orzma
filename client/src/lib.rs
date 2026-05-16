//! Tauri launcher entry point. Ensures the ozmux daemon is running, then
//! builds the webview window pointing at the daemon's HTTP UI.

use tauri::{WebviewUrl, WebviewWindowBuilder};

mod daemon;

/// Runs the Tauri application. Called from `main.rs`.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            tauri::async_runtime::block_on(async {
                daemon::ensure_running().await?;
                let url = WebviewUrl::External(daemon::DAEMON_BASE_URL.parse()?);
                WebviewWindowBuilder::new(app, "main", url)
                    .title("ozmux")
                    .inner_size(1280.0, 800.0)
                    .build()?;
                anyhow::Ok(())
            })?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
