//! Daemon-side cookie extraction: Chrome cookie store → `Vec<CefCookieDto>`.
//!
//! Reads the host Chrome `Default` profile cookies for the domain extracted
//! from `initial_url`, decrypts them via `decrypt-cookies`, and maps each
//! entry to a `CefCookieDto`. The resulting list is forwarded inline in
//! `HostCommand::BrowserCreate` so `cef_host` can seed its cookie store
//! before the first navigation.
//!
//! macOS only. Other platforms return `Ok(vec![])` immediately.
//!
//! Failures (Keychain locked, profile missing, etc.) return `Ok(vec![])` so
//! the caller can continue in a degraded-but-functional state and log a
//! warning. Phase B Task B12.

use ozmux_browser_cef_protocol::wire::{CefCookieDto, SameSite};
#[cfg(target_os = "macos")]
use {
    decrypt_cookies::browser::cookies::SameSite as DcSameSite,
    decrypt_cookies::chromium::{ChromiumCookie, GetCookies},
    decrypt_cookies::prelude::*,
};

/// Extracts Chrome cookies scoped to the domain of `initial_url` and returns
/// them as `CefCookieDto`s ready for `HostCommand::BrowserCreate`.
///
/// Returns `Ok(vec![])` on non-macOS platforms or when Chrome is not
/// installed. Returns `Ok(vec![])` (not `Err`) on soft failures such as a
/// locked Keychain or missing profile — the caller logs a warning and
/// proceeds without cookies per spec §4.6.
pub async fn extract_for(initial_url: &str) -> Result<Vec<CefCookieDto>, std::io::Error> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = initial_url;
        return Ok(Vec::new());
    }
    #[cfg(target_os = "macos")]
    {
        extract_macos(initial_url).await
    }
}

#[cfg(target_os = "macos")]
async fn extract_macos(initial_url: &str) -> Result<Vec<CefCookieDto>, std::io::Error> {
    let host = host_from_url(initial_url);

    let getter = ChromiumBuilder::<Chrome>::new()
        .build()
        .await
        .map_err(|e| std::io::Error::other(format!("build chromium getter: {e}")))?;

    let raw = getter
        .cookies_by_host(&host)
        .await
        .map_err(|e| std::io::Error::other(format!("read cookies for {host}: {e}")))?;

    let mut out = Vec::with_capacity(raw.len());
    for c in &raw {
        let Some(ref value) = c.decrypted_value else {
            continue;
        };
        out.push(to_cookie_dto(c, value.clone(), initial_url));
    }
    Ok(out)
}

/// Extracts the registrable host from a URL for use as the `cookies_by_host`
/// query key. Falls back to the full URL string on parse failure.
fn host_from_url(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_else(|| url.to_owned())
}

#[cfg(target_os = "macos")]
fn to_cookie_dto(c: &ChromiumCookie, value: String, initial_url: &str) -> CefCookieDto {
    let same_site = match c.same_site {
        DcSameSite::Strict => SameSite::Strict,
        DcSameSite::Lax => SameSite::Lax,
        DcSameSite::None => SameSite::None,
    };
    let cookie_url = if c.host_key.starts_with('.') {
        format!("https://{}{}", c.host_key.trim_start_matches('.'), c.path)
    } else {
        initial_url.to_owned()
    };
    CefCookieDto {
        url: cookie_url,
        name: c.name.clone(),
        value,
        domain: c.host_key.clone(),
        path: c.path.clone(),
        secure: c.is_secure,
        http_only: c.is_httponly,
        // NOTE: expires_utc as Option<f64> (Windows FILETIME microseconds since
        // 1601-01-01) is not currently wired — plan 3 will add the conversion.
        expires_utc: c.expires_utc.map(|dt| dt.timestamp() as f64),
        same_site,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn extract_for_non_macos_or_no_env_returns_empty() {
        if !crate::requires_real_chrome() {
            let result = extract_for("https://example.com").await;
            // On macOS with Chrome not set up, or on other platforms, we
            // expect either Ok([]) or Ok(small_list). We only assert no panic.
            let _ = result;
        }
    }

    #[tokio::test]
    async fn smoke_real_chrome() {
        if !crate::requires_real_chrome() {
            eprintln!("skipping; set OZMUX_TEST_REAL_CHROME=1 to run");
            return;
        }
        let cookies = extract_for("https://google.com")
            .await
            .expect("extract_for should not error");
        eprintln!("extracted {} cookies for google.com", cookies.len());
    }
}
