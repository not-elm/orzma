# tmux Phase 4 — Session UX + keybind mirror Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a choose-tree-style session switcher popup (switch via `switch-client`), connection-recovery overlay, and a tmux keybind mirror to the already-projecting tmux backend, alongside the old multiplexer.

**Architecture:** All tmux-facing logic stays in `crates/tmux_session` (`ozmux_tmux`) on top of `crates/tmux_control`. The chooser reads sessions/windows via one-shot `TmuxServer` subprocess queries (same socket, works attached or not — matching the boot path); switching, window-select, and the keybind mirror go through the live control client. A `%session-changed`/`%client-session-changed` to a different session id tears the window/pane projection down (reusing the existing `TmuxWindowsRetained` observer) and re-enumerates. The detached/error overlay extends the existing `tmux_dialog`.

**Tech Stack:** Rust 2024, Bevy 0.18 ECS, `crates/tmux_control` (portable-pty transport + sans-io parser), the repo's command-builder + `EnumerationState` correlation patterns.

**Spec:** `docs/superpowers/specs/2026-06-15-tmux-phase4-session-ux-design.md`

**Conventions (from `.claude/rules/rust.md`):** no `mod.rs`; only `// TODO:`/`// NOTE:`/`// SAFETY:` comments; doc-comment every `pub` item; all `use` at the top in one block; mutable params first; private items last in a block; gate whole-system change checks with `run_if`. Every commit message ends with:
```
Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
```

**Test commands:**
- `tmux_control` crate: `cargo test -p tmux_control <name>`
- `ozmux_tmux` crate: `cargo test -p ozmux_tmux <name>`
- binary (`ozmux-gui`): `cargo test -p ozmux-gui <name>`
- gated real-tmux: `cargo test -p ozmux_tmux --test <file> -- --ignored`
- lint/format before each commit: `cargo clippy -p <crate> --all-targets && cargo fmt`

---

## File Structure

**`crates/tmux_control/` (subprocess queries for the chooser):**
- Create `src/window_list.rs` — `WindowEntry` + `WindowEntry::parse_list` (tab-separated `list-windows -a` rows).
- Modify `src/transport.rs` — `TmuxServer::list_windows_all()`, `TmuxServer::create_detached_session()`, and their argv helpers.
- Modify `src/error.rs` — add `MalformedWindowList { line }`.
- Modify `src/lib.rs` — re-export `WindowEntry` and `WindowId`.

**`crates/tmux_session/` (control-mode commands, mirror, switch reducer):**
- Modify `src/enumerate.rs` — `switch_client_command`, `list_keys_command`, `list_keys_pending` field on `EnumerationState`.
- Create `src/keybinds.rs` — `KeyBinding`, `TmuxKeyBindings`, `parse_key_bindings` (all `pub(crate)`).
- Modify `src/event_pump.rs` — handle `ClientSessionChanged`; `take_key_bindings`; `detect_session_switch` helper.
- Modify `src/plugin.rs` — register `TmuxKeyBindings` + `keybinds` module; send `list-keys` on attach; wire switch detection (teardown + re-enumerate).
- Modify `src/lib.rs` — re-export `switch_client_command` (keybind types stay crate-private).

**`src/` (binary — chooser tree, switch, overlay):**
- Modify `src/tmux_picker.rs` — `SessionPicker` gains windows; `PickerRow`/`build_rows`/`row_target`; refresh on open transition; switch-vs-attach selection; tree rendering.
- Modify `src/ui/tmux_dialog.rs` — render state-specific text for `Detached` as well as `Error`.

**`crates/tmux_session/tests/` (gated integration):**
- Create `tests/real_tmux_switch.rs` — switch-client rebuild, list-windows-all, list-keys mirror, new-session+switch.

---

## Task 1: `WindowEntry` parser (tmux_control)

**Files:**
- Create: `crates/tmux_control/src/window_list.rs`
- Modify: `crates/tmux_control/src/error.rs` (add `MalformedWindowList`)
- Modify: `crates/tmux_control/src/lib.rs` (declare module + re-export)

- [ ] **Step 1: Add the error variant**

In `crates/tmux_control/src/error.rs`, inside `pub enum TmuxError`, after the `MalformedSessionList { line }` variant:

```rust
    /// A `list-windows` output line could not be parsed.
    #[error("malformed list-windows line: {line}")]
    MalformedWindowList {
        /// The offending line, verbatim.
        line: String,
    },
```

- [ ] **Step 2: Write the failing test (new file with parser + tests)**

Create `crates/tmux_control/src/window_list.rs`:

```rust
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

// NOTE: tab-separated; both free-text names are kept intact because tmux escapes
// a raw tab in a name to the literal `\t`, so a fixed `splitn(6, b'\t')` never
// loses a field. `window_name` is last as defence in depth.
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
        assert!(got[0].window_active);
        assert_eq!(got[1].window_id, WindowId(1));
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
    fn empty_is_empty() {
        assert_eq!(WindowEntry::parse_list(b"").unwrap(), vec![]);
    }
}
```

In `crates/tmux_control/src/lib.rs`, add `mod window_list;` with the other `mod` lines and re-export beside `pub use crate::session::SessionInfo;`:

```rust
pub use crate::window_list::WindowEntry;
pub use tmux_control_parser::WindowId;
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test -p tmux_control window_list`
Expected: 5 tests PASS.

- [ ] **Step 4: Lint + format**

Run: `cargo clippy -p tmux_control --all-targets && cargo fmt`
Expected: no warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/tmux_control/src/window_list.rs crates/tmux_control/src/error.rs crates/tmux_control/src/lib.rs
git commit -m "feat(tmux_control): parse list-windows -a into WindowEntry

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: `TmuxServer` subprocess queries for the chooser (tmux_control)

**Files:**
- Modify: `crates/tmux_control/src/transport.rs`

