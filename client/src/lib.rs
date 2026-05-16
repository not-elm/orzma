//! Tauri launcher entry point. Probes for an existing ozmux daemon, spawns
//! one as a sidecar if needed, waits for `/health`, and then builds the
//! webview window pointing at the daemon's HTTP UI.

use tauri::{WebviewUrl, WebviewWindowBuilder};

mod daemon;

/// Runs the Tauri application. Called from `main.rs`.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .setup(|app| {
            tauri::async_runtime::block_on(async {
                daemon::ensure_running(app.handle()).await?;
                let url = WebviewUrl::External("http://127.0.0.1:3200".parse()?);
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
