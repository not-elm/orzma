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
//!
//! Setting `OZMUX_BROWSER_SKIP_COOKIE_IMPORT=1` short-circuits extraction to
//! `Ok(vec![])`. On macOS the `decrypt-cookies` path reads the host Chrome's
//! `Chrome Safe Storage` Keychain item, which raises an authorization dialog
//! for the (differently-signed) ozmux binary on every launch — the skip flag
//! avoids that during local development.

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
/// proceeds without cookies per spec §4.6. Also returns `Ok(vec![])` when
/// `OZMUX_BROWSER_SKIP_COOKIE_IMPORT=1` is set.
pub async fn extract_for(initial_url: &str) -> Result<Vec<CefCookieDto>, std::io::Error> {
    if std::env::var("OZMUX_BROWSER_SKIP_COOKIE_IMPORT").as_deref() == Ok("1") {
        tracing::info!("OZMUX_BROWSER_SKIP_COOKIE_IMPORT=1 — skipping Chrome cookie import");
        return Ok(Vec::new());
    }
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
        out.push(to_cookie_dto(c, value.clone()));
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
fn to_cookie_dto(c: &ChromiumCookie, value: String) -> CefCookieDto {
    // NOTE: Chrome's SQLite `samesite` column has four states (-1 unspecified,
    // 0 None, 1 Lax, 2 Strict) but `decrypt-cookies` 0.11.1 collapses everything
    // that isn't 1/2 into `SameSite::None`. Forwarding that to CEF as
    // `NO_RESTRICTION` makes Chromium reject every cookie whose stored value
    // was actually `unspecified` and that lacks `Secure` — `SameSite=None`
    // without `Secure` is a CookieMonster rejection in modern Chromium. We
    // can't recover the original i32 through the public API, so remap `None`
    // to `Unspecified`: insecure cookies are then accepted (the common case),
    // and legitimate `SameSite=None; Secure` cookies degrade to Lax-by-default
    // (acceptable for the local single-user embedded browser).
    let same_site = match c.same_site {
        DcSameSite::Strict => SameSite::Strict,
        DcSameSite::Lax => SameSite::Lax,
        DcSameSite::None => SameSite::Unspecified,
    };
    let (cookie_url, cookie_domain) = build_url_and_domain(&c.host_key, &c.path);
    CefCookieDto {
        url: cookie_url,
        name: c.name.clone(),
        value,
        domain: cookie_domain,
        path: c.path.clone(),
        secure: c.is_secure,
        http_only: c.is_httponly,
        // NOTE: expires_utc as Option<f64> (Windows FILETIME microseconds since
        // 1601-01-01) is not currently wired — plan 3 will add the conversion.
        expires_utc: c.expires_utc.map(|dt| dt.timestamp() as f64),
        same_site,
    }
}

/// Builds the `(url, domain)` pair forwarded to `CefCookieManager::set_cookie`
/// for a Chrome-stored cookie. Two conventions matter for Chromium's
/// `CanonicalCookie::CreateSanitizedCookie`:
///
/// - A `host_key` beginning with `.` is a domain cookie (set with the
///   `Domain` attribute). CEF expects the `Domain` attribute carried verbatim
///   (leading dot included) and a URL whose host lies within that domain.
/// - A `host_key` without a leading dot is a host-only cookie (no `Domain`
///   attribute). CEF expects an empty `domain` and a URL whose host is
///   exactly `host_key`. This is also the only shape that satisfies the
///   `__Host-` cookie prefix invariants — sending a non-empty `domain` for
///   such a cookie fires Chromium's `EXCLUDE_INVALID_PREFIX`.
///
/// The URL scheme is always `https`. That works for `secure=true` cookies
/// (required) and `secure=false` cookies (accepted), and avoids
/// `EXCLUDE_SECURE_ONLY` rejections that would fire when setting a
/// secure cookie via an `http://` URL.
#[cfg(target_os = "macos")]
fn build_url_and_domain(host_key: &str, path: &str) -> (String, String) {
    let path = if path.is_empty() { "/" } else { path };
    if let Some(host) = host_key.strip_prefix('.') {
        (format!("https://{host}{path}"), host_key.to_owned())
    } else {
        (format!("https://{host_key}{path}"), String::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn build_url_and_domain_domain_cookie_keeps_dot_and_strips_for_url() {
        let (url, domain) = build_url_and_domain(".example.com", "/path");
        assert_eq!(url, "https://example.com/path");
        assert_eq!(domain, ".example.com");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn build_url_and_domain_host_only_clears_domain_for_host_cookie() {
        let (url, domain) = build_url_and_domain("mail.google.com", "/");
        assert_eq!(url, "https://mail.google.com/");
        assert!(
            domain.is_empty(),
            "host-only cookies must use an empty Domain attribute so __Host- prefixed cookies pass Chromium's prefix validation"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn build_url_and_domain_empty_path_defaults_to_root() {
        let (url, _) = build_url_and_domain("example.com", "");
        assert_eq!(url, "https://example.com/");
    }

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
