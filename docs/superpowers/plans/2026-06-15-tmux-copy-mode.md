# tmux-driven Copy Mode Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make tmux's copy mode usable under the `tmux -CC` control-mode backend, with every copy-mode keybinding sourced from the user's `tmux.conf`.

**Architecture:** tmux owns copy mode; ozmux drives it (relays the user's copy-table bindings verbatim over the control connection) and mirrors it for display (rebuilds the pane's `TerminalGrid` from `capture-pane` snapshots and overlays the cursor/selection from format variables). `%output` keeps streaming into the live pane handle the whole time, so exit just switches the renderer back to the live grid.

**Tech Stack:** Rust (edition 2024), Bevy 0.18 ECS, `crates/tmux_session` (`ozmux_tmux`), `crates/ozma_tty_engine` (alacritty VT), `crates/ozma_tty_renderer` (`TerminalGrid`), tmux 3.6a control mode.

**Spec:** `docs/superpowers/specs/2026-06-15-tmux-copy-mode-design.md` — read it before starting.

**Repo conventions (`.claude/rules/rust.md`):** no `mod.rs`; comments only `// TODO:` / `// NOTE:` / `// SAFETY:` (NOTE = critical caveat only); doc-comment every `pub` item; all `use` at file top in one block; visibility minimized (private unless a cross-module caller exists); mutable params before immutable; whole-system change guards via `run_if`, not in-body early return; mutate conditionally (no manual `set_changed`). Run `cargo fmt` + `cargo clippy --workspace` after each task.

---

## File Structure

**Modify (crate `crates/tmux_session/`):**
- `src/keybindings.rs` — add `Table::{CopyMode,CopyModeVi}`, `ModeKeys`, copy tables on `KeyBindings`, `CopyAction` + `copy_mode_dispatch` (Tasks 1–2).
- `src/enumerate.rs` — copy-mode command builders + coordinate helpers + `EnumerationState` copy-table pending fields (Tasks 3–4).
- `src/event_pump.rs` — `take_mode_keys` reply helper (Task 4).
- `src/plugin.rs` — fetch copy tables + `mode-keys` on attach (Task 4).
- `src/lib.rs` — re-export the new public items (Tasks 1–4).

**Modify (crate `crates/tmux_control_parser/` or `crates/tmux_control/`):**
- `%pane-mode-changed` notification surfaced as a `ControlEvent` variant (Task 5).

**Modify (binary `src/`):**
- `src/tmux_input.rs` — copy-mode entry interception + in-copy-mode key branch (Tasks 6–7).
- `src/tmux_render.rs` — `route_tmux_output` advances always, emits only when not in copy mode (Task 8).
- `src/ui/copy_mode.rs` — `CopyModeState` stays the marker; add tmux insert/remove helpers (do NOT reuse the Coalescer-bound observers) (Task 6).
- `src/input/mouse_wheel.rs`, `src/input/mouse_buttons.rs` — route wheel/drag to the copy-mode relay; suppress alacritty scrollback while in copy mode (Tasks 12–13).
- `src/main.rs` — register the new plugin/systems (Tasks 6, 8, 11).

**Create (binary `src/`):**
- `src/tmux_copy_mode.rs` — `OzmuxTmuxCopyModePlugin`: the refresh system (display-message + capture → grid + overlay), clipboard bridge, exit detection, the `CommandId`→refresh transaction map, and the per-pane copy-render handle (Tasks 8–10).
- `src/ui/copy_search.rs` — the copy-mode prompt input overlay (search regex + jump single-char) (Task 11).

---

## Phase 1 — Pure dispatch & command core (no Bevy systems; fully unit-testable)

### Task 1: Copy-mode key tables in `KeyBindings`

**Files:**
- Modify: `crates/tmux_session/src/keybindings.rs`
- Modify: `crates/tmux_session/src/lib.rs` (re-export `ModeKeys` if a consumer needs it — defer until Task 6 proves it; keep private for now)

- [ ] **Step 1: Write the failing tests** (append inside the existing `#[cfg(test)] mod tests`)

```rust
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
        assert_eq!(got[0].table, Table::CopyMode);
    }

    #[test]
    fn copy_table_selects_vi_or_emacs_by_mode_keys() {
        let mut kb = KeyBindings::default();
        kb.install(vec![
            KeyBinding { table: Table::CopyModeVi, key: "j".into(), command: "vi-down".into(), repeat: false },
            KeyBinding { table: Table::CopyMode,   key: "j".into(), command: "emacs-down".into(), repeat: false },
        ]);
        kb.set_mode_keys(ModeKeys::Vi);
        assert_eq!(kb.copy_command("j"), Some("vi-down".to_string()));
        kb.set_mode_keys(ModeKeys::Emacs);
        assert_eq!(kb.copy_command("j"), Some("emacs-down".to_string()));
    }

    #[test]
    fn clear_drops_copy_tables_and_mode_keys() {
        let mut kb = KeyBindings::default();
        kb.install(vec![KeyBinding { table: Table::CopyModeVi, key: "j".into(), command: "x".into(), repeat: false }]);
        kb.set_mode_keys(ModeKeys::Vi);
        kb.clear();
        assert_eq!(kb.copy_command("j"), None);
    }
```

- [ ] **Step 2: Run, verify failure**

Run: `cargo test -p ozmux_tmux keybindings::tests::parses_copy_mode_vi_binding`
Expected: FAIL — `Table::CopyModeVi` does not exist.

- [ ] **Step 3: Implement**

In `Table` (currently `enum Table { Root, Prefix }`):

```rust
pub(crate) enum Table {
    Root,
    Prefix,
    CopyMode,
    CopyModeVi,
}
```

In `parse_binding_line`, extend the `-T` arm's match:

```rust
                table = match name {
                    "root" => Some(Table::Root),
                    "prefix" => Some(Table::Prefix),
                    "copy-mode" => Some(Table::CopyMode),
                    "copy-mode-vi" => Some(Table::CopyModeVi),
                    _ => return None,
                };
```

Add the mode selector and extend `KeyBindings`:

```rust
/// Which copy-mode key table is active, from tmux's `mode-keys` option.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ModeKeys {
    /// `mode-keys vi` → the `copy-mode-vi` table.
    Vi,
    /// `mode-keys emacs` → the `copy-mode` table (tmux's default).
    #[default]
    Emacs,
}
```

Add fields to `KeyBindings` (keep the existing `root`/`prefix`/`prefix_keys`):

```rust
    copy_mode: HashMap<String, String>,
    copy_mode_vi: HashMap<String, String>,
    mode_keys: ModeKeys,
```

Route in `install`'s match:

```rust
                Table::CopyMode => { self.copy_mode.insert(binding.key, binding.command); }
                Table::CopyModeVi => { self.copy_mode_vi.insert(binding.key, binding.command); }
```

Add methods (place `pub(crate)` methods after the existing ones; private nothing new):

```rust
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
        table.get(key).or_else(|| table.get("Any")).cloned()
    }
```

