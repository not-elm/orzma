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
    copy_mode: HashMap<String, String>,
    copy_mode_vi: HashMap<String, String>,
    mode_keys: ModeKeys,
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
                Table::CopyMode => {
                    self.copy_mode.insert(binding.key, binding.command);
                }
                Table::CopyModeVi => {
                    self.copy_mode_vi.insert(binding.key, binding.command);
                }
            }
        }
    }

    /// Replaces the prefix-key set.
    pub(crate) fn set_prefix_keys(&mut self, keys: HashSet<String>) {
        self.prefix_keys = keys;
    }

    /// Sets which copy-mode table is active (from the `mode-keys` option).
    pub(crate) fn set_mode_keys(&mut self, mode_keys: ModeKeys) {
        self.mode_keys = mode_keys;
    }

    /// Looks up `key` in the active copy-mode table (vi or emacs per `mode-keys`),
    /// falling back to the table's `Any` binding. Returns the bound tmux command.
    pub(crate) fn copy_command(&self, key: &str) -> Option<String> {
        let table = match self.mode_keys {
            ModeKeys::Vi => &self.copy_mode_vi,
            ModeKeys::Emacs => &self.copy_mode,
        };
        lookup(table, key)
    }

    /// Clears all tables (on disconnect, so a reconnect re-reads).
    pub(crate) fn clear(&mut self) {
        self.root.clear();
        self.prefix.clear();
        self.prefix_keys.clear();
        self.copy_mode.clear();
        self.copy_mode_vi.clear();
        self.mode_keys = ModeKeys::default();
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

/// What ozmux does with one key while a pane is in copy mode. The bound tmux
/// command runs verbatim (`Relay`/`Copy`/`Exit`); ozmux adds only the side
/// effects tmux cannot supply over the control channel (`Prompt`).
#[derive(Debug)]
pub enum CopyAction {
    /// Run the bound command verbatim against the active pane.
    Relay(String),
    /// Run the bound copy command verbatim, then (after its reply) bridge the
    /// clipboard. `pipes` is true for `copy-pipe*`/`pipe*` (already piped to an
    /// external command — no bridge); `and_cancel` also exits copy mode.
    Copy {
        /// The verbatim tmux command to run.
        command: String,
        /// True when the binding pipes externally (skip the `show-buffer` bridge).
        pipes: bool,
        /// True when the binding ends copy mode (`*-and-cancel`).
        and_cancel: bool,
    },
    /// The binding is `command-prompt`-wrapped; ozmux opens its own prompt and
    /// builds the inner `send-keys -X` from `kind` with the typed text.
    Prompt {
        /// Which copy command the prompt feeds.
        kind: PromptKind,
    },
    /// Run the bound `cancel` verbatim and remove the copy-mode marker.
    Exit(String),
    /// Key not bound in the active copy table — do nothing (tmux ignores it too).
    Ignore,
}

/// The copy command an ozmux prompt feeds once the user submits text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    /// `/` — search down (regex prompt).
    SearchForward,
    /// `?` — search up (regex prompt).
    SearchBackward,
    /// `f` — jump to char forward (single-char prompt).
    JumpForward,
    /// `F` — jump to char backward (single-char prompt).
    JumpBackward,
    /// `t` — jump till char forward (single-char prompt).
    JumpToForward,
    /// `T` — jump till char backward (single-char prompt).
    JumpToBackward,
}

impl PromptKind {
    /// The tmux `-X` copy command name this prompt feeds.
    pub fn copy_command(self) -> &'static str {
        match self {
            PromptKind::SearchForward => "search-forward",
            PromptKind::SearchBackward => "search-backward",
            PromptKind::JumpForward => "jump-forward",
            PromptKind::JumpBackward => "jump-backward",
            PromptKind::JumpToForward => "jump-to-forward",
            PromptKind::JumpToBackward => "jump-to-backward",
        }
    }

    /// True for jump prompts, which read exactly one character.
    pub fn is_single_char(self) -> bool {
        !matches!(self, PromptKind::SearchForward | PromptKind::SearchBackward)
    }
}

