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

fn next_token(line: &str, from: usize) -> Option<(usize, usize)> {
    let bytes = line.as_bytes();
    let mut i = from;
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
        i += 1;
    }
    if i >= bytes.len() {
        return None;
    }
    let start = i;
    while i < bytes.len() && bytes[i] != b' ' && bytes[i] != b'\t' {
        i += 1;
    }
    Some((start, i))
}

fn parse_line(line: &str) -> Option<KeyBinding> {
    let (s, e) = next_token(line, 0)?;
    if &line[s..e] != "bind-key" {
        return None;
    }
    let mut pos = e;
    let mut table: Option<String> = None;
    let mut no_prefix = false;
    let key;
    let key_end;
    loop {
        let (ts, te) = next_token(line, pos)?;
        match &line[ts..te] {
            "-r" => pos = te,
            "-n" => {
                no_prefix = true;
                pos = te;
            }
            "-N" => {
                let (_, ae) = next_token(line, te)?;
                pos = ae;
            }
            "-T" => {
                let (vs, ve) = next_token(line, te)?;
                table = Some(line[vs..ve].to_string());
                pos = ve;
            }
            other => {
                key = other.to_string();
                key_end = te;
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
    let command = line[key_end..].trim().to_string();
    Some(KeyBinding {
        table,
        key,
        command,
    })
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
    fn key_equal_to_table_name_is_located_correctly() {
        let got = parse_key_bindings(&lines(&["bind-key -T prefix prefix send-prefix"]));
        assert_eq!(got[0].table, "prefix");
        assert_eq!(got[0].key, "prefix");
        assert_eq!(got[0].command, "send-prefix");
    }

    #[test]
    fn note_argument_equal_to_key_is_located_correctly() {
        let got = parse_key_bindings(&lines(&[
            "bind-key -N Enter -T copy-mode Enter copy-selection",
        ]));
        assert_eq!(got[0].table, "copy-mode");
        assert_eq!(got[0].key, "Enter");
        assert_eq!(got[0].command, "copy-selection");
    }

    #[test]
    fn skips_non_bind_key_lines() {
        let got = parse_key_bindings(&lines(&["", "Table: prefix", "bind-key -T root q detach"]));
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].command, "detach");
    }
}
