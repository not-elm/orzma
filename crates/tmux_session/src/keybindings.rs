//! Reads tmux key bindings (`list-keys`) and dispatches keypresses against them:
//! a bound key issues its tmux command verbatim, an unbound key is forwarded to
//! the pane. Pure translation + lookup; the binary's input plugin is a thin
//! adapter. `-K` is deliberately NOT used — it mis-encodes named keys under
//! `tmux -CC` — and tmux.conf is the single source of truth for bindings.

use bevy::prelude::Resource;
use std::collections::{HashMap, HashSet};

/// tmux key bindings read from `list-keys`, plus the prefix-key set. Built once
/// on attach; empty until then, so every key forwards pane-direct (the
/// pre-binding behavior).
#[derive(Resource, Default, Debug)]
pub struct KeyBindings {
    root: HashMap<String, String>,
    prefix: HashMap<String, String>,
    prefix_keys: HashSet<String>,
}

impl KeyBindings {
    /// Installs parsed bindings, routing each into its table's map.
    pub(crate) fn install(&mut self, bindings: Vec<KeyBinding>) {
        for binding in bindings {
            match binding.table {
                Table::Root => {
                    self.root.insert(binding.key, binding.command);
                }
                Table::Prefix => {
                    self.prefix.insert(binding.key, binding.command);
                }
            }
        }
    }

    /// Replaces the prefix-key set.
    pub(crate) fn set_prefix_keys(&mut self, keys: HashSet<String>) {
        self.prefix_keys = keys;
    }

    /// Clears all tables (on disconnect, so a reconnect re-reads).
    pub(crate) fn clear(&mut self) {
        self.root.clear();
        self.prefix.clear();
        self.prefix_keys.clear();
    }
}

/// One ordered forwarding action for a frame of key input.
#[derive(Debug)]
pub enum Forwarded {
    /// Run a bound tmux command verbatim over the control connection.
    Run(String),
    /// Forward these key names to the active pane (one batched `send-keys`).
    Keys(Vec<String>),
}

