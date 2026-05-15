//! Shared Origin-header allowlist for activity WebSocket and HTTP routes.
//!
//! All ozmux endpoints exposed to the browser accept the same origins as
//! `handlers_ws` did historically: the daemon's own loopback and the Vite
//! dev server.

const ALLOWED: &[&str] = &[
    "http://127.0.0.1:3200",
    "http://localhost:3200",
    "http://127.0.0.1:5173",
    "http://localhost:5173",
];

/// Returns true if `origin` (as taken from the `Origin` request header)
/// is allowed to upgrade or fetch.
pub(crate) fn is_allowed_origin(origin: &str) -> bool {
    ALLOWED.contains(&origin)
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
    fn arbitrary_origin_denied() {
        assert!(!is_allowed_origin("http://evil.example.com"));
    }

    #[test]
    fn trailing_slash_denied() {
        // Match is exact; trailing slash differs.
        assert!(!is_allowed_origin("http://127.0.0.1:3200/"));
    }
}