/// Classifies one key (already known to be pressed while in copy mode) against
/// the active copy-mode table. Looks up the bound tmux command and decides the
/// side effects ozmux must add; the command itself runs verbatim.
pub fn copy_mode_dispatch(bindings: &KeyBindings, key_name: &str) -> CopyAction {
    let Some(command) = bindings.copy_command(key_name) else {
        return CopyAction::Ignore;
    };
    if command.trim_start().starts_with("command-prompt") {
        if let Some(kind) = prompt_kind(&command) {
            return CopyAction::Prompt { kind };
        }
        return CopyAction::Relay(command);
    }
    if command.contains("copy-pipe")
        || command.contains("copy-selection")
        || command.contains(" pipe")
    {
        return CopyAction::Copy {
            pipes: command.contains("pipe"),
            and_cancel: command.contains("-and-cancel"),
            command,
        };
    }
    if copy_command_is_cancel(&command) {
        return CopyAction::Exit(command);
    }
    CopyAction::Relay(command)
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
    CopyMode,
    CopyModeVi,
}

/// Which copy-mode key table is active, from tmux's `mode-keys` option.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ModeKeys {
    /// `mode-keys vi` → the `copy-mode-vi` table.
    Vi,
    /// `mode-keys emacs` → the `copy-mode` table (tmux's default).
    #[default]
    Emacs,
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
                    "copy-mode" => Some(Table::CopyMode),
                    "copy-mode-vi" => Some(Table::CopyModeVi),
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

/// Detects the `PromptKind` of a `command-prompt`-wrapped binding by the inner
/// `search-*` / `jump-*` command name. Specific names (`jump-to-*`,
/// `search-backward`) are tested before their prefixes.
fn prompt_kind(command: &str) -> Option<PromptKind> {
    if command.contains("search-backward") {
        Some(PromptKind::SearchBackward)
    } else if command.contains("search-forward") {
        Some(PromptKind::SearchForward)
    } else if command.contains("jump-to-forward") {
        Some(PromptKind::JumpToForward)
    } else if command.contains("jump-to-backward") {
        Some(PromptKind::JumpToBackward)
    } else if command.contains("jump-backward") {
        Some(PromptKind::JumpBackward)
    } else if command.contains("jump-forward") {
        Some(PromptKind::JumpForward)
    } else {
        None
    }
}

