//! In-memory mirror of tmux key bindings parsed from `list-keys` output.
//!
//! Cosmetic / display-only and off the critical path: ozmux does not execute
//! these — tmux remains the actor. Parses the human-readable
//! `bind-key [-r] [-N ...] [-n] -T <table> <key> <command...>` lines (tmux's
//! `list-keys -F` custom format is unavailable before 3.7).

use bevy::prelude::Resource;

/// One parsed tmux key binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KeyBinding {
    /// Key table the binding lives in (`-n` is normalized to `root`).
    pub(crate) table: String,
    /// The key chord, verbatim (tmux's own escaping like `\;` is preserved).
    pub(crate) key: String,
    /// The bound command tail, verbatim.
    pub(crate) command: String,
}

/// The synced mirror of tmux's key tables, refreshed on attach.
#[derive(Resource, Default)]
pub(crate) struct TmuxKeyBindings {
    pub(crate) bindings: Vec<KeyBinding>,
}

/// Parses `list-keys` output lines into [`KeyBinding`]s, skipping lines that are
/// not `bind-key` rows.
pub(crate) fn parse_key_bindings(lines: &[String]) -> Vec<KeyBinding> {
    lines.iter().filter_map(|line| parse_line(line)).collect()
}

fn parse_line(line: &str) -> Option<KeyBinding> {
    let mut tokens = line.split_whitespace();
    if tokens.next()? != "bind-key" {
        return None;
    }
    let mut table: Option<String> = None;
    let mut no_prefix = false;
    let key;
    loop {
        let tok = tokens.next()?;
        match tok {
            "-r" => continue,
            "-n" => {
                no_prefix = true;
                continue;
            }
            "-N" => {
                tokens.next();
                continue;
            }
            "-T" => {
                table = Some(tokens.next()?.to_string());
                continue;
            }
            other => {
                key = other.to_string();
                break;
            }
        }
    }
    let table = table.unwrap_or_else(|| {
        if no_prefix {
            "root".to_string()
        } else {
            "prefix".to_string()
        }
    });
    // NOTE: the command tail is free-form (may contain spaces, quotes, braces);
    // take everything after the key token verbatim rather than re-splitting.
    let consumed = consumed_prefix_len(line, &key)?;
    let command = line[consumed..].trim().to_string();
    Some(KeyBinding {
        table,
        key,
        command,
    })
}

fn consumed_prefix_len(line: &str, key: &str) -> Option<usize> {
    let key_start = find_key_token(line, key)?;
    Some(key_start + key.len())
}

fn find_key_token(line: &str, key: &str) -> Option<usize> {
    let mut search_from = 0;
    while let Some(rel) = line[search_from..].find(key) {
        let abs = search_from + rel;
        let before_ok = abs == 0 || line.as_bytes()[abs - 1] == b' ';
        let after = abs + key.len();
        let after_ok = after == line.len() || line.as_bytes()[after] == b' ';
        if before_ok && after_ok {
            return Some(abs);
        }
        search_from = abs + key.len();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(raw: &[&str]) -> Vec<String> {
        raw.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_prefix_binding() {
        let got = parse_key_bindings(&lines(&["bind-key -T prefix c new-window"]));
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].table, "prefix");
        assert_eq!(got[0].key, "c");
        assert_eq!(got[0].command, "new-window");
    }

    #[test]
    fn normalizes_no_prefix_flag_to_root() {
        let got = parse_key_bindings(&lines(&["bind-key -n M-Left select-pane -L"]));
        assert_eq!(got[0].table, "root");
        assert_eq!(got[0].key, "M-Left");
        assert_eq!(got[0].command, "select-pane -L");
    }

    #[test]
    fn ignores_repeat_flag_and_keeps_command_with_spaces() {
        let got = parse_key_bindings(&lines(&["bind-key -r -T prefix Left resize-pane -L 5"]));
        assert_eq!(got[0].key, "Left");
        assert_eq!(got[0].command, "resize-pane -L 5");
    }

    #[test]
    fn keeps_command_with_braces_and_quotes() {
        let line = "bind-key -T copy-mode F command-prompt -1 -p \"(jump backward)\" { send-keys -X jump-backward }";
        let got = parse_key_bindings(&lines(&[line]));
        assert_eq!(got[0].table, "copy-mode");
        assert_eq!(got[0].key, "F");
        assert_eq!(
            got[0].command,
            "command-prompt -1 -p \"(jump backward)\" { send-keys -X jump-backward }"
        );
    }

    #[test]
    fn skips_non_bind_key_lines() {
        let got = parse_key_bindings(&lines(&["", "Table: prefix", "bind-key -T root q detach"]));
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].command, "detach");
    }
}
