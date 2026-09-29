//! The web assets, embedded at compile time from `assets/`.

use anyhow::{Context, anyhow};
use rust_embed::RustEmbed;

/// The file the web build writes the chrome page to.
const CHROME_PAGE: &str = "chrome.html";

/// The file the web build writes the page's preload script to.
const PAGE_SCRIPT: &str = "page.js";

/// The built web bundle under `assets/`.
#[derive(RustEmbed)]
#[folder = "assets/"]
struct Assets;

/// Returns the chrome page's HTML.
///
/// # Errors
/// Fails when this binary was built before the chrome page was (run `just
/// orzbrowser-web`), or when the page is not UTF-8.
pub(crate) fn chrome_html() -> anyhow::Result<String> {
    text_asset(CHROME_PAGE, "the chrome page")
}

/// Returns the page's preload script, which handles the scroll keys.
///
/// # Errors
/// Fails when this binary was built before the script was (run `just
/// orzbrowser-web`), or when the script is not UTF-8.
pub(crate) fn page_js() -> anyhow::Result<String> {
    text_asset(PAGE_SCRIPT, "the page script")
}

fn text_asset(file: &str, what: &str) -> anyhow::Result<String> {
    let asset = Assets::get(file).ok_or_else(|| {
        anyhow!(
            "{what} is missing from this build; run `just orzbrowser-web` and rebuild orzbrowser"
        )
    })?;
    String::from_utf8(asset.data.into_owned()).with_context(|| format!("{what} is not UTF-8"))
}