/// True when the bound command's `-X` action is exactly `cancel` (not
/// `*-and-cancel`, which is handled as a `Copy`).
fn copy_command_is_cancel(command: &str) -> bool {
    command.split_whitespace().last() == Some("cancel")
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

    #[test]
    fn parses_copy_mode_vi_binding() {
        let lines = vec!["bind-key -T copy-mode-vi j send-keys -X cursor-down".to_string()];
        let got = parse_list_keys(&lines);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].table, Table::CopyModeVi);
        assert_eq!(got[0].key, "j");
        assert_eq!(got[0].command, "send-keys -X cursor-down");
    }

    #[test]
    fn parses_copy_mode_emacs_binding() {
        let lines = vec!["bind-key -T copy-mode C-n send-keys -X cursor-down".to_string()];
        let got = parse_list_keys(&lines);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].table, Table::CopyMode);
        assert_eq!(got[0].key, "C-n");
        assert_eq!(got[0].command, "send-keys -X cursor-down");
    }

    #[test]
    fn copy_table_selects_vi_or_emacs_by_mode_keys() {
        let mut kb = KeyBindings::default();
        kb.install(vec![
            KeyBinding {
                table: Table::CopyModeVi,
                key: "j".into(),
                command: "vi-down".into(),
                repeat: false,
            },
            KeyBinding {
                table: Table::CopyMode,
                key: "j".into(),
                command: "emacs-down".into(),
                repeat: false,
            },
        ]);
        kb.set_mode_keys(ModeKeys::Vi);
        assert_eq!(kb.copy_command("j"), Some("vi-down".to_string()));
        kb.set_mode_keys(ModeKeys::Emacs);
        assert_eq!(kb.copy_command("j"), Some("emacs-down".to_string()));
    }

    #[test]
    fn clear_drops_copy_tables_and_mode_keys() {
        let mut kb = KeyBindings::default();
        kb.install(vec![KeyBinding {
            table: Table::CopyModeVi,
            key: "j".into(),
            command: "x".into(),
            repeat: false,
        }]);
        kb.set_mode_keys(ModeKeys::Vi);
        kb.clear();
        assert_eq!(kb.copy_command("j"), None);
    }

    #[test]
    fn copy_command_falls_back_to_any() {
        let mut kb = KeyBindings::default();
        kb.install(vec![KeyBinding {
            table: Table::CopyModeVi,
            key: "Any".into(),
            command: "cancel".into(),
            repeat: false,
        }]);
        kb.set_mode_keys(ModeKeys::Vi);
        assert_eq!(kb.copy_command("Escape"), Some("cancel".to_string()));
    }

    fn vi_bindings(pairs: &[(&str, &str)]) -> KeyBindings {
        let mut kb = KeyBindings::default();
        kb.install(
            pairs
                .iter()
                .map(|(k, c)| KeyBinding {
                    table: Table::CopyModeVi,
                    key: (*k).into(),
                    command: (*c).into(),
                    repeat: false,
                })
                .collect(),
        );
        kb.set_mode_keys(ModeKeys::Vi);
        kb
    }

    #[test]
    fn motion_relays_verbatim() {
        let kb = vi_bindings(&[("j", "send-keys -X cursor-down")]);
        assert!(matches!(copy_mode_dispatch(&kb, "j"),
            CopyAction::Relay(c) if c == "send-keys -X cursor-down"));
    }

    #[test]
    fn unbound_key_is_ignored() {
        let kb = vi_bindings(&[("j", "send-keys -X cursor-down")]);
        assert!(matches!(copy_mode_dispatch(&kb, "z"), CopyAction::Ignore));
    }

    #[test]
    fn cancel_is_exit() {
        let kb = vi_bindings(&[("q", "send-keys -X cancel")]);
        assert!(matches!(copy_mode_dispatch(&kb, "q"), CopyAction::Exit(_)));
    }

    #[test]
    fn copy_selection_and_cancel_is_copy_with_exit() {
        let kb = vi_bindings(&[("y", "send-keys -X copy-selection-and-cancel")]);
        match copy_mode_dispatch(&kb, "y") {
            CopyAction::Copy {
                pipes, and_cancel, ..
            } => {
                assert!(!pipes);
                assert!(and_cancel);
            }
            other => panic!("expected Copy, got {other:?}"),
        }
    }

    #[test]
    fn copy_pipe_is_copy_with_pipes_true() {
        let kb = vi_bindings(&[("Y", "send-keys -X copy-pipe-and-cancel pbcopy")]);
        match copy_mode_dispatch(&kb, "Y") {
            CopyAction::Copy {
                pipes, and_cancel, ..
            } => {
                assert!(pipes, "copy-pipe* must not be clipboard-bridged");
                assert!(and_cancel);
            }
            other => panic!("expected Copy, got {other:?}"),
        }
    }

    #[test]
    fn command_prompt_search_forward_is_prompt() {
        let kb = vi_bindings(&[(
            "/",
            r#"command-prompt -T search -p "(search down)" { send-keys -X search-forward "%%%" }"#,
        )]);
        assert!(matches!(
            copy_mode_dispatch(&kb, "/"),
            CopyAction::Prompt {
                kind: PromptKind::SearchForward
            }
        ));
    }

    #[test]
    fn command_prompt_jump_forward_is_single_char_prompt() {
        let kb = vi_bindings(&[(
            "f",
            r#"command-prompt -1 -p "(jump forward)" { send-keys -X jump-forward "%%%" }"#,
        )]);
        match copy_mode_dispatch(&kb, "f") {
            CopyAction::Prompt { kind } => {
                assert_eq!(kind, PromptKind::JumpForward);
                assert!(kind.is_single_char());
            }
            other => panic!("expected Prompt, got {other:?}"),
        }
    }

    #[test]
    fn bare_word_search_relays_verbatim_not_prompt() {
        let kb = vi_bindings(&[(
            "*",
            r##"send-keys -FX search-forward "#{copy_cursor_word}""##,
        )]);
        assert!(matches!(copy_mode_dispatch(&kb, "*"), CopyAction::Relay(_)));
    }

    #[test]
    fn copy_selection_without_cancel_keeps_mode_open() {
        let kb = vi_bindings(&[("Enter", "send-keys -X copy-selection")]);
        match copy_mode_dispatch(&kb, "Enter") {
            CopyAction::Copy {
                pipes, and_cancel, ..
            } => {
                assert!(!pipes);
                assert!(!and_cancel);
            }
            other => panic!("expected Copy, got {other:?}"),
        }
    }

    #[test]
    fn command_prompt_search_backward_is_prompt() {
        let kb = vi_bindings(&[(
            "?",
            r#"command-prompt -T search -p "(search up)" { send-keys -X search-backward "%%%" }"#,
        )]);
        assert!(matches!(
            copy_mode_dispatch(&kb, "?"),
            CopyAction::Prompt {
                kind: PromptKind::SearchBackward
            }
        ));
    }
}