Extend `clear`:

```rust
        self.copy_mode.clear();
        self.copy_mode_vi.clear();
        self.mode_keys = ModeKeys::default();
```

- [ ] **Step 4: Run, verify pass**

Run: `cargo test -p ozmux_tmux keybindings`
Expected: PASS (all existing + new).

- [ ] **Step 5: Commit**

```bash
git add crates/tmux_session/src/keybindings.rs
git commit -m "feat(tmux): parse copy-mode/copy-mode-vi key tables into KeyBindings"
```

---

### Task 2: `copy_mode_dispatch` → `CopyAction`

This is the keystone: maps a key (in copy mode) to either a verbatim relay, a clipboard-copy relay, an ozmux prompt (search/jump), an exit, or ignore. The bound command is **already** a complete tmux command (`send-keys -X cursor-down`); ozmux runs it verbatim and only classifies the side effects it must add.

**Files:**
- Modify: `crates/tmux_session/src/keybindings.rs`
- Modify: `crates/tmux_session/src/lib.rs` — `pub use keybindings::{CopyAction, PromptKind, copy_mode_dispatch};`

- [ ] **Step 1: Write the failing tests**

```rust
    fn vi_bindings(pairs: &[(&str, &str)]) -> KeyBindings {
        let mut kb = KeyBindings::default();
        kb.install(
            pairs.iter().map(|(k, c)| KeyBinding {
                table: Table::CopyModeVi, key: (*k).into(), command: (*c).into(), repeat: false,
            }).collect(),
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
            CopyAction::Copy { pipes, and_cancel, .. } => {
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
            CopyAction::Copy { pipes, and_cancel, .. } => {
                assert!(pipes, "copy-pipe* must not be clipboard-bridged");
                assert!(and_cancel);
            }
            other => panic!("expected Copy, got {other:?}"),
        }
    }

    #[test]
    fn command_prompt_search_forward_is_prompt() {
        let kb = vi_bindings(&[("/", r#"command-prompt -T search -p "(search down)" { send-keys -X search-forward "%%%" }"#)]);
        assert!(matches!(copy_mode_dispatch(&kb, "/"),
            CopyAction::Prompt { kind: PromptKind::SearchForward }));
    }

    #[test]
    fn command_prompt_jump_forward_is_single_char_prompt() {
        let kb = vi_bindings(&[("f", r#"command-prompt -1 -p "(jump forward)" { send-keys -X jump-forward "%%%" }"#)]);
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
        // `*` uses -FX with #{copy_cursor_word}; tmux substitutes it, so NO prompt.
        let kb = vi_bindings(&[("*", r#"send-keys -FX search-forward "#{copy_cursor_word}""#)]);
        assert!(matches!(copy_mode_dispatch(&kb, "*"), CopyAction::Relay(_)));
    }
```

- [ ] **Step 2: Run, verify failure**

Run: `cargo test -p ozmux_tmux keybindings::tests::motion_relays_verbatim`
Expected: FAIL — `copy_mode_dispatch` not defined.

- [ ] **Step 3: Implement**

```rust
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
        // command-prompt wrapping a non-search/jump command: relay verbatim.
        return CopyAction::Relay(command);
    }
    if command.contains("copy-pipe") || command.contains("copy-selection") || command.contains(" pipe") {
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

/// Detects the `PromptKind` of a `command-prompt`-wrapped binding by the inner
/// `search-*` / `jump-*` command name. Order matters: the more specific
/// `jump-to-*` and `search-backward` are tested before their prefixes.
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
```

NOTE: the search-backward-before-forward ordering in `prompt_kind` is load-bearing — `search-backward` contains the substring `search` but NOT `search-forward`, so testing `search-forward` first would be fine, but `jump-to-forward` contains `jump-forward`'s tokens differently; keep the specific-first order as written.

- [ ] **Step 4: Run, verify pass**

Run: `cargo test -p ozmux_tmux keybindings`
Expected: PASS.

- [ ] **Step 5: Re-export + commit**

In `crates/tmux_session/src/lib.rs`, extend the keybindings re-export:

```rust
pub use keybindings::{CopyAction, Forwarded, KeyBindings, PromptKind, copy_mode_dispatch, plan_forward};
```

```bash
cargo clippy -p ozmux_tmux && cargo fmt
git add crates/tmux_session/src/keybindings.rs crates/tmux_session/src/lib.rs
git commit -m "feat(tmux): copy_mode_dispatch classifies copy-table keys into CopyAction"
```

---

### Task 3: Copy-mode command builders + coordinate helpers

Pure string builders + the verified coordinate math. All unit-tested with the spec's verified numbers.

**Files:**
- Modify: `crates/tmux_session/src/enumerate.rs`
- Modify: `crates/tmux_session/src/lib.rs` — re-export the new builders + `CopyState`.

- [ ] **Step 1: Write the failing tests** (append to `enumerate.rs` tests)

```rust
    #[test]
    fn capture_offsets_match_verified_formula() {
        // spec §Verified: scroll_position=12, pane_height=8 -> -S -12 -E -5
        assert_eq!(capture_offsets(12, 8), (-12, -5));
        assert_eq!(capture_offsets(0, 8), (0, 7));
    }

    #[test]
    fn copy_mode_capture_command_uses_scroll_offsets() {
        assert_eq!(
            copy_mode_capture_command(PaneId(3), 12, 8),
            "capture-pane -e -p -t %3 -S -12 -E -5"
        );
    }

    #[test]
    fn absolute_to_visible_row_matches_verified_mapping() {
        // spec §Verified: history_size=53, scroll_position=3, abs 57 -> row 7
        assert_eq!(absolute_to_visible_row(57, 53, 3), 7);
        assert_eq!(absolute_to_visible_row(54, 53, 3), 4);
        // above the viewport clips negative
        assert_eq!(absolute_to_visible_row(10, 53, 3), 10i32 - 50);
    }

    #[test]
    fn copy_state_format_is_tab_separated() {
        assert!(COPY_STATE_FORMAT.contains('\t'));
        assert!(COPY_STATE_FORMAT.starts_with("#{pane_in_mode}"));
    }

    #[test]
    fn parse_copy_state_reads_all_fields() {
        let line = "1\t3\t8\t53\t6\t7\t1\t0\t2\t54\t6\t57";
        let s = parse_copy_state(line).expect("parse");
        assert_eq!(s.pane_in_mode, true);
        assert_eq!(s.scroll_position, 3);
        assert_eq!(s.pane_height, 8);
        assert_eq!(s.history_size, 53);
        assert_eq!((s.cursor_x, s.cursor_y), (6, 7));
        assert_eq!(s.selection_present, true);
        assert_eq!(s.rectangle, false);
        assert_eq!((s.sel_start_x, s.sel_start_y), (2, 54));
        assert_eq!((s.sel_end_x, s.sel_end_y), (6, 57));
    }

    #[test]
    fn parse_copy_state_detects_exited_mode() {
        let s = parse_copy_state("0\t0\t8\t53\t0\t0\t0\t0\t0\t0\t0\t0").expect("parse");
        assert!(!s.pane_in_mode);
    }

    #[test]
    fn copy_state_query_command_targets_pane() {
        assert_eq!(
            copy_state_query_command(PaneId(4)),
            format!("display-message -p -t %4 \"{COPY_STATE_FORMAT}\"")
        );
    }

    #[test]
    fn prompt_search_command_quotes_text_and_targets_pane() {
        assert_eq!(
            prompt_command(PaneId(2), PromptKind::SearchForward, "foo bar"),
            "send-keys -X -t %2 search-forward -- 'foo bar'"
        );
    }

    #[test]
    fn mode_keys_query_reads_format() {
        assert_eq!(mode_keys_command(), "display-message -p '#{mode-keys}'");
    }
```