- [ ] **Step 1: Write the failing test (argv builders)**

In `crates/tmux_control/src/transport.rs`, inside the existing `#[cfg(test)] mod tests { ... }` block, add:

```rust
    #[test]
    fn list_windows_all_argv_targets_all_with_format() {
        let server = TmuxServer::new().socket_name("sock");
        let argv = server.list_windows_all_argv();
        assert_eq!(argv[0..2], ["-L".to_string(), "sock".to_string()]);
        assert!(argv.contains(&"list-windows".to_string()));
        assert!(argv.contains(&"-a".to_string()));
        assert!(argv.iter().any(|a| a.contains("#{session_id}")));
    }

    #[test]
    fn create_detached_session_argv_is_detached_with_name_format() {
        let server = TmuxServer::new();
        let argv = server.create_detached_session_argv();
        assert_eq!(argv[0], "new-session");
        assert!(argv.contains(&"-d".to_string()));
        assert!(argv.contains(&"-P".to_string()));
        assert!(argv.contains(&"-F".to_string()));
        assert!(argv.contains(&"#{session_name}".to_string()));
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p tmux_control list_windows_all_argv`
Expected: FAIL (method not found).

- [ ] **Step 3: Implement the queries**

In `crates/tmux_control/src/transport.rs`, add `use crate::window_list::{LIST_ALL_FORMAT, WindowEntry};` to the file's top import block. Add these methods inside `impl TmuxServer` (after `list_sessions_argv`, keeping `pub` items before the private `socket_args`/`connect_argv`):

```rust
    /// Lists every window across all sessions (`tmux [..] list-windows -a -F ..`,
    /// plain pipe, no control mode). Returns `Ok(vec![])` when no server is
    /// running. Used to build the session-chooser tree against the same socket
    /// whether or not a control client is attached.
    pub fn list_windows_all(&self) -> TmuxResult<Vec<WindowEntry>> {
        let output = std::process::Command::new(&self.program)
            .args(self.list_windows_all_argv())
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(TmuxError::Spawn)?;
        if output.status.success() {
            return WindowEntry::parse_list(&output.stdout);
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        if output.stdout.is_empty() && stderr.contains("no server running") {
            return Ok(Vec::new());
        }
        let message = stderr.trim();
        let message = if message.is_empty() {
            "tmux list-windows failed"
        } else {
            message
        };
        Err(TmuxError::Spawn(std::io::Error::other(message.to_string())))
    }

    /// The argv (after the program) for the `list-windows -a` query.
    pub fn list_windows_all_argv(&self) -> Vec<String> {
        let mut argv = self.socket_args();
        argv.push("list-windows".to_string());
        argv.push("-a".to_string());
        argv.push("-F".to_string());
        argv.push(LIST_ALL_FORMAT.to_string());
        argv
    }

    /// Creates a new detached session (`tmux new-session -d -P -F ..`) and
    /// returns its name, without attaching. Used by the chooser's "New session"
    /// entry while a control client is already attached: create here, then
    /// `switch-client` the live client to the returned name.
    pub fn create_detached_session(&self) -> TmuxResult<String> {
        let output = std::process::Command::new(&self.program)
            .args(self.create_detached_session_argv())
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(TmuxError::Spawn)?;
        if !output.status.success() {
            let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let message = if message.is_empty() {
                "tmux new-session failed".to_string()
            } else {
                message
            };
            return Err(TmuxError::Spawn(std::io::Error::other(message)));
        }
        let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if name.is_empty() {
            return Err(TmuxError::Spawn(std::io::Error::other(
                "tmux new-session returned no name".to_string(),
            )));
        }
        Ok(name)
    }

    /// The argv (after the program) for `create_detached_session`.
    pub fn create_detached_session_argv(&self) -> Vec<String> {
        let mut argv = self.socket_args();
        argv.push("new-session".to_string());
        argv.push("-d".to_string());
        argv.push("-P".to_string());
        argv.push("-F".to_string());
        argv.push("#{session_name}".to_string());
        argv
    }
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p tmux_control list_windows_all_argv create_detached_session_argv`
Expected: 2 tests PASS.

- [ ] **Step 5: Lint + commit**

```bash
cargo clippy -p tmux_control --all-targets && cargo fmt
git add crates/tmux_control/src/transport.rs
git commit -m "feat(tmux_control): list-windows-all + create-detached-session queries

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: `switch_client_command` builder (tmux_session)

**Files:**
- Modify: `crates/tmux_session/src/enumerate.rs`
- Modify: `crates/tmux_session/src/lib.rs`

- [ ] **Step 1: Write the failing test**

In `crates/tmux_session/src/enumerate.rs`, inside its `#[cfg(test)] mod tests`, add:

```rust
    #[test]
    fn switch_client_command_targets_quoted_name() {
        assert_eq!(switch_client_command("main"), "switch-client -t main");
        assert_eq!(
            switch_client_command("my work"),
            "switch-client -t 'my work'"
        );
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p ozmux_tmux switch_client_command`
Expected: FAIL (function not found).

- [ ] **Step 3: Implement**

In `crates/tmux_session/src/enumerate.rs`, add beside `select_window_command` (`quote` is already imported at the top of the file):

```rust
/// Builds `switch-client -t <name>` to repoint the attached control client at
/// another session. The resulting `%session-changed` / `%client-session-changed`
/// drives the projection rebuild; ozmux never mutates it optimistically.
pub fn switch_client_command(name: &str) -> String {
    format!("switch-client -t {}", quote(name))
}
```

In `crates/tmux_session/src/lib.rs`, add `switch_client_command` to the `pub use enumerate::{ ... };` re-export list.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p ozmux_tmux switch_client_command`
Expected: PASS.

- [ ] **Step 5: Lint + commit**

```bash
cargo clippy -p ozmux_tmux --all-targets && cargo fmt
git add crates/tmux_session/src/enumerate.rs crates/tmux_session/src/lib.rs
git commit -m "feat(tmux_session): switch-client command builder

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Keybind mirror parser + types (tmux_session)

