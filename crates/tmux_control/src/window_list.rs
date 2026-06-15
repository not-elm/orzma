//! Sans-IO parsing of `tmux list-windows -a` output into typed [`WindowEntry`].

use crate::error::{TmuxError, TmuxResult};
use std::str;
use tmux_control_parser::{SessionId, WindowId};

/// One window row from `list-windows -a`, carrying its owning session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowEntry {
    /// Owning tmux session id (`$N`).
    pub session_id: SessionId,
    /// Owning session name.
    pub session_name: String,
    /// tmux window id (`@N`).
    pub window_id: WindowId,
    /// tmux display index (#{window_index}).
    pub window_index: u32,
    /// Whether this window is the active one in its session.
    pub window_active: bool,
    /// Window name (may contain spaces; tmux escapes raw tabs).
    pub window_name: String,
}

// NOTE: tab-separated. Only `window_name`, the LAST field, is protected by field
// ordering — `splitn(6, b'\t')` keeps it intact even if a tab leaks in.
// `session_name` sits in position 2 and has no such protection: it relies solely
// on tmux escaping a raw tab in a name to the literal `\t` so the field count holds.
pub(crate) const LIST_ALL_FORMAT: &str = "#{session_id}\t#{session_name}\t#{window_id}\t#{window_index}\t#{window_active}\t#{window_name}";

impl WindowEntry {
    /// Parses the tab-separated `list-windows -a -F` output (one window per line).
    pub fn parse_list(output: &[u8]) -> TmuxResult<Vec<WindowEntry>> {
        let mut entries = Vec::new();
        for mut line in output.split(|&b| b == b'\n') {
            if let [rest @ .., b'\r'] = line {
                line = rest;
            }
            if line.is_empty() {
                continue;
            }
            entries.push(parse_line(line)?);
        }
        Ok(entries)
    }
}

fn parse_line(line: &[u8]) -> TmuxResult<WindowEntry> {
    let mut fields = line.splitn(6, |&b| b == b'\t');
    let session_id = fields
        .next()
        .and_then(parse_session_id)
        .ok_or_else(|| malformed(line))?;
    let session_name = fields
        .next()
        .and_then(|f| str::from_utf8(f).ok())
        .ok_or_else(|| malformed(line))?
        .to_string();
    let window_id = fields
        .next()
        .and_then(parse_window_id)
        .ok_or_else(|| malformed(line))?;
    let window_index = fields
        .next()
        .and_then(parse_u32)
        .ok_or_else(|| malformed(line))?;
    let window_active = fields
        .next()
        .and_then(parse_u32)
        .ok_or_else(|| malformed(line))?
        > 0;
    let window_name = fields
        .next()
        .and_then(|f| str::from_utf8(f).ok())
        .ok_or_else(|| malformed(line))?
        .to_string();
    Ok(WindowEntry {
        session_id,
        session_name,
        window_id,
        window_index,
        window_active,
        window_name,
    })
}

fn malformed(line: &[u8]) -> TmuxError {
    TmuxError::MalformedWindowList {
        line: String::from_utf8_lossy(line).into_owned(),
    }
}

fn parse_session_id(field: &[u8]) -> Option<SessionId> {
    let digits = field.strip_prefix(b"$")?;
    Some(SessionId(str::from_utf8(digits).ok()?.parse().ok()?))
}

fn parse_window_id(field: &[u8]) -> Option<WindowId> {
    let digits = field.strip_prefix(b"@")?;
    Some(WindowId(str::from_utf8(digits).ok()?.parse().ok()?))
}

fn parse_u32(field: &[u8]) -> Option<u32> {
    str::from_utf8(field).ok()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_two_windows_one_session() {
        let out = b"$0\talpha\t@0\t0\t1\tzsh\n$0\talpha\t@1\t1\t0\teditor\n";
        let got = WindowEntry::parse_list(out).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].session_id, SessionId(0));
        assert_eq!(got[0].session_name, "alpha");
        assert_eq!(got[0].window_id, WindowId(0));
        assert_eq!(got[0].window_index, 0);
        assert!(got[0].window_active);
        assert_eq!(got[1].window_id, WindowId(1));
        assert_eq!(got[1].window_index, 1);
        assert!(!got[1].window_active);
        assert_eq!(got[1].window_name, "editor");
    }

    #[test]
    fn name_with_spaces_kept() {
        let out = b"$2\tmy work\t@5\t0\t1\tmy window\n";
        let got = WindowEntry::parse_list(out).unwrap();
        assert_eq!(got[0].session_name, "my work");
        assert_eq!(got[0].window_name, "my window");
    }

    #[test]
    fn crlf_and_blank_lines_tolerated() {
        let out = b"\n$0\ta\t@0\t0\t1\tw\r\n\n";
        assert_eq!(WindowEntry::parse_list(out).unwrap().len(), 1);
    }

    #[test]
    fn bad_window_id_errors() {
        let out = b"$0\ta\t0\t0\t1\tw\n";
        assert!(matches!(
            WindowEntry::parse_list(out),
            Err(TmuxError::MalformedWindowList { .. })
        ));
    }

    #[test]
    fn bad_session_id_errors() {
        let no_dollar = b"x\ta\t@0\t0\t1\tw\n";
        let non_numeric = b"$x\ta\t@0\t0\t1\tw\n";
        assert!(matches!(
            WindowEntry::parse_list(no_dollar),
            Err(TmuxError::MalformedWindowList { .. })
        ));
        assert!(matches!(
            WindowEntry::parse_list(non_numeric),
            Err(TmuxError::MalformedWindowList { .. })
        ));
    }

    #[test]
    fn trailing_newline_optional() {
        let with = WindowEntry::parse_list(b"$0\ta\t@0\t0\t1\tw\n").unwrap();
        let without = WindowEntry::parse_list(b"$0\ta\t@0\t0\t1\tw").unwrap();
        assert_eq!(with, without);
    }

    #[test]
    fn empty_is_empty() {
        assert_eq!(WindowEntry::parse_list(b"").unwrap(), vec![]);
    }
}
