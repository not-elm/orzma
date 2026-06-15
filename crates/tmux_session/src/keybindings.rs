//! Reads tmux key bindings (`list-keys`) and dispatches keypresses against them:
//! a bound key issues its tmux command verbatim, an unbound key is forwarded to
//! the pane. Pure translation + lookup; the binary's input plugin is a thin
//! adapter. `-K` is deliberately NOT used — it mis-encodes named keys under
//! `tmux -CC` — and tmux.conf is the single source of truth for bindings.

use std::collections::HashSet;

/// Builds `list-keys -T <table>` to list one key table's bindings.
pub(crate) fn list_keys_command(table: &str) -> String {
    format!("list-keys -T {table}")
}

/// Builds `display-message -p '#{prefix} #{prefix2}'`. `prefix`/`prefix2` are
/// session options; tmux expands an option name placed inside `#{}`.
pub(crate) fn prefix_options_command() -> String {
    "display-message -p '#{prefix} #{prefix2}'".to_string()
}

/// Which tmux key table a binding belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Table {
    Root,
    Prefix,
}

/// One parsed tmux key binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KeyBinding {
    pub(crate) table: Table,
    pub(crate) key: String,
    pub(crate) command: String,
    pub(crate) repeat: bool,
}

/// Parses `list-keys -T <table>` reply lines into bindings, skipping any line
/// that does not parse (per-line resilience — one exotic binding must not drop
/// the rest).
pub(crate) fn parse_list_keys(lines: &[String]) -> Vec<KeyBinding> {
    lines
        .iter()
        .filter_map(|line| parse_binding_line(line))
        .collect()
}

/// Parses a `#{prefix} #{prefix2}` reply line into the set of prefix keys,
/// skipping the literal `none` / `None` (tmux's unset `prefix2`).
pub(crate) fn parse_prefix(line: &str) -> HashSet<String> {
    line.split_whitespace()
        .filter(|t| !t.eq_ignore_ascii_case("none"))
        .map(str::to_owned)
        .collect()
}

/// Parses one `bind-key [flags…] -T <table> <key> <command…>` line. Tokenizes
/// the leading flags (order-independent: `-r`, `-N <n>`, `-a`, `-T <table>`),
/// takes the next token as the backslash-escaped key, and keeps the remainder
/// verbatim as the command (tmux re-parses it on re-send).
fn parse_binding_line(line: &str) -> Option<KeyBinding> {
    let rest = line.trim_start();
    let mut rest = rest
        .strip_prefix("bind-key")
        .or_else(|| rest.strip_prefix("bind"))?;
    let mut table: Option<Table> = None;
    let mut repeat = false;
    loop {
        rest = rest.trim_start();
        if let Some(after) = rest.strip_prefix("-T") {
            let (name, tail) = split_first_token(after);
            table = match name {
                "root" => Some(Table::Root),
                "prefix" => Some(Table::Prefix),
                _ => return None,
            };
            rest = tail;
        } else if let Some(after) = rest.strip_prefix("-N") {
            let (_count, tail) = split_first_token(after);
            rest = tail;
        } else if let Some(after) = rest.strip_prefix("-r") {
            repeat = true;
            rest = after;
        } else if let Some(after) = rest.strip_prefix("-a") {
            rest = after;
        } else {
            break;
        }
    }
    let (key_raw, command) = split_first_token(rest);
    if key_raw.is_empty() {
        return None;
    }
    Some(KeyBinding {
        table: table?,
        key: unescape_key(key_raw),
        command: command.trim().to_string(),
        repeat,
    })
}

/// Splits off the first whitespace-delimited token, returning it and the
/// remainder (original spacing preserved after the token).
fn split_first_token(s: &str) -> (&str, &str) {
    let s = s.trim_start();
    match s.find(char::is_whitespace) {
        Some(i) => (&s[..i], &s[i..]),
        None => (s, ""),
    }
}

/// Removes tmux's backslash escaping from a key token (e.g. `\;` → `;`).
fn unescape_key(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(next) = chars.next() {
                out.push(next);
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_keys_command_targets_table() {
        assert_eq!(list_keys_command("root"), "list-keys -T root");
        assert_eq!(list_keys_command("prefix"), "list-keys -T prefix");
    }

    #[test]
    fn prefix_options_command_reads_both_prefixes() {
        assert_eq!(
            prefix_options_command(),
            "display-message -p '#{prefix} #{prefix2}'"
        );
    }

    #[test]
    fn parses_root_binding() {
        let lines =
            vec!["bind-key    -T root         M-i               split-window -h".to_string()];
        let got = parse_list_keys(&lines);
        assert_eq!(
            got,
            vec![KeyBinding {
                table: Table::Root,
                key: "M-i".to_string(),
                command: "split-window -h".to_string(),
                repeat: false,
            }]
        );
    }

    #[test]
    fn parses_repeat_flag_before_table() {
        let lines = vec!["bind-key -r -T prefix Up   resize-pane -U".to_string()];
        let got = parse_list_keys(&lines);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].table, Table::Prefix);
        assert_eq!(got[0].key, "Up");
        assert_eq!(got[0].command, "resize-pane -U");
        assert!(got[0].repeat);
    }

    #[test]
    fn unescapes_backslash_escaped_key() {
        let lines = vec![r#"bind-key -T prefix \;   last-pane"#.to_string()];
        let got = parse_list_keys(&lines);
        assert_eq!(got[0].key, ";");
        assert_eq!(got[0].command, "last-pane");
    }

    #[test]
    fn preserves_command_internal_spacing_and_semicolons() {
        let lines =
            vec![r#"bind-key -T root C-x   display-message "a  b" \; refresh-client"#.to_string()];
        let got = parse_list_keys(&lines);
        assert_eq!(
            got[0].command,
            r#"display-message "a  b" \; refresh-client"#
        );
    }

    #[test]
    fn skips_unparseable_lines_keeping_others() {
        let lines = vec![
            "garbage line".to_string(),
            "bind-key -T root a   new-window".to_string(),
        ];
        let got = parse_list_keys(&lines);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].key, "a");
    }

    #[test]
    fn parse_prefix_skips_none_case_insensitive() {
        assert_eq!(parse_prefix("C-b None"), HashSet::from(["C-b".to_string()]));
        assert_eq!(
            parse_prefix("C-a C-b"),
            HashSet::from(["C-a".to_string(), "C-b".to_string()])
        );
        assert!(parse_prefix("none None").is_empty());
    }
}