- [ ] **Step 2: Run, verify failure**

Run: `cargo test -p ozmux_tmux enumerate::tests::capture_offsets_match_verified_formula`
Expected: FAIL — `capture_offsets` not defined.

- [ ] **Step 3: Implement** (add to `enumerate.rs`; import `PromptKind` and the `quote` helper)

At the top `use` block add `use crate::keybindings::PromptKind;` (one contiguous block — `input::quote` is already imported).

```rust
/// The tab-separated `display-message -F` format ozmux reads each refresh while
/// a pane is in copy mode. Field order is fixed; `parse_copy_state` depends on it.
pub const COPY_STATE_FORMAT: &str = "#{pane_in_mode}\t#{scroll_position}\t#{pane_height}\t#{history_size}\t#{copy_cursor_x}\t#{copy_cursor_y}\t#{selection_present}\t#{rectangle_toggle}\t#{selection_start_x}\t#{selection_start_y}\t#{selection_end_x}\t#{selection_end_y}";

/// One snapshot of a pane's copy-mode state from `COPY_STATE_FORMAT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CopyState {
    /// Whether the pane is still in a mode (`#{pane_in_mode}` != 0).
    pub pane_in_mode: bool,
    /// Lines scrolled back from the live tail.
    pub scroll_position: u32,
    /// Visible pane height in rows.
    pub pane_height: u16,
    /// Total scrollback history line count.
    pub history_size: u32,
    /// Copy cursor column (visible).
    pub cursor_x: u16,
    /// Copy cursor row (visible, 0 = top of viewport).
    pub cursor_y: u16,
    /// Whether a selection exists.
    pub selection_present: bool,
    /// Whether the selection is a rectangle (block) selection.
    pub rectangle: bool,
    /// Selection start column (visible).
    pub sel_start_x: u16,
    /// Selection start row (ABSOLUTE grid line — map with `absolute_to_visible_row`).
    pub sel_start_y: u32,
    /// Selection end column (visible).
    pub sel_end_x: u16,
    /// Selection end row (ABSOLUTE grid line).
    pub sel_end_y: u32,
}

/// Parses one `COPY_STATE_FORMAT` reply line. Returns `None` if any field is
/// missing or unparseable (one malformed refresh is dropped, not fatal).
pub fn parse_copy_state(line: &str) -> Option<CopyState> {
    let mut f = line.split('\t');
    let mut next_u32 = || f.next()?.trim().parse::<u32>().ok();
    let pane_in_mode = next_u32()? != 0;
    let scroll_position = next_u32()?;
    let pane_height = next_u32()? as u16;
    let history_size = next_u32()?;
    let cursor_x = next_u32()? as u16;
    let cursor_y = next_u32()? as u16;
    let selection_present = next_u32()? != 0;
    let rectangle = next_u32()? != 0;
    let sel_start_x = next_u32()? as u16;
    let sel_start_y = next_u32()?;
    let sel_end_x = next_u32()? as u16;
    let sel_end_y = next_u32()?;
    Some(CopyState {
        pane_in_mode, scroll_position, pane_height, history_size,
        cursor_x, cursor_y, selection_present, rectangle,
        sel_start_x, sel_start_y, sel_end_x, sel_end_y,
    })
}

/// Returns the `capture-pane -S/-E` offsets for the scrolled copy-mode view:
/// `(-scroll_position, pane_height - 1 - scroll_position)`. Verified against
/// tmux 3.6a (spec §Verified mechanism).
pub fn capture_offsets(scroll_position: u32, pane_height: u16) -> (i32, i32) {
    let start = -(scroll_position as i32);
    let end = pane_height as i32 - 1 - scroll_position as i32;
    (start, end)
}

/// Builds `capture-pane -e -p -t %N -S {start} -E {end}` for the scrolled view.
pub fn copy_mode_capture_command(pane: PaneId, scroll_position: u32, pane_height: u16) -> String {
    let (start, end) = capture_offsets(scroll_position, pane_height);
    format!("capture-pane -e -p -t %{} -S {start} -E {end}", pane.0)
}

/// Maps an absolute (history-relative) grid line to a visible viewport row:
/// `absolute_y - (history_size - scroll_position)`. Negative = above viewport.
pub fn absolute_to_visible_row(absolute_y: u32, history_size: u32, scroll_position: u32) -> i32 {
    let top = history_size as i32 - scroll_position as i32;
    absolute_y as i32 - top
}

/// Builds the per-refresh `display-message -p -t %N "<COPY_STATE_FORMAT>"`.
pub fn copy_state_query_command(pane: PaneId) -> String {
    format!("display-message -p -t %{} \"{COPY_STATE_FORMAT}\"", pane.0)
}

/// Builds `display-message -p '#{mode-keys}'` to read the active copy table.
pub fn mode_keys_command() -> String {
    "display-message -p '#{mode-keys}'".to_string()
}

/// Builds `send-keys -X -t %N <copy-command> -- '<text>'` for an ozmux prompt
/// submit (search regex or jump char). The text is tmux-quoted.
pub fn prompt_command(pane: PaneId, kind: PromptKind, text: &str) -> String {
    format!("send-keys -X -t %{} {} -- {}", pane.0, kind.copy_command(), quote(text))
}