/// Plans the ordered forwarding actions for a frame's tmux key names, threading
/// `prefix_pending` across frames. Consecutive forwarded keys coalesce into one
/// `Keys` batch; a bound key emits a `Run`; the prefix key and unmatched
/// prefix-table keys are swallowed.
pub fn plan_forward(
    prefix_pending: &mut bool,
    bindings: &KeyBindings,
    key_names: impl IntoIterator<Item = String>,
) -> Vec<Forwarded> {
    let mut actions: Vec<Forwarded> = Vec::new();
    for name in key_names {
        match dispatch(prefix_pending, bindings, &name) {
            Dispatch::Run(command) => actions.push(Forwarded::Run(command)),
            Dispatch::Forward => match actions.last_mut() {
                Some(Forwarded::Keys(batch)) => batch.push(name),
                _ => actions.push(Forwarded::Keys(vec![name])),
            },
            Dispatch::Swallow => {}
        }
    }
    actions
}

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
        let (token, tail) = split_first_token(rest);
        match token {
            "-T" => {
                let (name, after) = split_first_token(tail);
                table = match name {
                    "root" => Some(Table::Root),
                    "prefix" => Some(Table::Prefix),
                    _ => return None,
                };
                rest = after;
            }
            "-N" => {
                let (_count, after) = split_first_token(tail);
                rest = after;
            }
            "-r" => {
                repeat = true;
                rest = tail;
            }
            "-a" => rest = tail,
            _ => break,
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

/// What to do with one keypress.
enum Dispatch {
    Run(String),
    Forward,
    Swallow,
}

/// Routes one key name against the bindings, threading prefix state.
fn dispatch(prefix_pending: &mut bool, bindings: &KeyBindings, key_name: &str) -> Dispatch {
    if *prefix_pending {
        *prefix_pending = false;
        return lookup(&bindings.prefix, key_name).map_or(Dispatch::Swallow, Dispatch::Run);
    }
    if bindings.prefix_keys.contains(key_name) {
        *prefix_pending = true;
        return Dispatch::Swallow;
    }
    lookup(&bindings.root, key_name).map_or(Dispatch::Forward, Dispatch::Run)
}

/// Looks up a key in a table, falling back to the table's `Any` binding (tmux
/// runs `Any` when no more-specific key matches).
fn lookup(table: &HashMap<String, String>, key_name: &str) -> Option<String> {
    table.get(key_name).or_else(|| table.get("Any")).cloned()
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

    fn bindings(
        root: &[(&str, &str)],
        prefix: &[(&str, &str)],
        prefix_keys: &[&str],
    ) -> KeyBindings {
        let mut kb = KeyBindings::default();
        kb.install(
            root.iter()
                .map(|(k, c)| KeyBinding {
                    table: Table::Root,
                    key: (*k).to_string(),
                    command: (*c).to_string(),
                    repeat: false,
                })
                .chain(prefix.iter().map(|(k, c)| KeyBinding {
                    table: Table::Prefix,
                    key: (*k).to_string(),
                    command: (*c).to_string(),
                    repeat: false,
                }))
                .collect(),
        );
        kb.set_prefix_keys(prefix_keys.iter().map(|k| (*k).to_string()).collect());
        kb
    }

    #[test]
    fn root_bound_key_runs_its_command() {
        let kb = bindings(&[("M-i", "split-window -h")], &[], &["C-b"]);
        let actions = plan_forward(&mut false, &kb, vec!["M-i".to_string()]);
        assert_eq!(actions.len(), 1);
        assert!(matches!(&actions[0], Forwarded::Run(c) if c == "split-window -h"));
    }

    #[test]
    fn unbound_key_is_forwarded() {
        let kb = bindings(&[("M-i", "split-window -h")], &[], &["C-b"]);
        let actions = plan_forward(&mut false, &kb, vec!["Up".to_string()]);
        assert_eq!(actions.len(), 1);
        assert!(matches!(&actions[0], Forwarded::Keys(v) if v == &vec!["Up".to_string()]));
    }

    #[test]
    fn prefix_then_bound_key_runs_prefix_command() {
        let kb = bindings(&[], &[("c", "new-window")], &["C-b"]);
        let mut pending = false;
        let first = plan_forward(&mut pending, &kb, vec!["C-b".to_string()]);
        assert!(first.is_empty(), "prefix key is swallowed");
        assert!(pending, "prefix is now pending");
        let second = plan_forward(&mut pending, &kb, vec!["c".to_string()]);
        assert!(matches!(&second[0], Forwarded::Run(c) if c == "new-window"));
        assert!(!pending, "pending cleared after the prefix key");
    }

    #[test]
    fn prefix_then_unbound_key_is_swallowed() {
        let kb = bindings(&[], &[("c", "new-window")], &["C-b"]);
        let mut pending = false;
        plan_forward(&mut pending, &kb, vec!["C-b".to_string()]);
        let actions = plan_forward(&mut pending, &kb, vec!["z".to_string()]);
        assert!(
            actions.is_empty(),
            "unbound prefix key is dropped, not forwarded"
        );
        assert!(!pending);
    }

    #[test]
    fn any_binding_is_the_fallback() {
        let kb = bindings(
            &[("M-i", "split-window -h"), ("Any", "display-message hi")],
            &[],
            &["C-b"],
        );
        let specific = plan_forward(&mut false, &kb, vec!["M-i".to_string()]);
        assert!(matches!(&specific[0], Forwarded::Run(c) if c == "split-window -h"));
        let fallback = plan_forward(&mut false, &kb, vec!["X".to_string()]);
        assert!(matches!(&fallback[0], Forwarded::Run(c) if c == "display-message hi"));
    }

    #[test]
    fn consecutive_forwards_coalesce_then_split_on_run() {
        let kb = bindings(&[("M-i", "split-window -h")], &[], &["C-b"]);
        let actions = plan_forward(
            &mut false,
            &kb,
            vec![
                "a".to_string(),
                "b".to_string(),
                "M-i".to_string(),
                "c".to_string(),
            ],
        );
        assert_eq!(actions.len(), 3);
        assert!(
            matches!(&actions[0], Forwarded::Keys(v) if v == &vec!["a".to_string(), "b".to_string()])
        );
        assert!(matches!(&actions[1], Forwarded::Run(c) if c == "split-window -h"));
        assert!(matches!(&actions[2], Forwarded::Keys(v) if v == &vec!["c".to_string()]));
    }
}
