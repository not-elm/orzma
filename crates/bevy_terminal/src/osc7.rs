//! OSC 7 (`file://host/path`) parsing and validation into a CWD `PathBuf`.

use percent_encoding::percent_decode;
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;

/// Parses an OSC 7 payload (`file://<host>/<path>`) into an absolute working
/// directory, or `None` when the scheme is not `file:`, the host is neither
/// empty/`localhost`/`local_host`, the path is not absolute, or it contains a
/// NUL. `local_host` is the machine hostname (OSC 7 emitters send the real
/// host). The path component is percent-decoded byte-wise to preserve
/// non-UTF-8 paths.
pub(crate) fn parse_osc7(payload: &[u8], local_host: &str) -> Option<PathBuf> {
    let rest = payload.strip_prefix(b"file://")?;
    let slash = rest.iter().position(|&b| b == b'/')?;
    let host = std::str::from_utf8(&rest[..slash]).ok()?;
    if !(host.is_empty() || host == "localhost" || host.eq_ignore_ascii_case(local_host)) {
        return None;
    }
    let decoded = percent_decode(&rest[slash..]).collect::<Vec<u8>>();
    if decoded.first() != Some(&b'/') || decoded.contains(&0) {
        return None;
    }
    Some(PathBuf::from(OsString::from_vec(decoded)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn accepts_localhost_and_empty_and_local_host() {
        assert_eq!(parse_osc7(b"file://localhost/Users/me/p", "myhost"), Some(PathBuf::from("/Users/me/p")));
        assert_eq!(parse_osc7(b"file:///Users/me/p", "myhost"), Some(PathBuf::from("/Users/me/p")));
        assert_eq!(parse_osc7(b"file://myhost/tmp", "myhost"), Some(PathBuf::from("/tmp")));
    }

    #[test]
    fn percent_decodes_uppercase_and_keeps_semicolons() {
        assert_eq!(parse_osc7(b"file:///tmp/a%20b", "h"), Some(PathBuf::from("/tmp/a b")));
        assert_eq!(parse_osc7(b"file:///tmp/a;b", "h"), Some(PathBuf::from("/tmp/a;b")));
    }

    #[test]
    fn rejects_remote_host_relative_scheme_and_nul() {
        assert_eq!(parse_osc7(b"file://other/tmp", "myhost"), None);
        assert_eq!(parse_osc7(b"http://localhost/tmp", "h"), None);
        assert_eq!(parse_osc7(b"file://localhost", "h"), None);
        assert_eq!(parse_osc7(b"file:///tmp/%00x", "h"), None);
    }
}