/// Builds `show-buffer` to read tmux's top paste buffer for the clipboard bridge.
pub fn show_buffer_command() -> String {
    "show-buffer".to_string()
}
```

NOTE: `quote` is currently `pub(crate)` in `input.rs`; `prompt_command` lives in the same crate so `crate::input::quote` resolves. Do not widen `quote`'s visibility.

- [ ] **Step 4: Run, verify pass**

Run: `cargo test -p ozmux_tmux enumerate`
Expected: PASS.

- [ ] **Step 5: Re-export + commit**

In `lib.rs`, extend the `enumerate` re-export:

```rust
pub use enumerate::{
    COPY_STATE_FORMAT, CopyState, LIST_WINDOWS_FORMAT, WindowRow, absolute_to_visible_row,
    capture_offsets, copy_mode_capture_command, copy_state_query_command, mode_keys_command,
    parse_copy_state, parse_window_rows, prompt_command, refresh_client_command,
    select_pane_command, select_window_command, set_environment_command, show_buffer_command,
};
```

```bash
cargo clippy -p ozmux_tmux && cargo fmt
git add crates/tmux_session/src/enumerate.rs crates/tmux_session/src/lib.rs
git commit -m "feat(tmux): copy-mode capture/query/prompt command builders + coordinate helpers"
```

---

## Phase 2 — Attach wiring + parser notification

### Task 4: Fetch copy tables + `mode-keys` on attach

**Files:**
- Modify: `crates/tmux_session/src/enumerate.rs` — add pending fields to `EnumerationState`.
- Modify: `crates/tmux_session/src/event_pump.rs` — add `take_mode_keys`.
- Modify: `crates/tmux_session/src/plugin.rs` — send the three commands on attach, consume replies, clear on disconnect.

- [ ] **Step 1: Write the failing test** (in `event_pump.rs` tests)

```rust
    #[test]
    fn take_mode_keys_parses_vi() {
        let events = vec![TransportEvent::Protocol(ClientEvent::CommandComplete {
            id: CommandId(21), number: 0, ok: true, output: vec!["vi".to_string()],
        })];
        let mut pending = Some(CommandId(21));
        assert_eq!(take_mode_keys(&mut pending, &events), Some(ModeKeys::Vi));
        assert_eq!(pending, None);
    }

    #[test]
    fn take_mode_keys_defaults_emacs_on_other() {
        let events = vec![TransportEvent::Protocol(ClientEvent::CommandComplete {
            id: CommandId(22), number: 0, ok: true, output: vec!["emacs".to_string()],
        })];
        let mut pending = Some(CommandId(22));
        assert_eq!(take_mode_keys(&mut pending, &events), Some(ModeKeys::Emacs));
    }
```

- [ ] **Step 2: Run, verify failure**

Run: `cargo test -p ozmux_tmux event_pump::tests::take_mode_keys_parses_vi`
Expected: FAIL — `take_mode_keys` not defined.

- [ ] **Step 3: Implement**

In `keybindings.rs`, make `ModeKeys` `pub(crate)` already (done in Task 1) and add a parser:

```rust
impl ModeKeys {
    /// Parses tmux's `#{mode-keys}` reply (`vi` → `Vi`, anything else → `Emacs`).
    pub(crate) fn parse(s: &str) -> ModeKeys {
        if s.trim() == "vi" { ModeKeys::Vi } else { ModeKeys::Emacs }
    }
}
```

In `event_pump.rs`, add (mirror `take_prefix_keys`; import `ModeKeys`):

```rust
/// Returns the `ModeKeys` from a `CommandComplete` matching `pending`
/// (parsing `#{mode-keys}`), clearing `pending`.
pub(crate) fn take_mode_keys(
    pending: &mut Option<CommandId>,
    events: &[TransportEvent],
) -> Option<ModeKeys> {
    for event in events {
        if let TransportEvent::Protocol(ClientEvent::CommandComplete { id, ok, output, .. }) = event
            && *pending == Some(*id)
        {
            *pending = None;
            if *ok {
                return Some(output.first().map(|l| ModeKeys::parse(l)).unwrap_or_default());
            }
            tracing::warn!("mode-keys query failed");
            return None;
        }
    }
    None
}
```

In `enumerate.rs` `EnumerationState`, add:

```rust
    /// In-flight `list-keys -T copy-mode` command, if any.
    pub(crate) keys_copy_mode_pending: Option<CommandId>,
    /// In-flight `list-keys -T copy-mode-vi` command, if any.
    pub(crate) keys_copy_mode_vi_pending: Option<CommandId>,
    /// In-flight `#{mode-keys}` query, if any.
    pub(crate) mode_keys_pending: Option<CommandId>,
```

In `plugin.rs` `drain_tmux_events`, in the `Attached` send block (after the prefix query), add three sends mirroring the existing `list_keys_command` sends:

```rust
            match client.handle().send(&list_keys_command("copy-mode")) {
                Ok(id) => enumeration.keys_copy_mode_pending = Some(id),
                Err(error) => tracing::warn!(?error, "failed to send list-keys -T copy-mode"),
            }
            match client.handle().send(&list_keys_command("copy-mode-vi")) {
                Ok(id) => enumeration.keys_copy_mode_vi_pending = Some(id),
                Err(error) => tracing::warn!(?error, "failed to send list-keys -T copy-mode-vi"),
            }
            match client.handle().send(&mode_keys_command()) {
                Ok(id) => enumeration.mode_keys_pending = Some(id),
                Err(error) => tracing::warn!(?error, "failed to send mode-keys query"),
            }
```

In the `else` (still-attached) reply-consuming block, add (after the prefix-keys consume):

```rust
        if let Some(bindings) = take_keybindings(&mut enumeration.keys_copy_mode_pending, &events) {
            keybindings.install(bindings);
        }
        if let Some(bindings) = take_keybindings(&mut enumeration.keys_copy_mode_vi_pending, &events) {
            keybindings.install(bindings);
        }
        if let Some(mode_keys) = take_mode_keys(&mut enumeration.mode_keys_pending, &events) {
            keybindings.set_mode_keys(mode_keys);
        }
```

In the `Closed` reset block, add:

```rust
        enumeration.keys_copy_mode_pending = None;
        enumeration.keys_copy_mode_vi_pending = None;
        enumeration.mode_keys_pending = None;
```

Update imports in `plugin.rs`: add `mode_keys_command` to the `enumerate` use and `take_mode_keys` to the `event_pump` use; add `mode_keys_command` (it is `pub`, fine). `list_keys_command` already imported.

- [ ] **Step 4: Run, verify pass**

Run: `cargo test -p ozmux_tmux`
Expected: PASS (the plugin still compiles; the existing `plugin_registers_resources_and_stays_idle_without_connection` test passes — no connection means no sends).

- [ ] **Step 5: Commit**

```bash
cargo clippy -p ozmux_tmux && cargo fmt
git add crates/tmux_session/src/enumerate.rs crates/tmux_session/src/event_pump.rs crates/tmux_session/src/plugin.rs
git commit -m "feat(tmux): fetch copy-mode key tables + mode-keys on attach"
```

---

### Task 5: Surface `%pane-mode-changed` as a `ControlEvent`

The refresh/exit trigger. Verified: tmux emits `%pane-mode-changed %<pane>` on entry and exit.

**Files:**
- Inspect then modify: `crates/tmux_control_parser/src/` (the `ControlEvent` enum + its line parser) and re-export through `crates/tmux_control`.

- [ ] **Step 1: Locate the parser**

Run: `grep -rn "WindowPaneChanged\|pane-mode-changed\|ControlEvent" crates/tmux_control_parser/src crates/tmux_control/src | head`
Read the file defining `ControlEvent` and its `parse`/notification matcher.

- [ ] **Step 2: Write the failing test** (in the parser's test module, mirroring an existing notification test)

```rust
    #[test]
    fn parses_pane_mode_changed() {
        let ev = ControlEvent::parse("%pane-mode-changed %3").expect("parse");
        assert_eq!(ev, ControlEvent::PaneModeChanged { pane: PaneId(3) });
    }