**Files:**
- Create: `crates/tmux_session/src/keybinds.rs`
- Modify: `crates/tmux_session/src/lib.rs` (declare `mod keybinds;` — no re-export)

NOTE: Phase 4 uses the universal `list-keys` line parser (works on every tmux
version). The `list-keys -F` fast path on tmux ≥ 3.7 is a deferred optimization
recorded in the spec; do not implement it here.

- [ ] **Step 1: Write the failing test (new file with parser + tests)**

Create `crates/tmux_session/src/keybinds.rs`:

```rust
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
        let got = parse_key_bindings(&lines(&[
            "bind-key -r -T prefix Left resize-pane -L 5",
        ]));
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
```

In `crates/tmux_session/src/lib.rs`, add `mod keybinds;` to the module list. Do NOT re-export its items.

- [ ] **Step 2: Run to verify it compiles + passes**

Run: `cargo test -p ozmux_tmux keybinds`
Expected: 5 tests PASS.

(If clippy warns the module is unused until Task 5 wires it, that resolves in Task 5; the tests still run now.)

- [ ] **Step 3: Lint + commit**

```bash
cargo clippy -p ozmux_tmux --all-targets && cargo fmt
git add crates/tmux_session/src/keybinds.rs crates/tmux_session/src/lib.rs
git commit -m "feat(tmux_session): parse list-keys bind-key lines into a mirror

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Wire the keybind mirror into the drain (tmux_session)

**Files:**
- Modify: `crates/tmux_session/src/enumerate.rs` (`list_keys_command`, `list_keys_pending`)
- Modify: `crates/tmux_session/src/event_pump.rs` (`take_key_bindings`)
- Modify: `crates/tmux_session/src/plugin.rs` (register resource; send on attach; store reply)

- [ ] **Step 1: Add `list_keys_command` + the pending field with tests**

In `crates/tmux_session/src/enumerate.rs`, add the builder beside the others:

```rust
/// Builds `list-keys` to enumerate tmux's key tables for the (cosmetic) mirror.
///
/// No `-F`: the custom format was only added in tmux 3.7; the reply is the
/// default human-readable `bind-key` lines, parsed by `parse_key_bindings`.
pub(crate) fn list_keys_command() -> String {
    "list-keys".to_string()
}
```

Add the field to `EnumerationState` (after `active_pane_pending`):

```rust
    /// The id of the in-flight `list-keys` mirror query, if any.
    pub(crate) list_keys_pending: Option<CommandId>,
```

In the same test module, add:

```rust
    #[test]
    fn list_keys_command_has_no_format_flag() {
        assert_eq!(list_keys_command(), "list-keys");
    }
```

- [ ] **Step 2: Add `take_key_bindings` with a test**

In `crates/tmux_session/src/event_pump.rs`, add to the top import block:

```rust
use crate::keybinds::{KeyBinding, parse_key_bindings};
```

Add the helper (beside `take_client_name`, keeping `pub(crate)` items grouped):

```rust
/// Returns the parsed key bindings from a `CommandComplete` whose id matches
/// `pending` (the `list-keys` reply), clearing `pending`. Returns `None` when no
/// matching reply is in the batch.
pub(crate) fn take_key_bindings(
    pending: &mut Option<CommandId>,
    events: &[TransportEvent],
) -> Option<Vec<KeyBinding>> {
    for event in events {
        if let TransportEvent::Protocol(ClientEvent::CommandComplete { id, ok, output, .. }) = event
            && *pending == Some(*id)
        {
            *pending = None;
            if *ok {
                return Some(parse_key_bindings(output));
            }
            tracing::warn!("list-keys mirror query failed");
            return None;
        }
    }
    None
}
```

In the `event_pump.rs` test module, add:

```rust
    #[test]
    fn take_key_bindings_parses_matching_reply() {
        let events = vec![TransportEvent::Protocol(ClientEvent::CommandComplete {
            id: CommandId(4),
            number: 0,
            ok: true,
            output: vec!["bind-key -T prefix c new-window".to_string()],
        })];
        let mut pending = Some(CommandId(4));
        let got = take_key_bindings(&mut pending, &events).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(pending, None);
    }
```

- [ ] **Step 3: Register resource, send on attach, store reply (plugin.rs)**

In `crates/tmux_session/src/plugin.rs`:

Add to the top import block:
```rust
use crate::event_pump::take_key_bindings;
use crate::keybinds::TmuxKeyBindings;
```
and extend the existing `use crate::enumerate::{ ... }` to include `list_keys_command`.

In `impl Plugin for TmuxSessionPlugin`, add the resource init (beside `init_resource::<EnumerationState>()`):
```rust
            .init_resource::<TmuxKeyBindings>()
```

Inside `drain_tmux_events`, in the `Attached`-transition block (right after the `active_pane_command()` send), add:
```rust
            match client.handle().send(&list_keys_command()) {
                Ok(id) => enumeration.list_keys_pending = Some(id),
                Err(error) => tracing::warn!(?error, "failed to send list-keys mirror query"),
            }
```

Add `mut key_bindings: ResMut<TmuxKeyBindings>` to `drain_tmux_events`'s parameters (mutable params come before the immutable ones; place it among the other `ResMut`/`NonSendMut` params). In the non-`Closed` `else` branch (beside the other `take_*` calls), add:
```rust
        if let Some(bindings) = take_key_bindings(&mut enumeration.list_keys_pending, &events) {
            key_bindings.bindings = bindings;
        }
