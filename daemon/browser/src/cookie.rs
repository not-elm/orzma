//! One-shot snapshot import of the local Chrome cookie store, used at the
//! moment the shared Chromium process starts. Wraps the `decrypt-cookies`
//! crate so the rest of the daemon never sees its types directly.
//!
//! Scope is intentionally narrow:
//! - macOS only (other platforms return `Ok(Vec::new())`)
//! - Chrome's `Default` profile
//! - Partitioned cookies and write-back are out of scope
//! - Failures are returned as `Err(BrowserError::Cookie)` so the caller can
//!   log and continue without cookies.

use crate::error::{BrowserError, BrowserResult};
use chromiumoxide::cdp::browser_protocol::network::CookieParam;

/// Read the local Chrome `Default` profile cookies (macOS only), decrypt
/// each value, and return CDP `CookieParam`s ready to feed into
/// `Network.setCookies` before the first navigation.
///
/// Returns `Ok(vec![])` on non-macOS platforms or when Chrome is not
/// installed; returns `Err(BrowserError::Cookie)` on read/decrypt failures
/// so the caller can decide whether to proceed without cookies (the spec
/// calls for a non-fatal warning).
#[expect(dead_code, reason = "called by BrowserService::spawn in a later task")]
pub(crate) async fn import_chrome_default_cookies() -> BrowserResult<Vec<CookieParam>> {
    #[cfg(not(target_os = "macos"))]
    {
        Ok(Vec::new())
    }
    #[cfg(target_os = "macos")]
    {
        import_macos_chrome_default().await
    }
}

#[cfg(target_os = "macos")]
async fn import_macos_chrome_default() -> BrowserResult<Vec<CookieParam>> {
    use decrypt_cookies::chromium::GetCookies;
    use decrypt_cookies::prelude::*;

    let getter = ChromiumBuilder::<Chrome>::new()
        .build()
        .await
        .map_err(|e| BrowserError::Cookie(format!("build chromium getter: {e}")))?;
    let raw = getter
        .cookies_all()
        .await
        .map_err(|e| BrowserError::Cookie(format!("read cookies: {e}")))?;

    let mut out = Vec::with_capacity(raw.len());
    for c in &raw {
        let Some(ref value) = c.decrypted_value else {
            continue;
        };
        if let Some(param) = to_cookie_param(c, value.clone()) {
            out.push(param);
        }
    }
    Ok(out)
}

/// Convert one `decrypt-cookies` raw cookie into a CDP `CookieParam`.
///
/// Returns `None` if the CDP builder rejects the cookie (e.g. missing
/// required fields).
#[cfg(target_os = "macos")]
fn to_cookie_param(
    c: &decrypt_cookies::chromium::ChromiumCookie,
    value: String,
) -> Option<CookieParam> {
    use chromiumoxide::cdp::browser_protocol::network::{CookieSameSite, TimeSinceEpoch};
    use decrypt_cookies::browser::cookies::SameSite;

    let same_site = match c.same_site {
        SameSite::Strict => Some(CookieSameSite::Strict),
        SameSite::Lax => Some(CookieSameSite::Lax),
        SameSite::None => Some(CookieSameSite::None),
    };

    let expires = c
        .expires_utc
        .map(|dt| TimeSinceEpoch::new(dt.timestamp() as f64));

    let mut builder = CookieParam::builder()
        .name(c.name.clone())
        .value(value)
        .domain(c.host_key.clone())
        .path(c.path.clone())
        .secure(c.is_secure)
        .http_only(c.is_httponly);

    if let Some(ss) = same_site {
        builder = builder.same_site(ss);
    }
    if let Some(exp) = expires {
        builder = builder.expires(exp);
    }

    builder.build().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs only when `OZMUX_TEST_REAL_CHROME=1` is set. Verifies decryption
    /// against the real local Chrome on the developer's machine. Will trigger
    /// a macOS Keychain prompt on first run on a given machine; subsequent
    /// runs are silent.
    #[tokio::test]
    async fn import_smoke() {
        if std::env::var("OZMUX_TEST_REAL_CHROME").ok().as_deref() != Some("1") {
            eprintln!("skipping; set OZMUX_TEST_REAL_CHROME=1 to run");
            return;
        }
        let cookies = import_chrome_default_cookies().await.expect("import");
        eprintln!("imported {} cookies", cookies.len());
        // The user must be logged into something for this to assert > 0;
        // we only assert the call completes without error.
    }
}
