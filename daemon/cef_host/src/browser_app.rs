//! The `cef::App` implementation passed to `cef::initialize`. Injects
//! `--use-mock-keychain` into every CEF process and the browser-process flags
//! (`--no-sandbox`, `--disable-gpu`, optional Site Isolation disable) needed
//! by the ozmux out-of-process browser.

use cef::rc::Rc as _;
use cef::{App, ImplApp, WrapApp, wrap_app};

use crate::{append_flag, append_flag_value};

wrap_app! {
    pub struct BrowserApp;

    impl App {
        fn on_before_command_line_processing(
            &self,
            process_type: Option<&cef::CefString>,
            command_line: Option<&mut cef::CommandLine>,
        ) {
            let is_browser = process_type
                .map(|s| s.to_string().is_empty())
                .unwrap_or(true);
            let Some(cl) = command_line else {
                return;
            };

            // NOTE: --use-mock-keychain must reach EVERY process. CEF does not always
            // propagate it from the browser command line to helpers, so the Network Service
            // utility (which performs cookie encryption) ends up invoking the real macOS
            // Keychain and raises a "Chromium Safe Storage" authorization dialog. Inject it
            // unconditionally to keep cookie crypto fully in-memory.
            append_flag(cl, "use-mock-keychain");

            if is_browser {
                append_flag(cl, "no-sandbox");
                append_flag(cl, "disable-gpu");

                if std::env::var("OZMUX_BROWSER_SITE_ISOLATION").as_deref() != Ok("1") {
                    append_flag_value(cl, "disable-features", "IsolateOrigins,site-per-process");
                    append_flag(cl, "disable-site-isolation-trials");
                } else {
                    tracing::info!(
                        "OZMUX_BROWSER_SITE_ISOLATION=1 — Site Isolation left enabled"
                    );
                }
            }
        }
    }
}