```

In the `Closed` branch, clear the pending id beside the others:
```rust
        enumeration.list_keys_pending = None;
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p ozmux_tmux list_keys take_key_bindings`
Expected: PASS. Then `cargo test -p ozmux_tmux` — all crate tests PASS.

- [ ] **Step 5: Lint + commit**

```bash
cargo clippy -p ozmux_tmux --all-targets && cargo fmt
git add crates/tmux_session/src/enumerate.rs crates/tmux_session/src/event_pump.rs crates/tmux_session/src/plugin.rs
git commit -m "feat(tmux_session): query and store the tmux keybind mirror on attach

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: Map `%client-session-changed` to the session-changed projection (tmux_session)

**Files:**
- Modify: `crates/tmux_session/src/event_pump.rs` (`trigger_notification`)

- [ ] **Step 1: Write the failing test**

In `crates/tmux_session/src/event_pump.rs` test module, add:

```rust
    #[test]
    fn client_session_changed_triggers_session_changed() {
        use crate::events::TmuxSessionChanged;
        use std::sync::{Arc, Mutex};
        use tmux_control_parser::SessionId;

        #[derive(Resource, Default, Clone)]
        struct Seen(Arc<Mutex<Vec<(u32, String)>>>);

        #[derive(Resource)]
        struct Batch(Vec<TransportEvent>);

        fn run(mut commands: Commands, mut pending: ResMut<EnumerationState>, batch: Res<Batch>) {
            trigger_events(&mut commands, &mut pending.pending, &batch.0);
        }

        let mut app = App::new();
        app.init_resource::<Seen>();
        app.init_resource::<EnumerationState>();
        app.insert_resource(Batch(vec![TransportEvent::Protocol(
            ClientEvent::Notification(ControlEvent::ClientSessionChanged {
                client: "main".to_string(),
                session: SessionId(9),
                name: "beta".to_string(),
            }),
        )]));
        app.add_observer(|ev: On<TmuxSessionChanged>, seen: Res<Seen>| {
            seen.0.lock().unwrap().push((ev.session.0, ev.name.clone()));
        });
        app.add_systems(Update, run);
        let seen = app.world().resource::<Seen>().clone();
        app.update();

        assert_eq!(*seen.0.lock().unwrap(), vec![(9, "beta".to_string())]);
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p ozmux_tmux client_session_changed_triggers`
Expected: FAIL (no `TmuxSessionChanged` fired).

- [ ] **Step 3: Implement**

In `crates/tmux_session/src/event_pump.rs`, in `trigger_notification`, add a match arm beside the `SessionChanged` arm:

```rust
        ControlEvent::ClientSessionChanged { session, name, .. } => {
            commands.trigger(TmuxSessionChanged {
                session: *session,
                name: name.clone(),
            });
        }
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p ozmux_tmux client_session_changed_triggers`
Expected: PASS.

- [ ] **Step 5: Lint + commit**

