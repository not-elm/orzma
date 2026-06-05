//! OSC 7 (`file://host/path`) parsing and validation into a CWD `PathBuf`.

use crate::vt::listener::ControlFrame;
use crossbeam_channel::Sender;
use percent_encoding::percent_decode;
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;
use vte::Perform;

/// Parses an OSC 7 payload (`file://<host>/<path>`) into an absolute working
/// directory, or `None` when the scheme is not `file:`, the host is neither
/// empty/`localhost`/`local_host`, the path is not absolute, or it contains a
/// NUL. `local_host` is the machine hostname (OSC 7 emitters send the real
/// host). The path component is percent-decoded byte-wise to preserve
/// non-UTF-8 paths.
pub fn parse_osc7(payload: &[u8], local_host: &str) -> Option<PathBuf> {
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

/// A `vte::Perform` that watches a second, independent parser for OSC 7 and
/// forwards changed directories onto the terminal's `ControlFrame` channel.
/// Dedups: a shell re-emits OSC 7 on every prompt, so only a *changed* path is
/// sent.
pub struct Osc7Capture {
    control_tx: Sender<ControlFrame>,
    local_host: String,
    last: Option<PathBuf>,
}

impl Osc7Capture {
    /// Builds a capture that sends `ControlFrame::CurrentDir` on `control_tx`,
    /// accepting OSC 7 paths whose host is empty / `localhost` / `local_host`.
    pub fn new(control_tx: Sender<ControlFrame>, local_host: String) -> Self {
        Self {
            control_tx,
            local_host,
            last: None,
        }
    }
}

impl Perform for Osc7Capture {
    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        if params.first().copied() != Some(&b"7"[..]) {
            return;
        }
        // NOTE: vte splits the OSC payload on every ';'; rejoin params[1..] so
        // a path containing ';' is not truncated.
        let payload: Vec<u8> = params[1..].join(&b';');
        if let Some(path) = parse_osc7(&payload, &self.local_host)
            && self.last.as_deref() != Some(path.as_path())
        {
            self.last = Some(path.clone());
            if let Err(e) = self.control_tx.send(ControlFrame::CurrentDir(path)) {
                tracing::warn!(?e, "control_tx send(CurrentDir) failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vt::listener::ControlFrame;
    use crossbeam_channel::unbounded;
    use std::path::PathBuf;
    use vte::Perform;

    #[test]
    fn accepts_localhost_and_empty_and_local_host() {
        assert_eq!(
            parse_osc7(b"file://localhost/Users/me/p", "myhost"),
            Some(PathBuf::from("/Users/me/p"))
        );
        assert_eq!(
            parse_osc7(b"file:///Users/me/p", "myhost"),
            Some(PathBuf::from("/Users/me/p"))
        );
        assert_eq!(
            parse_osc7(b"file://myhost/tmp", "myhost"),
            Some(PathBuf::from("/tmp"))
        );
    }

    #[test]
    fn percent_decodes_uppercase_and_keeps_semicolons() {
        assert_eq!(
            parse_osc7(b"file:///tmp/a%20b", "h"),
            Some(PathBuf::from("/tmp/a b"))
        );
        assert_eq!(
            parse_osc7(b"file:///tmp/a;b", "h"),
            Some(PathBuf::from("/tmp/a;b"))
        );
    }

    #[test]
    fn rejects_remote_host_relative_scheme_and_nul() {
        assert_eq!(parse_osc7(b"file://other/tmp", "myhost"), None);
        assert_eq!(parse_osc7(b"http://localhost/tmp", "h"), None);
        assert_eq!(parse_osc7(b"file://localhost", "h"), None);
        assert_eq!(parse_osc7(b"file:///tmp/%00x", "h"), None);
    }

    #[test]
    fn capture_sends_changed_dir_once_and_dedups() {
        let (tx, rx) = unbounded::<ControlFrame>();
        let mut cap = Osc7Capture::new(tx, "myhost".into());

        cap.osc_dispatch(&[b"7", b"file://localhost/tmp"], true);
        assert_eq!(
            rx.try_recv(),
            Ok(ControlFrame::CurrentDir(PathBuf::from("/tmp")))
        );

        cap.osc_dispatch(&[b"7", b"file://localhost/tmp"], true);
        assert!(rx.try_recv().is_err());

        cap.osc_dispatch(&[b"7", b"file://localhost/var"], true);
        assert_eq!(
            rx.try_recv(),
            Ok(ControlFrame::CurrentDir(PathBuf::from("/var")))
        );
    }

    #[test]
    fn capture_rejoins_semicolon_paths_and_ignores_non_osc7() {
        let (tx, rx) = unbounded::<ControlFrame>();
        let mut cap = Osc7Capture::new(tx, "h".into());
        cap.osc_dispatch(&[b"7", b"file://localhost/tmp/a", b"b"], true);
        assert_eq!(
            rx.try_recv(),
            Ok(ControlFrame::CurrentDir(PathBuf::from("/tmp/a;b")))
        );
        cap.osc_dispatch(&[b"0", b"title"], true);
        assert!(rx.try_recv().is_err());
    }
}
