//! Sans-IO parsing of `tmux list-sessions` output into typed [`SessionInfo`].

use crate::error::{TmuxError, TmuxResult};
use std::str;
use tmux_control_parser::SessionId;

/// A tmux session reported by `list-sessions`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInfo {
    /// tmux session id (`$N`).
    pub id: SessionId,
    /// Session name (may contain spaces, never a raw tab).
    pub name: String,
    /// Number of windows in the session.
    pub windows: u32,
    /// Whether at least one client is attached.
    pub attached: bool,
    /// Session creation time (unix seconds).
    pub created: u64,
}

// NOTE: real tab bytes (0x09) separate the fields — `session_name` is LAST so a
// `splitn(5, b'\t')` keeps the free-text name in the final field even if a
// delimiter ever leaks in. tmux escapes a tab in a name to the literal `\t`.
pub(crate) const LIST_FORMAT: &str =
    "#{session_id}\t#{session_windows}\t#{session_attached}\t#{session_created}\t#{session_name}";

impl SessionInfo {
    /// Parses the tab-separated `list-sessions -F` output (one session per line).
    pub fn parse_list(output: &[u8]) -> TmuxResult<Vec<SessionInfo>> {
        let mut sessions = Vec::new();
        for line in output.split(|&b| b == b'\n') {
            if line.is_empty() {
                continue;
            }
            sessions.push(parse_line(line)?);
        }
        Ok(sessions)
    }
}

fn parse_line(line: &[u8]) -> TmuxResult<SessionInfo> {
    let mut fields = line.splitn(5, |&b| b == b'\t');
    let id = fields.next().and_then(parse_session_id).ok_or_else(|| malformed(line))?;
    let windows = fields.next().and_then(parse_u32).ok_or_else(|| malformed(line))?;
    let attached = fields.next().and_then(parse_u32).ok_or_else(|| malformed(line))? > 0;
    let created = fields.next().and_then(parse_u64).ok_or_else(|| malformed(line))?;
    let name_bytes = fields.next().ok_or_else(|| malformed(line))?;
    let name = str::from_utf8(name_bytes).map_err(|_| malformed(line))?.to_string();
    Ok(SessionInfo { id, name, windows, attached, created })
}

fn malformed(line: &[u8]) -> TmuxError {
    TmuxError::MalformedSessionList {
        line: String::from_utf8_lossy(line).into_owned(),
    }
}

fn parse_session_id(field: &[u8]) -> Option<SessionId> {
    let digits = field.strip_prefix(b"$")?;
    Some(SessionId(str::from_utf8(digits).ok()?.parse().ok()?))
}

fn parse_u32(field: &[u8]) -> Option<u32> {
    str::from_utf8(field).ok()?.parse().ok()
}

fn parse_u64(field: &[u8]) -> Option<u64> {
    str::from_utf8(field).ok()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_single_session() {
        let out = b"$0\t3\t1\t1781400000\tmain\n";
        assert_eq!(
            SessionInfo::parse_list(out).unwrap(),
            vec![SessionInfo {
                id: SessionId(0),
                name: "main".to_string(),
                windows: 3,
                attached: true,
                created: 1781400000,
            }]
        );
    }

    #[test]
    fn parse_multiple_preserves_order() {
        let out = b"$0\t1\t0\t10\tone\n$1\t2\t1\t20\ttwo\n";
        let got = SessionInfo::parse_list(out).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!((got[0].id, got[0].name.as_str()), (SessionId(0), "one"));
        assert_eq!((got[1].id, got[1].name.as_str()), (SessionId(1), "two"));
    }

    #[test]
    fn parse_empty_input() {
        assert_eq!(SessionInfo::parse_list(b"").unwrap(), vec![]);
    }

    #[test]
    fn trailing_newline_optional() {
        let with = SessionInfo::parse_list(b"$0\t1\t0\t1\tx\n").unwrap();
        let without = SessionInfo::parse_list(b"$0\t1\t0\t1\tx").unwrap();
        assert_eq!(with, without);
    }

    #[test]
    fn name_with_spaces() {
        let out = b"$5\t1\t0\t1\tmy work session\n";
        assert_eq!(SessionInfo::parse_list(out).unwrap()[0].name, "my work session");
    }

    #[test]
    fn attached_count_to_bool() {
        let out = b"$0\t1\t0\t1\ta\n$1\t1\t1\t1\tb\n$2\t1\t2\t1\tc\n";
        let got = SessionInfo::parse_list(out).unwrap();
        assert_eq!(
            (got[0].attached, got[1].attached, got[2].attached),
            (false, true, true)
        );
    }

    #[test]
    fn too_few_fields_errors() {
        let out = b"$0\t1\t0\t1\n";
        assert!(matches!(
            SessionInfo::parse_list(out),
            Err(TmuxError::MalformedSessionList { .. })
        ));
    }

    #[test]
    fn non_numeric_field_errors() {
        let out = b"$0\tnotnum\t0\t1\tx\n";
        assert!(matches!(
            SessionInfo::parse_list(out),
            Err(TmuxError::MalformedSessionList { .. })
        ));
    }

    #[test]
    fn bad_id_errors() {
        let no_dollar = b"0\t1\t0\t1\tx\n";
        let non_numeric = b"$x\t1\t0\t1\tx\n";
        assert!(matches!(
            SessionInfo::parse_list(no_dollar),
            Err(TmuxError::MalformedSessionList { .. })
        ));
        assert!(matches!(
            SessionInfo::parse_list(non_numeric),
            Err(TmuxError::MalformedSessionList { .. })
        ));
    }

    #[test]
    fn blank_lines_skipped() {
        let out = b"\n$0\t1\t0\t1\tx\n\n";
        assert_eq!(SessionInfo::parse_list(out).unwrap().len(), 1);
    }

    #[test]
    fn invalid_utf8_name_errors() {
        let out = b"$0\t1\t0\t1\t\xff\xfe\n";
        assert!(matches!(
            SessionInfo::parse_list(out),
            Err(TmuxError::MalformedSessionList { .. })
        ));
    }

    #[test]
    fn escaped_tab_in_name_kept_verbatim() {
        let out = b"$2\t1\t0\t1\tx\\ty\n";
        assert_eq!(SessionInfo::parse_list(out).unwrap()[0].name, "x\\ty");
    }
}