```bash
cargo clippy -p ozmux_tmux --all-targets && cargo fmt
git add crates/tmux_session/src/event_pump.rs
git commit -m "feat(tmux_session): treat %client-session-changed as a session change

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: Session-switch detection → teardown + re-enumerate (tmux_session)

**Files:**
- Modify: `crates/tmux_session/src/event_pump.rs` (`detect_session_switch` helper)
- Modify: `crates/tmux_session/src/plugin.rs` (`drain_tmux_events` wiring)

- [ ] **Step 1: Write the failing test (pure helper)**

In `crates/tmux_session/src/event_pump.rs` test module, add:

```rust
    #[test]
    fn detect_session_switch_reports_new_id_only_on_change() {
        use tmux_control_parser::SessionId;
        let changed = vec![TransportEvent::Protocol(ClientEvent::Notification(
            ControlEvent::SessionChanged {
                session: SessionId(2),
                name: "b".to_string(),
            },
        ))];
        // No prior session → first attach, not a switch.
        assert_eq!(detect_session_switch(&changed, None), None);
        // Same id → not a switch.
        assert_eq!(detect_session_switch(&changed, Some(SessionId(2))), None);
        // Different id → switch to the new session.
        assert_eq!(
            detect_session_switch(&changed, Some(SessionId(1))),
            Some(SessionId(2))
        );
        // No session-changed event → no switch.
        assert_eq!(detect_session_switch(&[], Some(SessionId(1))), None);
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p ozmux_tmux detect_session_switch`
Expected: FAIL (function not found).

- [ ] **Step 3: Implement the helper**

In `crates/tmux_session/src/event_pump.rs`, add `SessionId` to the `tmux_control_parser` import at the top:
```rust
use tmux_control_parser::{PaneId, SessionId, WindowId};
```
Add the helper (a `pub(crate)` fn, placed before the private `fn`s):

```rust
/// Returns the new session id if `events` contains a session-change to an id
/// different from `current`, i.e. a real `switch-client`. Returns `None` on the
/// first attach (`current == None`) or when the id is unchanged, so the initial
/// enumeration is not duplicated and only an actual switch triggers a rebuild.
pub(crate) fn detect_session_switch(
    events: &[TransportEvent],
    current: Option<SessionId>,
) -> Option<SessionId> {
    let current = current?;
    for event in events {
        let next = match event {
            TransportEvent::Protocol(ClientEvent::Notification(
                ControlEvent::SessionChanged { session, .. }
                | ControlEvent::ClientSessionChanged { session, .. },
            )) => *session,
            _ => continue,
        };
        if next != current {
            return Some(next);
        }
    }
    None
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p ozmux_tmux detect_session_switch`
Expected: PASS.

- [ ] **Step 5: Wire into `drain_tmux_events` (plugin.rs)**

In `crates/tmux_session/src/plugin.rs`:

Extend imports:
```rust
use crate::components::{TmuxPane, TmuxSession};
use crate::event_pump::{
    advance_state, detect_session_switch, drain_transport, take_active_pane, take_client_name,
    take_key_bindings, take_pane_captures, trigger_events,
};
use crate::events::{TmuxActivePaneChanged, TmuxConnectionReset, TmuxWindowsRetained};
```
and extend the `use crate::enumerate::{ ... }` to include `active_pane_command, list_windows_command` (already present) — ensure `active_pane_command` and `list_windows_command` remain imported.

Add a read-only query parameter to `drain_tmux_events` (immutable, so after the mutable params):
```rust
    index: Res<TmuxProjection>,
    sessions: Query<&TmuxSession>,
```
Add `use crate::observers::TmuxProjection;` if not already imported (it is, via `register_observers`/`TmuxProjection`).

Immediately after the `if events.is_empty() { return; }` guard and the `collect_pane_outputs` loop, before the `advance_state` block, add:

```rust
    let current_session = index
        .session
        .and_then(|e| sessions.get(e).ok())
        .map(|s| s.id);
    if let Some(_new) = detect_session_switch(&events, current_session)
        && let Some(client) = connection.client()
    {
        commands.trigger(TmuxWindowsRetained {
            windows: Vec::new(),
        });
        send_session_enumeration(client, &mut enumeration);
    }
```

Add a private helper at the bottom of `plugin.rs` (private items last), and refactor the existing on-attach sends to call it too:

```rust
// NOTE: re-enumerating on a session switch must use the SAME query set as the
// attach transition, so factor it; drift here would leave a switched-to session
// with stale windows or no active-pane marker.
fn send_session_enumeration(client: &tmux_control::TmuxClient, enumeration: &mut EnumerationState) {
    match client.handle().send(&list_windows_command()) {
        Ok(id) => enumeration.pending = Some(id),
        Err(error) => tracing::warn!(?error, "failed to send list-windows enumeration"),
    }
    match client.handle().send(&active_pane_command()) {
        Ok(id) => enumeration.active_pane_pending = Some(id),
        Err(error) => tracing::warn!(?error, "failed to send active-pane query"),
    }
}
```

In the `Attached`-transition block, replace the inline `list-windows` + `active-pane` sends with `send_session_enumeration(client, &mut enumeration);` (keep the separate `client_name_command()` and `list_keys_command()` sends inline — those run only on first attach, not on every switch).

NOTE: `tracing` is already used in this file. `TmuxClient` is re-exported from `tmux_control`.

- [ ] **Step 6: Add a headless teardown test**

In `crates/tmux_session/src/plugin.rs` test module, add a test that the teardown event clears the projection windows/panes while keeping the session (exercises the reuse decision):

```rust
    #[test]
    fn empty_windows_retained_clears_windows_and_panes_keeps_session() {
        use crate::events::{TmuxLayoutChanged, TmuxSessionChanged, TmuxWindowAdded, TmuxWindowsRetained};
        use crate::events::pane_geoms;
        use tmux_control_parser::{SessionId, WindowId, WindowLayout};

        let mut app = App::new();
        app.add_plugins(TmuxSessionPlugin);
        app.world_mut().trigger(TmuxSessionChanged {
            session: SessionId(1),
            name: "a".into(),
        });
        app.world_mut().trigger(TmuxWindowAdded {
            window: WindowId(1),
            index: 0,
            name: "w".into(),
        });
        app.world_mut().trigger(TmuxLayoutChanged {
            window: WindowId(1),
            panes: pane_geoms(&WindowLayout::parse(b"abcd,80x24,0,0,1").unwrap()),
        });
        app.update();
        app.world_mut().trigger(TmuxWindowsRetained { windows: Vec::new() });
        app.update();

        let index = app.world().resource::<TmuxProjection>();
        assert!(index.windows.is_empty());
        assert!(index.panes.is_empty());
        assert!(index.session.is_some());
    }
```

- [ ] **Step 7: Run tests**

Run: `cargo test -p ozmux_tmux`
Expected: all PASS (including `empty_windows_retained_clears_windows_and_panes_keeps_session` and `detect_session_switch`).

- [ ] **Step 8: Lint + commit**

```bash
cargo clippy -p ozmux_tmux --all-targets && cargo fmt
git add crates/tmux_session/src/event_pump.rs crates/tmux_session/src/plugin.rs
git commit -m "feat(tmux_session): rebuild projection on a session switch

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 8: Chooser row model + selection logic (binary)

**Files:**
- Modify: `src/tmux_picker.rs`

- [ ] **Step 1: Write the failing tests (pure row model)**

In `src/tmux_picker.rs` test module (`#[cfg(test)] mod tests`), add at the top of the module `use tmux_control::WindowEntry;` and these tests:

```rust
    fn fake_window(session: u32, sname: &str, wid: u32, active: bool, wname: &str) -> WindowEntry {
        WindowEntry {
            session_id: tmux_control::SessionId(session),
            session_name: sname.to_string(),
            window_id: tmux_control::WindowId(wid),
            window_index: 0,
            window_active: active,
            window_name: wname.to_string(),
        }
    }

    #[test]
    fn build_rows_nests_windows_under_sessions_then_new_session() {
        let sessions = vec![fake_session(0, "alpha"), fake_session(1, "beta")];
        let windows = vec![
            fake_window(0, "alpha", 0, true, "zsh"),
            fake_window(0, "alpha", 1, false, "editor"),
            fake_window(1, "beta", 2, true, "top"),
        ];
        let rows = build_rows(&sessions, &windows);
        assert_eq!(
            rows,
            vec![
                PickerRow::Session(0),
                PickerRow::Window { session: 0, window: 0 },
                PickerRow::Window { session: 0, window: 1 },
                PickerRow::Session(1),
                PickerRow::Window { session: 1, window: 2 },
                PickerRow::NewSession,
            ]
        );
    }

    #[test]
    fn build_rows_with_no_sessions_is_just_new_session() {
        assert_eq!(build_rows(&[], &[]), vec![PickerRow::NewSession]);
    }
```

(`fake_session` already exists in the test module.)

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p ozmux-gui build_rows`
Expected: FAIL (`PickerRow` / `build_rows` not found).

- [ ] **Step 3: Implement the model**

In `src/tmux_picker.rs`, add to the top import block `use tmux_control::WindowEntry;` and extend the `use ozmux_tmux::{ ... }` to also import `switch_client_command, select_window_command`.

Add the row type (above `SessionPicker`):

```rust
/// One selectable row in the chooser tree: a session header, a window under a
/// session (indices into the picker's `sessions` / `windows`), or the trailing
/// "New session" entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PickerRow {
    Session(usize),
    Window { session: usize, window: usize },
    NewSession,
}
```

Add `windows: Vec<WindowEntry>` to `SessionPicker` (after `sessions`):

```rust
    windows: Vec<WindowEntry>,
```

Add the pure builder (a private fn near `target_for`):

```rust
fn build_rows(sessions: &[SessionInfo], windows: &[WindowEntry]) -> Vec<PickerRow> {
    let mut rows = Vec::new();
    for (si, session) in sessions.iter().enumerate() {
        rows.push(PickerRow::Session(si));
        for (wi, window) in windows.iter().enumerate() {
            if window.session_id == session.id {
                rows.push(PickerRow::Window {
                    session: si,
                    window: wi,
                });
            }
        }
    }
    rows.push(PickerRow::NewSession);
    rows
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p ozmux-gui build_rows`
Expected: 2 tests PASS.

- [ ] **Step 5: Lint + commit**

```bash
cargo clippy -p ozmux-gui --all-targets && cargo fmt
git add src/tmux_picker.rs
git commit -m "feat: chooser row model nesting windows under sessions

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 9: Refresh the chooser on open + render the tree (binary)

**Files:**
- Modify: `src/tmux_picker.rs`

- [ ] **Step 1: Add an open-transition refresh system**

In `src/tmux_picker.rs`, register a system that refreshes sessions+windows when the picker opens. Add to `OzmuxTmuxPickerPlugin::build` (after the existing `handle_picker_input` registration):

```rust
            .add_systems(Update, refresh_picker_on_open)
```

Add a local change-tracker field to `SessionPicker`:

```rust
    last_open: bool,
```

Add the system (private fn). It runs every frame but only does work on the closed→open edge:

```rust
// NOTE: subprocess `list-*` against the same socket sees the live server whether
// or not a control client is attached, mirroring the boot path — so the chooser
// needs no control-mode reply correlation.
fn refresh_picker_on_open(mut picker: ResMut<SessionPicker>, configs: Res<OzmuxConfigsResource>) {
    let opened = picker.open && !picker.last_open;
    picker.last_open = picker.open;
    if !opened {
        return;
    }
    let server = build_server(&configs);
    match (server.list_sessions(), server.list_windows_all()) {
        (Ok(sessions), Ok(windows)) => {
            picker.sessions = sessions;
            picker.windows = windows;
            picker.selected = 0;
        }
        (Err(e), _) | (_, Err(e)) => {
            tracing::warn!(?e, "failed to refresh session chooser");
        }
    }
}
```

- [ ] **Step 2: Render the tree (replace `sync_picker_ui` body)**

Replace the entry-building loop in `sync_picker_ui` so it walks `build_rows`. Replace the block from `let entry_count = ...` through the closing of the `with_children` closure with:

```rust
    let rows = build_rows(&picker.sessions, &picker.windows);
    let selected = picker.selected.min(rows.len().saturating_sub(1));
    let mut child_commands = commands.entity(list_entity);
    child_commands.with_children(|parent| {
        for (i, row) in rows.iter().enumerate() {
            let is_selected = i == selected;
            let prefix = if is_selected { "> " } else { "  " };
            let label = match row {
                PickerRow::Session(si) => {
                    let s = &picker.sessions[*si];
                    let attached = if s.attached { " *attached" } else { "" };
                    format!("{}{}  ({} windows){}", prefix, s.name, s.windows, attached)
                }
                PickerRow::Window { window, .. } => {
                    let w = &picker.windows[*window];
                    let active = if w.window_active { "*" } else { " " };
                    format!("{}    {}{}: {}", prefix, active, w.window_index, w.window_name)
                }
                PickerRow::NewSession => format!("{}+ New session", prefix),
            };
            let color = if is_selected {
                TextColor(Color::WHITE)
            } else {
                TextColor(Color::srgba(0.6, 0.6, 0.6, 1.0))
            };
            parent.spawn((Text::new(label), color));
        }
    });
```

Update `handle_picker_input`'s navigation to use the row count. Replace `let entry_count = picker.sessions.len() + 1;` with:

```rust
    let entry_count = build_rows(&picker.sessions, &picker.windows).len();
```

- [ ] **Step 3: Build to verify it compiles**

Run: `cargo build -p ozmux-gui`
Expected: compiles. (Rendering is validated at runtime / in Task 13's manual check.)

- [ ] **Step 4: Run crate tests**

Run: `cargo test -p ozmux-gui tmux_picker`
Expected: existing nav tests still PASS.

- [ ] **Step 5: Lint + commit**

```bash
cargo clippy -p ozmux-gui --all-targets && cargo fmt
git add src/tmux_picker.rs
git commit -m "feat: refresh chooser on open and render the session/window tree

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 10: Selection — switch while attached, attach while disconnected (binary)

**Files:**
- Modify: `src/tmux_picker.rs`

- [ ] **Step 1: Implement the selection branch in `handle_picker_input`**

In `src/tmux_picker.rs`, replace the `KeyCode::Enter => { ... }` arm body in `handle_picker_input` with a branch on the selected `PickerRow` and connection state. The new arm:

```rust
            KeyCode::Enter => {
                let rows = build_rows(&picker.sessions, &picker.windows);
                let row = rows.get(picker.selected).copied().unwrap_or(PickerRow::NewSession);
                if connection.client().is_some() {
                    apply_switch(&mut connection, &mut state, &configs, &picker, row);
                } else {
                    apply_attach(&mut connection, &mut state, &configs, control.as_deref(), &picker, row);
                }
                picker.open = false;
                break;
            }
```

Add the two helpers (private fns). `apply_switch` uses the live client; `apply_attach` is the existing boot/reconnect path factored out:

```rust
// NOTE: while attached, switching must go through the live control client so the
// single `tmux -CC` connection survives; a fresh `attach_or_create` would spawn a
// second client and orphan the first.
fn apply_switch(
    connection: &mut TmuxConnection,
    state: &mut ConnectionState,
    configs: &OzmuxConfigsResource,
    picker: &SessionPicker,
    row: PickerRow,
) {
    let Some(client) = connection.client() else {
        return;
    };
    let cmds: Vec<String> = match row {
        PickerRow::Session(si) => vec![switch_client_command(&picker.sessions[si].name)],
        PickerRow::Window { session, window } => vec![
            switch_client_command(&picker.sessions[session].name),
            select_window_command(picker.windows[window].window_id),
        ],
        PickerRow::NewSession => {
            let server = build_server(configs);
            match server.create_detached_session() {
                Ok(name) => vec![switch_client_command(&name)],
                Err(e) => {
                    tracing::warn!(?e, "failed to create new session");
                    return;
                }
            }
        }
    };
    for cmd in &cmds {
        if let Err(e) = client.handle().send(cmd) {
            tracing::warn!(?e, cmd, "switch command send failed");
            *state = ConnectionState::Error {
                reason: format!("switch failed: {e}"),
            };
            return;
        }
    }
}

fn apply_attach(
    connection: &mut TmuxConnection,
    state: &mut ConnectionState,
    configs: &OzmuxConfigsResource,
    control: Option<&ControlPlaneHandle>,
    picker: &SessionPicker,
    row: PickerRow,
) {
    let target = match row {
        PickerRow::Session(si) => ozmux_tmux::AttachTarget::Attach(picker.sessions[si].name.clone()),
        PickerRow::Window { session, .. } => {
            ozmux_tmux::AttachTarget::Attach(picker.sessions[session].name.clone())
        }
        PickerRow::NewSession => ozmux_tmux::AttachTarget::CreateNew,
    };
    let mut server = build_server(configs);
    if let Some(handle) = control {
        server = server.env("OZMUX_SOCK", &handle.sock_path.to_string_lossy());
    }
    match attach_or_create(&server, &target) {
        Ok(client) => {
            connection.set(client);
            *state = ConnectionState::Connecting;
        }
        Err(e) => {
            *state = ConnectionState::Error {
                reason: format!("tmux connect failed: {e}"),
            };
        }
    }
}
```

Remove the now-unused `target_for` function if nothing else references it (the new `apply_attach` replaces it). Confirm with `cargo build`; if `target_for`'s tests fail to compile, delete those tests too (they are superseded by `build_rows` tests).

NOTE: `state` and `connection` params are `&mut ConnectionState` / `&mut TmuxConnection`; pass `&mut state`/`&mut connection` from the `ResMut`/`NonSendMut` in `handle_picker_input` (deref-coerce via `&mut *state`).

- [ ] **Step 2: Build + run tests**

Run: `cargo build -p ozmux-gui && cargo test -p ozmux-gui tmux_picker`
Expected: compiles; `build_rows` tests PASS.

- [ ] **Step 3: Lint + commit**

```bash
cargo clippy -p ozmux-gui --all-targets && cargo fmt
git add src/tmux_picker.rs
git commit -m "feat: switch via switch-client while attached, attach while disconnected

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 11: Detached/error overlay text in `tmux_dialog` (binary)

**Files:**
- Modify: `src/ui/tmux_dialog.rs`

- [ ] **Step 1: Update the test to cover `Detached`**

In `src/ui/tmux_dialog.rs` test module, extend `dialog_shows_only_on_error_state` (rename to `dialog_shows_on_error_and_detached`) — add, before the final `Attached` assertion:

```rust
        app.insert_resource(ConnectionState::Detached);
        app.update();
        assert_eq!(backdrop_display(&mut app), Display::Flex);
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p ozmux-gui dialog_shows`
Expected: FAIL (Detached currently hides the overlay).

- [ ] **Step 3: Implement state-specific text**

In `src/ui/tmux_dialog.rs`, replace the `match &*state { ... }` in `sync_tmux_dialog` with:

```rust
    match &*state {
        ConnectionState::Error { reason } => {
            node.display = Display::Flex;
            if let Ok(mut label) = text.single_mut() {
                label.0 = format!("tmux unavailable\n{reason}");
            }
        }
        ConnectionState::Detached => {
            node.display = Display::Flex;
            if let Ok(mut label) = text.single_mut() {
                label.0 = "Disconnected — press \u{2318}\u{21e7}P to choose a session".to_string();
            }
        }
        _ => node.display = Display::None,
    }
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p ozmux-gui dialog_shows`
Expected: PASS.

- [ ] **Step 5: Lint + commit**

```bash
cargo clippy -p ozmux-gui --all-targets && cargo fmt
git add src/ui/tmux_dialog.rs
git commit -m "feat: show a reconnect hint overlay when the tmux session detaches

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 12: Full workspace verification (no new code)

**Files:** none

- [ ] **Step 1: Build the whole workspace**

Run: `cargo build`
Expected: success, no errors.

- [ ] **Step 2: Run all tests**

Run: `cargo test`
Expected: all PASS.

- [ ] **Step 3: Workspace lint + format**

Run: `cargo clippy --workspace --all-targets && cargo fmt --check`
Expected: no warnings; formatting clean.

- [ ] **Step 4: TypeScript untouched — confirm no JS changed**

Run: `git status --porcelain -- sdk extensions`
Expected: empty (Phase 4 touches no TS).

- [ ] **Step 5: Commit (only if fmt changed anything)**

```bash
git add -A
git commit -m "chore: workspace fmt/clippy after phase 4 session UX

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>" || echo "nothing to commit"
```

---

## Task 13: Gated real-tmux integration tests (tmux_session)

**Files:**
- Create: `crates/tmux_session/tests/real_tmux_switch.rs`

- [ ] **Step 1: Write the integration tests**

Create `crates/tmux_session/tests/real_tmux_switch.rs`:

```rust
//! Gated end-to-end tests for the Phase 4 session-switch + chooser-query path
//! against a real tmux. Run with:
//! `cargo test -p ozmux_tmux --test real_tmux_switch -- --ignored`.

use std::time::Duration;
use tmux_control::{TmuxServer, WindowEntry};

fn unique_socket(tag: &str) -> String {
    format!("ozmux-phase4-{tag}-{}", std::process::id())
}

#[test]
#[ignore = "requires a real tmux binary"]
fn list_windows_all_spans_sessions() {
    let socket = unique_socket("lw");
    let server = TmuxServer::new().socket_name(&socket);
    // Create two detached sessions with windows.
    let a = server.create_detached_session().expect("create a");
    let b = server.create_detached_session().expect("create b");
    std::thread::sleep(Duration::from_millis(200));

    let windows: Vec<WindowEntry> = server.list_windows_all().expect("list-windows -a");
    let names: Vec<&str> = windows.iter().map(|w| w.session_name.as_str()).collect();
    assert!(names.contains(&a.as_str()), "session a present: {names:?}");
    assert!(names.contains(&b.as_str()), "session b present: {names:?}");

    server
        .attach(&a)
        .map(|c| c.handle().send("kill-server").ok())
        .ok();
}

#[test]
#[ignore = "requires a real tmux binary and a controlling PTY"]
fn switch_client_emits_a_session_change() {
    let socket = unique_socket("sw");
    let server = TmuxServer::new().socket_name(&socket);
    let a = server.create_detached_session().expect("create a");
    let b = server.create_detached_session().expect("create b");

    let client = server.attach(&a).expect("attach a");
    std::thread::sleep(Duration::from_millis(400));
    // Drain the attach burst.
    while client.events().try_recv().is_ok() {}

    client
        .handle()
        .send(&ozmux_tmux::switch_client_command(&b))
        .expect("switch-client");
    std::thread::sleep(Duration::from_millis(400));

    let mut saw_session_change = false;
    while let Ok(ev) = client.events().try_recv() {
        if let tmux_control::TransportEvent::Protocol(
            tmux_control::ClientEvent::Notification(n),
        ) = &ev
        {
            let s = format!("{n:?}");
            if s.contains("SessionChanged") || s.contains("ClientSessionChanged") {
                saw_session_change = true;
            }
        }
    }
    assert!(
        saw_session_change,
        "switch-client should emit a (client-)session-changed notification"
    );

    client.handle().send("kill-server").ok();
}
```

- [ ] **Step 2: Run the gated tests locally (requires tmux)**

Run: `cargo test -p ozmux_tmux --test real_tmux_switch -- --ignored`
Expected: both PASS. This confirms the two spec-flagged unknowns: which notification `switch-client` emits in `-CC`, and that `list-windows -a` / `create-detached-session` behave as designed.

If `switch_client_emits_a_session_change` fails or shows `%session-changed` vs `%client-session-changed` differently than expected, both are already handled by `detect_session_switch` (Task 7); record the observed notification in the test comment.

- [ ] **Step 3: Commit**

```bash
git add crates/tmux_session/tests/real_tmux_switch.rs
git commit -m "test(tmux_session): gated real-tmux switch + chooser-query coverage

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Manual verification (after all tasks)

Run the app against a real tmux server with multiple sessions:

```bash
tmux new-session -d -s alpha; tmux new-session -d -s beta
cargo run
```

- Press `⌘⇧P` → the chooser shows `alpha` / `beta` with their windows nested, plus "+ New session".
- Arrow to `beta`, press Enter → panes rebuild to beta's layout (the live connection stays; no re-attach flicker).
- Select a window row under a session → switches session and selects that window.
- Select "+ New session" while attached → a new session is created and switched to.
- `tmux kill-server` from another terminal → the "Disconnected — press ⌘⇧P…" overlay appears; pressing `⌘⇧P` and selecting a session reconnects.

---

## Self-Review notes

- **Spec coverage:** chooser tree (Tasks 8–10), refresh-on-open (Task 9), switch-client + select-window + new-session-while-attached (Task 10), session-change reconciliation incl. `%client-session-changed` (Tasks 6–7), detached/error overlay reuse (Task 11), keybind mirror parse+resource, no UI, `pub(crate)` (Tasks 4–5), subprocess chooser queries (Tasks 1–2), gated integration (Task 13). The `%exit` correctness note in the spec needs no code change (teardown stays on transport `Closed`); flagged for the integration phase.
- **Deferred (per spec):** `list-keys -F` fast path on tmux ≥ 3.7; keybind display UI; re-sync on `%config-error`; copy-mode and other window modes; tree collapse/mouse; multi-window tab strip.
- **Type consistency:** `PickerRow`, `build_rows`, `WindowEntry`, `switch_client_command`, `select_window_command`, `detect_session_switch`, `send_session_enumeration`, `TmuxKeyBindings`, `parse_key_bindings`, `take_key_bindings`, `list_keys_pending` are defined once and used with the same signatures across tasks.