```

- [ ] **Step 3: Run, verify failure**

Run: `cargo test -p tmux_control_parser parses_pane_mode_changed`
Expected: FAIL — variant absent (currently `%pane-mode-changed` falls into `ControlEvent::Unknown`).

- [ ] **Step 4: Implement**

Add the variant to `ControlEvent`:

```rust
    /// `%pane-mode-changed %<pane>` — the pane entered or left a mode (copy
    /// mode). Payload is only the pane id; query `#{pane_in_mode}` for the state.
    PaneModeChanged {
        /// The pane whose mode changed.
        pane: PaneId,
    },
```

Add the match arm in the notification parser, mirroring `%window-pane-changed`'s arm (strip the `%pane-mode-changed ` prefix, parse the `%N` pane id). Keep the `Unknown` fallback last.

- [ ] **Step 5: Run, verify pass + commit**

Run: `cargo test -p tmux_control_parser`
Expected: PASS.

```bash
cargo clippy -p tmux_control_parser && cargo fmt
git add crates/tmux_control_parser/src
git commit -m "feat(tmux-parser): surface %pane-mode-changed as ControlEvent::PaneModeChanged"
```

NOTE: if `tmux_control` re-exports `ControlEvent` variants explicitly, add `PaneModeChanged` there too; if it re-exports the whole enum, no change is needed.

---

## Phase 3 — State + entry/exit (milestone: enter/exit works, indicator lights)

### Task 6: `CopyModeState` marker (tmux-driven) + entry interception

`src/ui/copy_mode.rs`'s existing `EnterCopyModeActionEvent`/`ExitCopyMode` observers require `&mut Coalescer` (which tmux panes lack), so do NOT reuse them. Insert/remove `CopyModeState` directly.

**Files:**
- Modify: `src/ui/copy_mode.rs` — keep `CopyModeState`; no observer reuse for tmux.
- Modify: `src/tmux_input.rs` — entry interception.

- [ ] **Step 1: Write the failing test** (in `src/tmux_input.rs` tests, pure helper)

Add a pure predicate so entry detection is unit-testable without Bevy:

```rust
    #[test]
    fn detects_copy_mode_entry_command() {
        assert!(is_copy_mode_entry("copy-mode"));
        assert!(is_copy_mode_entry("copy-mode -u"));
        assert!(is_copy_mode_entry("copy-mode -eu"));
        assert!(!is_copy_mode_entry("copy-selection"));
        assert!(!is_copy_mode_entry("new-window"));
    }
```

- [ ] **Step 2: Run, verify failure**

Run: `cargo test -p ozmux-gui is_copy_mode_entry` (root binary package name; confirm with `cargo metadata` if unsure)
Expected: FAIL — `is_copy_mode_entry` not defined.

- [ ] **Step 3: Implement the predicate**

In `src/tmux_input.rs`:

```rust
/// True when a resolved tmux command enters copy mode (`copy-mode`, with any
/// flags). ozmux intercepts these to insert `CopyModeState` alongside running
/// the command on tmux.
fn is_copy_mode_entry(command: &str) -> bool {
    command
        .split_whitespace()
        .next()
        .is_some_and(|first| first == "copy-mode")
}
```

- [ ] **Step 4: Wire entry interception into `forward_keys_to_tmux`**

In `forward_keys_to_tmux`, the `plan_forward` result loop sends `Forwarded::Run(command)`. After sending a `Forwarded::Run` whose command `is_copy_mode_entry`, insert the marker on the active pane entity. Add `active pane Entity` to the system: change the active-pane param to also yield the entity, e.g.

```rust
    active_pane: Option<Single<(Entity, &TmuxPane), With<ActivePane>>>,
    mut commands: Commands,
