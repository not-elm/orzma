//! Tauri launcher entry point. Ensures the ozmux daemon is running, then
//! builds the webview window pointing at the daemon's HTTP UI.
//!
//! Usage: `ozmux-client [URL_OR_SESSION_ID]`. With no arg, opens the
//! daemon's root page; with a `http(s)://...` arg, opens that URL
//! directly; otherwise treats the arg as a session id and opens
//! `<daemon_base>/?session=<arg>`.

use tauri::{Url, WebviewUrl, WebviewWindowBuilder};

mod daemon;

/// Runs the Tauri application. Called from `main.rs`.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let arg = std::env::args().nth(1);
    tauri::Builder::default()
        .setup(move |app| {
            tauri::async_runtime::block_on(async {
                daemon::ensure_running().await?;
                let url = resolve_initial_url(arg.as_deref(), daemon::DAEMON_BASE_URL)?;
                WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url))
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

/// Resolve the initial Webview URL given the CLI positional arg and the
/// daemon's base URL. See module docs for the priority.
fn resolve_initial_url(arg: Option<&str>, base: &str) -> anyhow::Result<Url> {
    let base_url = Url::parse(base)?;
    match arg {
        None => Ok(base_url),
        Some(s) if s.starts_with("http://") || s.starts_with("https://") => match Url::parse(s) {
            Ok(u) => Ok(u),
            Err(e) => {
                eprintln!("ozmux-client: failed to parse '{s}' as URL: {e}; using {base}");
                Ok(base_url)
            }
        },
        Some(s) => {
            let mut u = base_url.clone();
            u.set_query(Some(&format!(
                "session={}",
                percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC)
            )));
            Ok(u)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "http://127.0.0.1:3200";

    #[test]
    fn none_arg_returns_base() {
        let u = resolve_initial_url(None, BASE).unwrap();
        assert_eq!(u.as_str(), "http://127.0.0.1:3200/");
    }

    #[test]
    fn http_url_arg_used_directly() {
        let u = resolve_initial_url(Some("http://127.0.0.1:3200/?session=abc"), BASE).unwrap();
        assert_eq!(u.query(), Some("session=abc"));
    }

    #[test]
    fn bare_id_appended_as_query() {
        let u = resolve_initial_url(Some("abc-123"), BASE).unwrap();
        assert_eq!(u.query(), Some("session=abc%2D123"));
    }

    #[test]
    fn malformed_http_arg_falls_back_to_base() {
        let u = resolve_initial_url(Some("http://[bad"), BASE).unwrap();
        assert!(u.query().is_none() || u.query() == Some(""));
        assert!(u.as_str().starts_with("http://127.0.0.1:3200"));
    }
}
