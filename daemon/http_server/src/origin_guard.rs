//! Shared Origin-header allowlist for activity WebSocket and HTTP routes.
//!
//! ozmux is a local single-user tool: the only legitimate callers are the
//! daemon's own loopback origin and the Vite dev server, both served from
//! `localhost` / `127.0.0.1`. The dev server's port drifts (5173 → 5174 → …
//! when an earlier instance still holds the port), so the guard accepts any
//! port on a loopback host rather than a fixed list. A page served from a
//! remote site still carries that site's origin and is rejected — the
//! loopback-host check is the actual security boundary.

/// Returns true if `origin` (as taken from the `Origin` request header)
/// is a loopback HTTP origin (`http://localhost:<port>` or
/// `http://127.0.0.1:<port>`) and thus allowed to upgrade or fetch.
pub(crate) fn is_allowed_origin(origin: &str) -> bool {
    let Some(authority) = origin.strip_prefix("http://") else {
        return false;
    };
    let Some((host, port)) = authority.rsplit_once(':') else {
        return false;
    };
    if host != "localhost" && host != "127.0.0.1" {
        return false;
    }
    !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::is_allowed_origin;

    #[test]
    fn loopback_3200_allowed() {
        assert!(is_allowed_origin("http://127.0.0.1:3200"));
    }

    #[test]
    fn localhost_5173_allowed() {
        assert!(is_allowed_origin("http://localhost:5173"));
    }

    #[test]
    fn loopback_any_port_allowed() {
        // Vite drifts to 5174+ when 5173 is taken; any loopback port is fine.
        assert!(is_allowed_origin("http://localhost:5174"));
        assert!(is_allowed_origin("http://127.0.0.1:61234"));
    }

    #[test]
    fn arbitrary_origin_denied() {
        assert!(!is_allowed_origin("http://evil.example.com"));
        assert!(!is_allowed_origin("http://evil.example.com:5173"));
    }

    #[test]
    fn https_loopback_denied() {
        // The dev server is plain http; an https origin is not expected.
        assert!(!is_allowed_origin("https://localhost:5173"));
    }

    #[test]
    fn trailing_slash_denied() {
        // The port segment must be purely numeric.
        assert!(!is_allowed_origin("http://127.0.0.1:3200/"));
    }

    #[test]
    fn missing_port_denied() {
        assert!(!is_allowed_origin("http://localhost"));
    }
}