```

After the `for action in actions` loop sends a `Forwarded::Run(cmd)`, if `is_copy_mode_entry(&cmd)` and there is an active pane entity, `commands.entity(entity).insert(crate::ui::copy_mode::CopyModeState);`. (The command was already sent to tmux by the existing `Run` arm — entry only adds the marker.)

Test approach (Bevy app test, mirroring `src/tmux_render.rs` tests): build an app with a `TmuxPane` + `ActivePane` entity, a stub `KeyBindings` binding `[` (or `BSpace`) to `copy-mode` in the prefix table, drive a `KeyboardInput` for the prefix then the key, run `forward_keys_to_tmux`, assert the entity gains `CopyModeState`. If a full keyboard-driven test is heavy, instead unit-test `is_copy_mode_entry` (Step 1) and add a focused system test that calls a small extracted `maybe_enter_copy_mode(&mut commands, entity, &cmd)` helper.

- [ ] **Step 5: Run + commit**

Run: `cargo test -p ozmux-gui tmux_input`
Expected: PASS.

```bash
cargo clippy --workspace && cargo fmt
git add src/tmux_input.rs src/ui/copy_mode.rs
git commit -m "feat(copy-mode): insert CopyModeState on tmux copy-mode entry interception"
```

---

### Task 7: In-copy-mode key branch (relay verbatim + exit)

**Files:**
- Modify: `src/tmux_input.rs`

- [ ] **Step 1: Add the branch**

At the top of `forward_keys_to_tmux`, after the picker/IME/focus guards and after computing `mods`, branch when the active pane has `CopyModeState`:

```rust
    copy_modes: Query<(), With<crate::ui::copy_mode::CopyModeState>>,
```

If the active pane entity is in `copy_modes`, for each pressed key event:
1. Compute `name = bevy_key_to_tmux_name(&ev.logical_key, ev.key_code, mods)`; skip if `None`.
2. `match copy_mode_dispatch(&bindings, &name)`:
   - `CopyAction::Relay(cmd)` / `CopyAction::Copy { command: cmd, .. }` / `CopyAction::Exit(cmd)` → send `cmd` verbatim via `client.handle().send(&cmd)`.
   - `CopyAction::Exit(_)` → also `commands.entity(entity).remove::<CopyModeState>()`.
   - `CopyAction::Copy { pipes, and_cancel, .. }` → record a pending clipboard bridge (Task 10) if `!pipes`; if `and_cancel`, remove `CopyModeState`.
   - `CopyAction::Prompt { kind }` → open the search/jump prompt (Task 11); for now (until Task 11) `tracing::debug!` and ignore.
   - `CopyAction::Ignore` → nothing.
3. `events.clear()` / `return` so the non-copy `plan_forward` path does not also run.

Keep GUI chords (`Cmd+V`, `Cmd+Q`, picker) intercepting BEFORE this branch (they already do, earlier in the function).

- [ ] **Step 2: Test**

Bevy app test: entity with `TmuxPane + ActivePane + CopyModeState`, `KeyBindings` with `copy-mode-vi` `j → send-keys -X cursor-down` and `q → send-keys -X cancel`, a `TmuxConnection` stub capturing sent commands (follow the `display_only_pane...` test's connection setup in `src/tmux_render.rs`). Drive `j` → assert the captured command is `send-keys -X cursor-down`. Drive `q` → assert `cancel` sent AND `CopyModeState` removed.

If capturing sent commands needs a fake, extract the dispatch-to-commands decision into a pure helper `fn plan_copy_actions(bindings, key_names) -> Vec<CopyOutcome>` and unit-test that without a live connection (preferred — mirrors how `plan_forward` is tested purely). `CopyOutcome` = `{ command: Option<String>, exit: bool, bridge: bool, prompt: Option<PromptKind> }`.

- [ ] **Step 3: Commit**

```bash
cargo clippy --workspace && cargo fmt
git add src/tmux_input.rs
git commit -m "feat(copy-mode): relay copy-table keys verbatim; exit on cancel"
```

**MILESTONE:** entering copy mode (`prefix-[`), pressing motions (they drive tmux's real copy cursor), and `q`/`Enter` exit now work end-to-end at the protocol level. Rendering of the scrolled view/overlay lands in Phase 4.

---

## Phase 4 — Rendering (milestone: scrollback + selection visible)

### Task 8: Refresh system skeleton — gate live emit, query state, capture, rebuild grid

**Files:**
- Create: `src/tmux_copy_mode.rs`
- Modify: `src/tmux_render.rs` — `route_tmux_output` advances always, emits only when not in copy mode.
- Modify: `src/main.rs` — add `OzmuxTmuxCopyModePlugin`.

- [ ] **Step 1: Gate the live emit** (`src/tmux_render.rs`)

Change `route_tmux_output` so it always `handle.advance(&data)` (never drops output) but only `handle.flush_emit(...)` when the pane is NOT in copy mode. Add `copy_modes: Query<(), With<crate::ui::copy_mode::CopyModeState>>` and guard the `flush_emit` call:

```rust
        handle.advance(&data);
        if copy_modes.get(entity).is_err() {
            handle.flush_emit(&mut commands, entity);
        }
        let _ = handle.take_replies();
```

Test: extend the existing `output_routed_into_pane_grid_renders_text` test with a sibling that inserts `CopyModeState` on the pane and asserts the grid does NOT change after `advance` (live emit gated), then removing it + a forced emit repaints.

- [ ] **Step 2: Refresh plugin skeleton** (`src/tmux_copy_mode.rs`)

Create `OzmuxTmuxCopyModePlugin`. Resources:

```rust
/// Pending copy-mode control-command replies, keyed by CommandId, so per-pane
/// refresh round-trips are correlated and stale ones dropped.
#[derive(Resource, Default)]
struct CopyModeTxns {
    /// state-query replies → pane.
    state: HashMap<tmux_control::CommandId, PaneId>,
    /// capture replies → pane.
    capture: HashMap<tmux_control::CommandId, PaneId>,
    /// show-buffer replies pending the clipboard bridge.
    buffer: Vec<tmux_control::CommandId>,
    /// per-pane generation counter; a reply older than the pane's current gen is dropped.
    gen: HashMap<PaneId, u64>,
}

/// One per-pane handle used purely to parse `capture-pane` bytes into the pane's
/// rendered grid while in copy mode (the live handle stays untouched).
#[derive(Component)]
struct CopyRenderHandle(TerminalHandle);
```

Systems (registered in `Update`, `.after(TmuxProjectionSet)`):
1. `issue_copy_refresh` — `run_if` there are panes with `CopyModeState`, AND triggered by `%pane-mode-changed` or after a relayed copy command. For each in-copy-mode pane, send `copy_state_query_command(pane.id)` and record the `CommandId` in `txns.state`. (To trigger after relays, set a `CopyRefreshNeeded` marker/event when Task 7 relays a command; the simplest robust v1: also run the refresh every frame while in copy mode but skip the `capture-pane` when `scroll_position` is unchanged — see Step 4. Per spec Open Q1, coalesce per frame.)
2. `consume_copy_state` — drain transport replies matching `txns.state` (use the same `client.events()` drain the plugin uses, OR read the already-drained batch; mirror `take_keybindings` correlation). Parse with `parse_copy_state`. If `!pane_in_mode` → trigger exit (remove `CopyModeState`, force the live handle to emit). Else, if scroll/region changed, send `copy_mode_capture_command(pane, scroll_position, pane_height)` and record in `txns.capture`; stash the `CopyState` on a per-pane component for the overlay step (Task 9).
3. `consume_copy_capture` — drain replies matching `txns.capture`; feed the bytes (CRLF-joined, with a `\x1b[H\x1b[2J` reset prefix — reuse the `capture_to_bytes` shape from `event_pump.rs`) into the pane's `CopyRenderHandle` (creating it sized to the pane if absent), then `flush_emit` it to the pane entity so the pane's `TerminalGrid` shows the captured view.

NOTE: the cleanest reply source is the same crossbeam receiver `drain_tmux_events` reads — but that system already drains it. Two options: (a) move copy-mode reply correlation INTO `ozmux_tmux` (add `take_copy_state`/`take_copy_capture` helpers consumed inside `drain_tmux_events`, then publish results as messages the binary reads); (b) have the binary subscribe via a dedicated message. Prefer (a): add `pub fn take_copy_state(pending, events) -> Option<(PaneId, CopyState)>`-style helpers to `event_pump.rs` (mirroring `take_pane_captures`) and a `CopyModeRefresh` message the binary consumes. Decide this in Step 2 and keep the correlation in the crate (consistent with every other reply).

- [ ] **Step 3: Test**

Pure unit test for the reply helpers in `event_pump.rs` (mirror `take_pane_captures_seeds_matching_reply_as_output`): given a `CommandComplete` with a `COPY_STATE_FORMAT` line and a matching pending entry, returns the parsed `CopyState` for the pane. Capture-bytes test: a capture reply produces the `\x1b[H\x1b[2J`-prefixed CRLF-joined bytes.

- [ ] **Step 4: Coalesce / skip unchanged capture**

Track last `scroll_position` per pane; only send `copy_mode_capture_command` when it changed (cursor/selection-only motions skip the capture — they only move the overlay, Task 9). Document with a `// NOTE:` only if a missed capture would cause a real stale-grid bug; otherwise plain code.

- [ ] **Step 5: Register + commit**

`src/main.rs`: add `OzmuxTmuxCopyModePlugin` to the plugin list near `CopyModePlugin`.

```bash
cargo clippy --workspace && cargo fmt
git add src/tmux_copy_mode.rs src/tmux_render.rs src/main.rs crates/tmux_session/src/event_pump.rs crates/tmux_session/src/lib.rs
git commit -m "feat(copy-mode): refresh system rebuilds pane grid from capture-pane snapshots"
```

---

### Task 9: Cursor + selection overlay

**Files:**
- Modify: `src/tmux_copy_mode.rs`

- [ ] **Step 1: Write the failing test** (pure overlay-builder helper)

```rust
    #[test]
    fn overlay_maps_cursor_and_selection_to_viewport() {
        // history_size=53, scroll_position=3, pane_height=8
        let state = CopyState {
            pane_in_mode: true, scroll_position: 3, pane_height: 8, history_size: 53,
            cursor_x: 6, cursor_y: 7, selection_present: true, rectangle: false,
            sel_start_x: 2, sel_start_y: 54, sel_end_x: 6, sel_end_y: 57,
        };
        let (vi_cursor, selection) = build_overlay(&state);
        assert_eq!((vi_cursor.column, vi_cursor.row), (6, 7));
        let sel = selection.expect("selection present");
        assert_eq!((sel.start.column, sel.start.row), (2, 4)); // 54 - (53-3) = 4
        assert_eq!((sel.end.column, sel.end.row), (6, 7));      // 57 - 50 = 7
        assert_eq!(sel.kind, SelectionKind::Char);
    }

    #[test]
    fn overlay_omits_selection_when_absent() {
        let state = CopyState {
            pane_in_mode: true, scroll_position: 0, pane_height: 8, history_size: 0,
            cursor_x: 1, cursor_y: 1, selection_present: false, rectangle: false,
            sel_start_x: 0, sel_start_y: 0, sel_end_x: 0, sel_end_y: 0,
        };
        let (_c, selection) = build_overlay(&state);
        assert!(selection.is_none());
    }
```

- [ ] **Step 2: Run, verify failure**

Run: `cargo test -p ozmux-gui build_overlay`
Expected: FAIL — `build_overlay` not defined.

- [ ] **Step 3: Implement**

```rust
/// Builds the `ViCursor` + optional `SelectionRange` overlay for the rendered
/// grid from a copy-mode state snapshot, mapping absolute selection rows to
/// visible rows via the verified equation. Rectangle selections render as
/// `Char` in v1 (the schema has no block kind — spec Open Q3).
fn build_overlay(state: &CopyState) -> (ViCursor, Option<SelectionRange>) {
    let vi_cursor = ViCursor {
        row: state.cursor_y as i32,
        column: state.cursor_x,
        in_scrollback: false,
    };
    let selection = state.selection_present.then(|| {
        let start_row = absolute_to_visible_row(state.sel_start_y, state.history_size, state.scroll_position);
        let end_row = absolute_to_visible_row(state.sel_end_y, state.history_size, state.scroll_position);
        SelectionRange {
            start: ViewportPoint { row: clamp_row(start_row, state.pane_height), column: state.sel_start_x },
            end: ViewportPoint { row: clamp_row(end_row, state.pane_height), column: state.sel_end_x },
            kind: SelectionKind::Char,
        }
    });
    (vi_cursor, selection)
}

/// Clamps a visible row to `-1` (above) or `rows` (below) for off-screen
/// selection endpoints, matching `ViewportPoint`'s clamping convention.
fn clamp_row(row: i32, rows: u16) -> i16 {
    row.clamp(-1, rows as i32) as i16
}
```

NOTE: confirm `ViCursor`'s exact field types (`row` is signed — `crates/ozma_tty_renderer/src/schema`). Import `ViCursor`, `SelectionRange`, `ViewportPoint`, `SelectionKind` from `ozma_tty_renderer`.

- [ ] **Step 4: Apply the overlay to the grid**

After `consume_copy_capture` rebuilds the grid (Task 8), a system writes `grid.vi_cursor = Some(vi_cursor)` and `grid.selection = selection` for the pane from its stashed `CopyState`. Order it AFTER the capture grid rebuild (`.chain()`). Mutate conditionally (only write when changed) per the rules.

- [ ] **Step 5: Run + commit**

Run: `cargo test -p ozmux-gui build_overlay`
Expected: PASS.

```bash
cargo clippy --workspace && cargo fmt
git add src/tmux_copy_mode.rs
git commit -m "feat(copy-mode): overlay copy cursor + selection onto the rendered grid"
```

**MILESTONE:** navigating copy mode now visibly scrolls/selects on screen.

---

### Task 10: Clipboard bridge

**Files:**
- Modify: `src/tmux_copy_mode.rs`

- [ ] **Step 1: Implement**

When Task 7 produced a `CopyAction::Copy { pipes: false, .. }`, after relaying the copy command, send `show_buffer_command()` and record the `CommandId` in `txns.buffer`. A `consume_copy_buffer` system drains matching replies and writes the joined output lines to the `Clipboard` resource (`clipboard.write(text)` — see `src/clipboard.rs`). `pipes: true` copies skip the bridge entirely.

- [ ] **Step 2: Test**

Unit-test the reply→clipboard text join in a pure helper `fn buffer_reply_to_text(lines: &[String]) -> String` (`lines.join("\n")`); assert it preserves multi-line buffers. The system wiring is covered by the Task 14 integration test.

- [ ] **Step 3: Commit**

```bash
cargo clippy --workspace && cargo fmt
git add src/tmux_copy_mode.rs
git commit -m "feat(copy-mode): bridge tmux paste buffer to the system clipboard on copy"
```

---

## Phase 5 — Prompt + mouse (full parity)

### Task 11: Search / jump prompt overlay

**Files:**
- Create: `src/ui/copy_search.rs`
- Modify: `src/tmux_input.rs` (open the prompt on `CopyAction::Prompt`), `src/main.rs`, `src/ui.rs` (module decl)

- [ ] **Step 1: Prompt resource + state**

```rust
/// The active copy-mode prompt (search regex or jump char). Present while the
/// user is typing; owns the keyboard like the session picker.
#[derive(Resource, Default)]
pub struct CopyPrompt {
    /// The pending prompt, if open.
    pub open: Option<CopyPromptState>,
}

/// In-progress prompt input.
pub struct CopyPromptState {
    /// Which copy command to run on submit.
    pub kind: PromptKind,
    /// The pane the result targets.
    pub pane: PaneId,
    /// Text typed so far.
    pub text: String,
}
```

- [ ] **Step 2: Open on dispatch**

In `forward_keys_to_tmux`'s copy-mode branch, `CopyAction::Prompt { kind }` sets `copy_prompt.open = Some(CopyPromptState { kind, pane: active_pane_id, text: String::new() })` and returns (drains events).

- [ ] **Step 3: Drive the prompt**

A system (gated `run_if(|p: Res<CopyPrompt>| p.open.is_some())`) reads `KeyboardInput`: printable chars append to `text` (jump kinds submit immediately on the first char — `kind.is_single_char()`); `Enter` submits; `Escape` cancels. On submit, send `prompt_command(pane, kind, &text)` via the connection and clear `open`. While `open.is_some()`, `forward_keys_to_tmux` must early-return (add a guard mirroring the `picker.open` guard at its top).

- [ ] **Step 4: Render the prompt**

Render a one-line input at the bottom of the active pane (reuse the palette/picker UI in `src/ui/palette.rs` / `src/tmux_picker.rs` as the pattern — a `Node` + `Text`). Show the prompt label (`/`, `?`, or the jump glyph) + the typed text.

- [ ] **Step 5: Test + commit**

Unit-test a pure `fn prompt_label(kind: PromptKind) -> &'static str` and the submit-string via `prompt_command` (already tested). System behavior verified in Task 14.

```bash
cargo clippy --workspace && cargo fmt
git add src/ui/copy_search.rs src/tmux_input.rs src/ui.rs src/main.rs
git commit -m "feat(copy-mode): search + jump prompt overlay feeding send-keys -X"
```

---

### Task 12: Mouse wheel in copy mode

**Files:**
- Modify: `src/input/mouse_wheel.rs`

- [ ] **Step 1: Suppress alacritty scrollback + relay wheel**

`mouse_wheel.rs` already queries `copy_modes: Query<(), With<CopyModeState>>`. While the active pane is in copy mode, instead of host scrollback, map each wheel notch to the copy table's `WheelUpPane`/`WheelDownPane` binding via `copy_mode_dispatch(&bindings, "WheelUpPane")` (note: wheel key names in tmux are `WheelUpPane`/`WheelDownPane`) and relay the resulting command. If unbound, fall back to relaying `send-keys -X -t %pane scroll-up`/`scroll-down`.

- [ ] **Step 2: Test**

Unit-test the notch→key-name mapping (`fn wheel_key_name(up: bool) -> &str`) and that a copy-mode wheel produces a relay, not a host-scroll, via the existing pure wheel helpers in `crates/ozma_tty_engine/src/wheel.rs` patterns.

- [ ] **Step 3: Commit**

```bash
cargo clippy --workspace && cargo fmt
git add src/input/mouse_wheel.rs
git commit -m "feat(copy-mode): mouse wheel drives tmux copy-mode scroll, not host scrollback"
```

---

### Task 13: Mouse drag-select (delta-motion state machine)

**Files:**
- Modify: `src/input/mouse_buttons.rs`, `src/tmux_copy_mode.rs`

- [ ] **Step 1: Implement the state machine**

On press in copy mode, target cell `(col,row)` from cursor position via `layout_tmux_panes`' cell metrics. Move the copy cursor there with delta motions off the last-read `copy_cursor_x/y` (`send-keys -X -N {Δ} cursor-{dir}`), AWAIT the readback (the refresh's `CopyState`), recompute against the newest pointer, repeat until converged; then `begin-selection`. On drag, repeat the reposition. On release, relay `copy-selection`. Use a `DragSelect` resource holding the target cell + a "settling" flag; the refresh loop advances it.

NOTE: never assume a requested delta landed — short lines clamp `copy_cursor_x`; recompute from the readback each step. This is the async-race fix the spec calls out.

- [ ] **Step 2: Test**

Unit-test the pure delta computation `fn cursor_deltas(cur: (u16,u16), target: (u16,u16)) -> Vec<String>` returning the `send-keys -X -N ...` commands; assert directions/counts and that a zero delta yields no command.

- [ ] **Step 3: Commit**

```bash
cargo clippy --workspace && cargo fmt
git add src/input/mouse_buttons.rs src/tmux_copy_mode.rs
git commit -m "feat(copy-mode): mouse drag-select via delta-motion state machine"
```

---

## Phase 6 — Integration

### Task 14: Gated real-tmux integration test

**Files:**
- Modify: `src/tmux_copy_mode.rs` (test module)

- [ ] **Step 1: Write the `#[ignore]` test**

Mirror `display_only_pane_does_not_inject_phantom_device_replies` in `src/tmux_render.rs` (spawn `tmux -CC` via `TmuxServer`, drive `app.update()` until attached + a pane is projected). Then: send `copy-mode -t %pane`; relay `send-keys -X -t %pane cursor-up` (×3), `begin-selection`, `cursor-down`; run a refresh cycle; assert:
- the pane's `TerminalGrid` matches the scrolled `capture-pane` content,
- `grid.vi_cursor` matches the queried `copy_cursor_x/y`,
- `grid.selection` is present with mapped rows,
- relaying `copy-selection-and-cancel` + the bridge lands text in the `Clipboard` resource,
- after exit, `CopyModeState` is removed and the grid returns to the live view.

Mark `#[ignore = "requires a real tmux binary and a controlling PTY"]`.

- [ ] **Step 2: Run it explicitly**

Run: `cargo test -p ozmux-gui --  --ignored copy_mode_integration`
Expected: PASS against local tmux 3.6a. (If `-M` capture is preferred, switch `copy_mode_capture_command` to `-M` here and confirm; otherwise keep `-S/-E`.)

- [ ] **Step 3: Commit**

```bash
git add src/tmux_copy_mode.rs
git commit -m "test(copy-mode): gated real-tmux integration test for the full cycle"
```

---

## Self-Review

**Spec coverage:**
- Decisions 1–2 (drive real copy mode, tmux.conf bindings) → Tasks 1, 2, 4, 7. ✓
- Decision 3 (live handle untouched, render from capture) → Tasks 8, 9. ✓
- Decision 4 (scope: nav/selection/scroll/jump/search/wheel/drag) → Tasks 7, 9, 11, 12, 13. ✓
- Decision 5 (binding-aware clipboard) → Tasks 2 (`Copy{pipes}`), 10. ✓
- Verified mechanism (capture offsets, coordinate mapping, `%pane-mode-changed`, `mode-keys`, `%output` not paused) → Tasks 3, 5, 8. ✓
- Reply-correlation transaction map → Task 8 (`CopyModeTxns`). ✓
- Prompt-driven search/jump (`command-prompt` form) → Tasks 2, 11. ✓
- Open Q3 rectangle (render as Char in v1) → Task 9. ✓

**Type consistency:** `CopyAction`/`PromptKind`/`copy_mode_dispatch` (T2), `CopyState`/`capture_offsets`/`absolute_to_visible_row`/`copy_mode_capture_command`/`copy_state_query_command`/`prompt_command`/`show_buffer_command`/`mode_keys_command` (T3), `ModeKeys`/`copy_command`/`set_mode_keys` (T1), `take_mode_keys` (T4), `CopyModeTxns`/`CopyRenderHandle`/`build_overlay` (T8–T9), `CopyPrompt`/`CopyPromptState` (T11) — names are used consistently across tasks.

**Known iteration points (Bevy ECS wiring the subagent finalizes against the compiler):** the reply-source decision in Task 8 (correlate inside `ozmux_tmux` and publish a message — preferred), system ordering for capture-rebuild-then-overlay (`.chain()`), and the exact `ViCursor` field types. These are flagged in-task; the pure logic they wrap is fully specified and tested.

**Scope note:** this is one feature delivered in 6 phases; Phase 3 (entry/exit) and Phase 4 (rendering) are the load-bearing milestones, each independently testable. If execution stalls, Phases 1–4 already yield usable keyboard copy mode; Phase 5 (mouse/search) and Phase 6 (integration test) layer on top.
