//! The chrome page, embedded at compile time from `assets/`.

use anyhow::{Context, anyhow};
use rust_embed::RustEmbed;

/// The file the web build writes the chrome page to.
const CHROME_PAGE: &str = "chrome.html";

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
    let page = Assets::get(CHROME_PAGE).ok_or_else(|| {
        anyhow!("the chrome page is missing from this build; run `just orzbrowser-web` and rebuild orzbrowser")
    })?;
    String::from_utf8(page.data.into_owned()).context("the chrome page is not UTF-8")
}
