# tmux Phase 3a — Keyboard input + GUI chords + reply routing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Make tmux panes keyboard-interactive — forward focused keys to tmux through its key tables (`send-keys -K`), intercept a small fixed GUI-chord set, forward terminal replies (DSR/DA) back to tmux, and delete keybindings/actions that have no meaning under tmux.

**Architecture:** A new `tmux_session::input` module holds a pure Bevy-key → tmux-key-name mapper + `send-keys` command builders. A new binary plugin `OzmuxTmuxInputPlugin` reads focused keyboard input, intercepts GUI chords (open picker, quit, paste, release-inline-focus), and forwards everything else as one batched `send-keys -K -c <client>` per frame. The control client's name is captured on attach (mirroring the `list-windows` enumeration query). The legacy `dispatch_focused_key` keyboard path and the surface/copy actions are removed.

**Tech Stack:** Rust 2024, Bevy 0.18 ECS (`KeyboardInput.logical_key`, `ButtonInput<KeyCode>`), tmux `-CC` control mode (`send-keys`, `display-message`).

**Spec:** `docs/superpowers/specs/2026-06-15-tmux-phase3a-keyboard-input-design.md`.
**Worktree/branch:** `tmux-phase3` at `/Users/taiga/workspace/ozmux/wt/tmux-phase3` (off `tmux-migration`). All commands run there.

**Conventions** (`.claude/rules/rust.md`): no `mod.rs`; comments only `// TODO:`/`// NOTE:`/`// SAFETY:`; `//!` on module files; `///` on `pub` items; imports contiguous; mutable params first; private items after public; minimize visibility.

**⚠️ Verify-live items** (cannot be confirmed headlessly — covered by `#[ignore]`-gated real-tmux tests, matching `crates/tmux_session/tests/real_tmux_*.rs`): exact `send-keys -K -c` routing under `tmux -CC`, and the `display-message`/`%client-session-changed` client-name source.

---

## File structure

- `crates/tmux_session/src/input.rs` *(new)* — pure key-name mapper + `send-keys` builders + tmux-arg quoting. Exported from `lib.rs`.
- `crates/tmux_session/src/enumerate.rs` — add `client_name_command()` builder.
- `crates/tmux_session/src/event_pump.rs` / `plugin.rs` — capture the client name on attach (mirror the `list-windows` pending-reply pattern); store on connection-scoped state.
- `crates/tmux_session/src/connection.rs` — hold the cached client name (`TmuxConnection`).
- `src/tmux_input.rs` *(new)* — `OzmuxTmuxInputPlugin`: focused-key forwarding + GUI-chord interception.
- `src/tmux_render.rs` — `route_tmux_output` forwards `take_replies()` instead of dropping.
- `src/main.rs` — register `OzmuxTmuxInputPlugin`; drop the legacy keyboard dispatch.
- `src/input.rs` — remove `dispatch_focused_key` (legacy keyboard path) + its registration.
- `crates/configs/src/shortcuts.rs` + `src/action/*` — delete surface/copy/copy-mode actions, bindings, dispatch.
- `src/tmux_boot.rs` — delete (orphaned).

---

## Task 1: Delete the orphaned `src/tmux_boot.rs`

**Files:** Delete `src/tmux_boot.rs`.

- [ ] **Step 1: Confirm it's unreferenced**

Run: `grep -rn "tmux_boot" src/ crates/`
Expected: no `mod tmux_boot;` and no `use ...tmux_boot...`. (The Phase-2 merge re-introduced this dead file; `src/main.rs` declares `mod tmux_picker;`/`mod tmux_render;` but not `mod tmux_boot;`.)

- [ ] **Step 2: Delete + build**

```bash
git rm src/tmux_boot.rs
cargo build
```
Expected: SUCCESS (file was never compiled).

- [ ] **Step 3: Commit**

```bash
git commit -m "chore(tmux): remove orphaned tmux_boot.rs (dead since the phase-2 merge)"
```

---

## Task 2: `tmux_session::input` — key-name mapper + command builders

**Files:** Create `crates/tmux_session/src/input.rs`; modify `crates/tmux_session/src/lib.rs`.

- [ ] **Step 1: Create the module with the mapper, builders, and tests**

Create `crates/tmux_session/src/input.rs`:

```rust
//! Pure translation of Bevy keyboard input to tmux `send-keys` commands.
//!
//! Forwarded keys route through tmux's key tables (`send-keys -K`), so tmux's
//! prefix + bindings act. Raw bytes (terminal replies) go to a pane via
//! `send-keys -H`. All construction here is pure + unit-tested; the binary's
//! input plugin is a thin adapter.

use bevy::input::keyboard::Key;

/// Active keyboard modifiers for a key event.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeyMods {
    /// Control.
    pub ctrl: bool,
    /// Alt / Option (tmux `M-`).
    pub alt: bool,
    /// Shift.
    pub shift: bool,
    /// Command / Super — tmux has NO equivalent; GUI-only (see `bevy_key_to_tmux_name`).
    pub super_: bool,
}

/// Maps a Bevy logical key + modifiers to a tmux key-name string for
/// `send-keys -K`, or `None` if the key has no tmux representation OR carries
/// `Super` (which is GUI-only and must never be forwarded).
///
/// NOTE: `Shift` is folded into the glyph for `Key::Character` (so a shifted
/// letter arrives already uppercased) — the `S-` prefix is only emitted for
/// non-character named keys (e.g. `S-Up`). `Super` returns `None`: the caller
/// intercepts GUI chords before this and drops any other `Super` key.
pub fn bevy_key_to_tmux_name(key: &Key, mods: KeyMods) -> Option<String> {
    if mods.super_ {
        return None;
    }
    let base = match key {
        Key::Character(s) => {
            let mut chars = s.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            c.to_string()
        }
        Key::Enter => "Enter".to_string(),
        Key::Escape => "Escape".to_string(),
        Key::Tab if mods.shift => return Some(prefix(&mods, false, "BTab")),
        Key::Tab => "Tab".to_string(),
        Key::Backspace => "BSpace".to_string(),
        Key::Space => "Space".to_string(),
        Key::ArrowUp => "Up".to_string(),
        Key::ArrowDown => "Down".to_string(),
        Key::ArrowLeft => "Left".to_string(),
        Key::ArrowRight => "Right".to_string(),
        Key::Home => "Home".to_string(),
        Key::End => "End".to_string(),
        Key::PageUp => "PageUp".to_string(),
        Key::PageDown => "PageDown".to_string(),
        Key::Insert => "IC".to_string(),
        Key::Delete => "DC".to_string(),
        Key::F1 => "F1".to_string(),
        Key::F2 => "F2".to_string(),
        Key::F3 => "F3".to_string(),
        Key::F4 => "F4".to_string(),
        Key::F5 => "F5".to_string(),
        Key::F6 => "F6".to_string(),
        Key::F7 => "F7".to_string(),
        Key::F8 => "F8".to_string(),
        Key::F9 => "F9".to_string(),
        Key::F10 => "F10".to_string(),
        Key::F11 => "F11".to_string(),
        Key::F12 => "F12".to_string(),
        _ => return None,
    };
    let shift_prefix = !matches!(key, Key::Character(_));
    Some(prefix(&mods, shift_prefix, &base))
}

/// Builds a batched `send-keys -K -c <client>` command for the given key names
/// (one tmux command per frame). `client` and each name are quoted.
pub fn send_keys_command(client: &str, names: &[String]) -> String {
    let mut cmd = format!("send-keys -K -c {}", quote(client));
    for n in names {
        cmd.push(' ');
        cmd.push_str(&quote(n));
    }
    cmd
}

/// Builds a `send-keys -H -t <pane> <hex>…` command injecting raw bytes into a
/// pane (used for terminal replies). `pane` is the tmux pane id like `%3`.
pub fn send_bytes_command(pane: &str, bytes: &[u8]) -> String {
    let mut cmd = format!("send-keys -H -t {}", quote(pane));
    for b in bytes {
        cmd.push_str(&format!(" {b:02x}"));
    }
    cmd
}

/// Prefixes `C-`/`M-`/`S-` modifier tokens onto a tmux key name.
fn prefix(mods: &KeyMods, shift: bool, base: &str) -> String {
    let mut out = String::new();
    if mods.ctrl {
        out.push_str("C-");
    }
    if mods.alt {
        out.push_str("M-");
    }
    if shift && mods.shift {
        out.push_str("S-");
    }
    out.push_str(base);
    out
}

/// Quotes a tmux command argument: wraps in single quotes if it contains
/// whitespace or shell/tmux metacharacters, escaping embedded single quotes.
fn quote(arg: &str) -> String {
    let needs = arg.is_empty()
        || arg
            .chars()
            .any(|c| c.is_whitespace() || "\"'\\$;|&<>(){}[]*?#`".contains(c));
    if !needs {
        return arg.to_string();
    }
    let escaped = arg.replace('\'', r"'\''");
    format!("'{escaped}'")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(ctrl: bool, alt: bool, shift: bool, super_: bool) -> KeyMods {
        KeyMods { ctrl, alt, shift, super_ }
    }

    #[test]
    fn plain_char_maps_to_itself() {
        assert_eq!(
            bevy_key_to_tmux_name(&Key::Character("a".into()), m(false, false, false, false)),
            Some("a".to_string())
        );
    }

    #[test]
    fn ctrl_char_gets_c_prefix() {
        assert_eq!(
            bevy_key_to_tmux_name(&Key::Character("c".into()), m(true, false, false, false)),
            Some("C-c".to_string())
        );
    }

    #[test]
    fn shift_is_not_prefixed_for_characters() {
        // The shifted glyph already arrives in the character; no S- prefix.
        assert_eq!(
            bevy_key_to_tmux_name(&Key::Character("A".into()), m(false, false, true, false)),
            Some("A".to_string())
        );
    }

    #[test]
    fn named_keys_map_to_tmux_names() {
        assert_eq!(bevy_key_to_tmux_name(&Key::Enter, m(false, false, false, false)), Some("Enter".into()));
        assert_eq!(bevy_key_to_tmux_name(&Key::ArrowUp, m(false, false, false, false)), Some("Up".into()));
        assert_eq!(bevy_key_to_tmux_name(&Key::Backspace, m(false, false, false, false)), Some("BSpace".into()));
        assert_eq!(bevy_key_to_tmux_name(&Key::PageUp, m(false, false, false, false)), Some("PageUp".into()));
        assert_eq!(bevy_key_to_tmux_name(&Key::Delete, m(false, false, false, false)), Some("DC".into()));
        assert_eq!(bevy_key_to_tmux_name(&Key::F5, m(false, false, false, false)), Some("F5".into()));
    }

    #[test]
    fn shift_prefixes_named_keys_only() {
        assert_eq!(bevy_key_to_tmux_name(&Key::ArrowUp, m(false, false, true, false)), Some("S-Up".into()));
        assert_eq!(bevy_key_to_tmux_name(&Key::Tab, m(false, false, true, false)), Some("BTab".into()));
    }

    #[test]
    fn super_is_never_forwarded() {
        assert_eq!(bevy_key_to_tmux_name(&Key::Character("p".into()), m(false, false, false, true)), None);
        assert_eq!(bevy_key_to_tmux_name(&Key::Enter, m(false, false, false, true)), None);
    }

    #[test]
    fn send_keys_batches_and_quotes() {
        assert_eq!(
            send_keys_command("ozmux", &["a".into(), "C-c".into(), "Up".into()]),
            "send-keys -K -c ozmux a C-c Up"
        );
        // A client name with a space is quoted.
        assert_eq!(
            send_keys_command("pts 3", &["a".into()]),
            "send-keys -K -c 'pts 3' a"
        );
        // A semicolon key name is quoted so tmux doesn't see a command separator.
        assert_eq!(
            send_keys_command("c", &[";".into()]),
            "send-keys -K -c c ';'"
        );
    }

    #[test]
    fn send_bytes_hex_encodes() {
        assert_eq!(send_bytes_command("%3", &[0x1b, b'[', b'0', b'n']), "send-keys -H -t %3 1b 5b 30 6e");
    }
}
```

- [ ] **Step 2: Declare + export the module**

In `crates/tmux_session/src/lib.rs`: add `mod input;` (alphabetical — after `mod enumerate;`/`mod event_pump;`, before `mod model;`) and `pub use input::{KeyMods, bevy_key_to_tmux_name, send_bytes_command, send_keys_command};`.

- [ ] **Step 3: Run tests**

Run: `cargo test -p ozmux_tmux input::`
Expected: PASS (all mapper + builder tests).

- [ ] **Step 4: Verify Bevy `Key` variant names compile**

If any `Key::*` variant name differs in this Bevy version (e.g. `Key::Backspace` vs `Key::Back`), `cargo build -p ozmux_tmux` will fail — fix the match arm against `bevy::input::keyboard::Key` (the existing `src/input.rs` already matches `Key::Character`, `Key::Shift`, etc., so cross-check there). Do NOT change the test expectations (tmux names are fixed by the man page).

- [ ] **Step 5: clippy + fmt + commit**

```bash
cargo clippy -p ozmux_tmux --all-targets && cargo fmt
git add crates/tmux_session/src/input.rs crates/tmux_session/src/lib.rs
git commit -m "feat(ozmux_tmux): tmux key-name mapper + send-keys command builders"
```

---

## Task 3: Client-name capture (mirror the enumeration query)

**Files:** Modify `crates/tmux_session/src/enumerate.rs`, `connection.rs`, `event_pump.rs`, `plugin.rs`.

Context: `event_pump.rs` already sends `list-windows` on attach and correlates the `CommandComplete` reply via a pending `CommandId` (`EnumerationState.pending` → `seed_from_reply`). The client-name query follows the SAME pattern.

- [ ] **Step 1: Add the command builder + test**

In `enumerate.rs`, next to `refresh_client_command`:

```rust
/// Builds `display-message -p -F '#{client_name}'` — prints the control
/// client's name as a one-line command reply (correlated like `list-windows`).
pub fn client_name_command() -> String {
    "display-message -p -F '#{client_name}'".to_string()
}
```

Test:
```rust
#[test]
fn client_name_command_prints_client_name_format() {
    assert_eq!(client_name_command(), "display-message -p -F '#{client_name}'");
}
```
Export `client_name_command` from `lib.rs` (add to the `enumerate` re-export). Run: `cargo test -p ozmux_tmux client_name_command`.

- [ ] **Step 2: Hold the cached name on `TmuxConnection`**

In `connection.rs`, add a `client_name: Option<String>` field to `TmuxConnection` with `pub fn client_name(&self) -> Option<&str>` and `pub fn set_client_name(&mut self, name: String)`, and clear it in `take()` (re-queried on reconnect). Keep existing methods.

- [ ] **Step 3: Send the query on attach + parse its reply**

In `event_pump.rs`/`plugin.rs`, mirror the enumeration flow: when the connection transitions to `Attached` (same place `list_windows_command()` is sent), also send `client_name_command()` and record its `CommandId` in a pending slot (extend `EnumerationState` with `client_name_pending: Option<CommandId>` or add a sibling). When a `CommandComplete` matches that id and `ok`, take the first non-empty output line and `connection.set_client_name(line)`.

```rust
// in the attach branch, alongside the list-windows send:
match client.handle().send(&client_name_command()) {
    Ok(id) => enumeration.client_name_pending = Some(id),
    Err(error) => tracing::warn!(?error, "failed to send client-name query"),
}
// in apply_events' CommandComplete arm (NonSend connection must be in scope to set it;
// if event_pump's pure fns can't hold the connection, set the name in drain_tmux_events
// after apply_events returns, threading the parsed name out):
```
NOTE: `apply_events` is a pure fn over `ProjectionModel`; the connection (`NonSend`) is mutated in `drain_tmux_events`. Thread the parsed client name out of the apply step (e.g. return `Option<String>` or scan the drained batch in `drain_tmux_events`) and call `connection.set_client_name(..)` there — do NOT add the `NonSend` to the pure reducer. Add a unit test for the pure "extract client name from a matching `CommandComplete`" helper.

- [ ] **Step 4: Tests + commit**

Run: `cargo test -p ozmux_tmux` (all pass, including a new pure test for client-name extraction).
clippy + fmt.
```bash
git add crates/tmux_session/src/{enumerate.rs,connection.rs,event_pump.rs,plugin.rs,lib.rs}
git commit -m "feat(ozmux_tmux): capture the control client name on attach"
```

---

## Task 4: Prune obsolete actions, bindings, and the legacy keyboard dispatch

**Files:** `crates/configs/src/shortcuts.rs`, `src/action/{close_surface,focus_surface,new_terminal_surface}.rs` (delete), `src/action.rs`/`src/action/*` mod decls, `src/input.rs`, `src/main.rs`, `crates/configs/src/raw.rs` (snapshot tests).

Goal: delete surface + copy + copy-mode actions/bindings, and remove the legacy `dispatch_focused_key` keyboard path (superseded by the new tmux plugin). Pane/window action modules (`split_pane`, `focus_pane`, `close_pane`, `swap_pane`, `workspace`) STAY (dormant; re-targeted in 3b).

- [ ] **Step 1: Delete the surface action modules**

```bash
git rm src/action/close_surface.rs src/action/focus_surface.rs src/action/new_terminal_surface.rs
```
Remove their `mod`/`use` declarations from `src/action.rs` (or wherever the action submodules are declared) and any imports in `src/input.rs`.

- [ ] **Step 2: Remove obsolete `ShortcutAction` variants + `Bindings` fields**

In `crates/configs/src/shortcuts.rs`, delete the variants with no tmux meaning and the `Copy`/`EnterCopyMode` ones: `FocusSurface`, `NewTerminalSurface`, `CloseSurface`, `BreakSurfaceToPane`, `RenameSurface`, `ListSurfaces`, `Copy`, `EnterCopyMode`. Delete the matching `Bindings` fields (`close_surface`, `new_terminal_surface`, `focus_surface_prev`, `focus_surface_next`, `enter_copy_mode`, `copy`) and their entries in `Bindings::default()` and `Bindings::iter()`. KEEP `paste` and `release_inline_focus`. KEEP the pane/window fields (close_pane, focus_pane_*, split_pane_*, swap_pane_*, new_workspace, focus_workspace_*) — dormant for 3b.
Update the JSON snapshot test (`default_shortcuts_json_snapshot`) and the `copy`/`paste` lookup tests accordingly (drop the `copy` assertion; keep `paste`).

- [ ] **Step 3: Remove the legacy keyboard dispatch**

In `src/input.rs`: delete `dispatch_focused_key` and its registration in `OzmuxShortcutPlugin::build` (the `.add_systems(Update, dispatch_focused_key...)`). Keep `current_modifiers` (the new plugin reuses it) — move it to a shared spot if needed, or re-export. Remove now-unused imports (the surface/copy action events, `forward_to_active_terminal` if unused, `bevy_to_terminal_key` if unused). `OzmuxShortcutPlugin` may become empty — if so, remove it from `main.rs` and delete it (the new `OzmuxTmuxInputPlugin` replaces it).

- [ ] **Step 4: Build + fix fallout**

Run: `cargo build`
Expected: compile errors only for now-dangling references — resolve by removing them. Run `cargo test -p ozmux_configs` (shortcut tests pass after the snapshot/binding updates).

- [ ] **Step 5: clippy + fmt + commit**

```bash
cargo clippy --workspace --all-targets && cargo fmt
git add -A
git commit -m "refactor(tmux): drop surface/copy actions + legacy keyboard dispatch (superseded by tmux forwarding)"
```

---

## Task 5: `OzmuxTmuxInputPlugin` — forwarding + GUI chords

**Files:** Create `src/tmux_input.rs`; modify `src/main.rs`.

- [ ] **Step 1: Create the plugin**

Create `src/tmux_input.rs`:

```rust
//! Forwards focused keyboard input to tmux via `send-keys -K`, intercepting a
//! fixed set of ozmux GUI chords. Replaces the legacy `dispatch_focused_key`
//! path for the tmux backend.

use crate::tmux_picker::SessionPicker;
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyCode, KeyboardInput};
use bevy::prelude::*;
use ozmux_tmux::{KeyMods, TmuxConnection, bevy_key_to_tmux_name, send_keys_command};

/// Registers the tmux keyboard-forwarding system.
pub struct OzmuxTmuxInputPlugin;

impl Plugin for OzmuxTmuxInputPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, forward_keys_to_tmux);
    }
}

fn forward_keys_to_tmux(
    mut picker: ResMut<SessionPicker>,
    mut exit: MessageWriter<AppExit>,
    mut events: MessageReader<KeyboardInput>,
    connection: NonSend<TmuxConnection>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    let mods = KeyMods {
        ctrl: keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight),
        alt: keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight),
        shift: keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight),
        super_: keys.pressed(KeyCode::SuperLeft) || keys.pressed(KeyCode::SuperRight),
    };

    let mut names: Vec<String> = Vec::new();
    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        if let Some(chord) = gui_chord(&ev.logical_key, &ev.key_code, mods) {
            match chord {
                GuiChord::OpenPicker => picker.open = true,
                GuiChord::Quit => {
                    exit.write(AppExit::Success);
                }
                // Paste + release-inline-focus handled by their own systems (kept).
                GuiChord::Other => {}
            }
            continue;
        }
        if let Some(name) = bevy_key_to_tmux_name(&ev.logical_key, mods) {
            names.push(name);
        }
    }

    if names.is_empty() {
        return;
    }
    let Some(client) = connection.client_name() else {
        return;
    };
    if let Some(c) = connection.client() {
        if let Err(e) = c.handle().send(&send_keys_command(client, &names)) {
            tracing::warn!(?e, "send-keys forward failed");
        }
    }
}

enum GuiChord {
    OpenPicker,
    Quit,
    Other,
}

/// Returns the GUI chord for a key event, if it is one. GUI chords are matched
/// on physical `key_code` + the `Super` (Cmd) modifier (layout-stable) and are
/// never forwarded to tmux.
fn gui_chord(_logical: &Key, key_code: &KeyCode, mods: KeyMods) -> Option<GuiChord> {
    if mods.super_ && mods.shift && *key_code == KeyCode::KeyP {
        return Some(GuiChord::OpenPicker);
    }
    if mods.super_ && !mods.shift && *key_code == KeyCode::KeyQ {
        return Some(GuiChord::Quit);
    }
    // Any other Super-modified key is GUI-only: swallow it (do not forward).
    if mods.super_ {
        return Some(GuiChord::Other);
    }
    None
}
```
NOTE: `SessionPicker` (with `open`) is currently private to `src/tmux_picker.rs` — make `SessionPicker` and its `open` field `pub(crate)` so this plugin can re-open the picker. Paste (`Cmd+V`) and release-inline-focus stay in their existing systems for now (do NOT route them here); they are caught by the `GuiChord::Other` swallow so they aren't forwarded — wire their real handlers in a follow-up step if the existing ones depended on the deleted dispatch (verify during Task 4 fallout).

- [ ] **Step 2: Register in `main.rs`**

Add `mod tmux_input;`, `use crate::tmux_input::OzmuxTmuxInputPlugin;`, and add `OzmuxTmuxInputPlugin` to the plugin set (after `OzmuxTmuxRenderPlugin`).

- [ ] **Step 3: build + clippy + fmt + commit**

Run: `cargo build`, `cargo clippy -p ozmux-gui --all-targets`, `cargo fmt`.
```bash
git add src/tmux_input.rs src/main.rs src/tmux_picker.rs
git commit -m "feat(tmux): forward focused keys via send-keys -K + GUI-chord interception"
```

---

## Task 6: Reply routing

**Files:** Modify `src/tmux_render.rs`.

- [ ] **Step 1: Forward replies instead of dropping**

In `route_tmux_output`, the loop needs the connection + the pane's tmux `PaneId` (the `pane` key already in scope). Add `connection: NonSend<TmuxConnection>` to the system params. Replace the drop:

```rust
        handle.advance(&data);
        handle.flush_emit(&mut commands, entity);
        let replies = handle.take_replies();
        if !replies.is_empty()
            && let Some(c) = connection.client()
        {
            // pane id formats as `%<n>` for tmux targets.
            let target = format!("%{}", pane.0);
            if let Err(e) = c.handle().send(&send_bytes_command(&target, &replies)) {
                tracing::warn!(?e, "reply send-keys failed");
            }
        }
```
Confirm `PaneId`'s inner field / Display gives the `%N` form (check `tmux_control_parser::PaneId`); use whichever yields the tmux target string. Import `send_bytes_command`, `TmuxConnection`.

- [ ] **Step 2: build + clippy + fmt + commit**

```bash
cargo build && cargo clippy -p ozmux-gui --all-targets && cargo fmt
git add src/tmux_render.rs
git commit -m "feat(tmux): forward pane DSR/DA replies back to tmux"
```

---

## Task 7: Integration test + final verification

**Files:** `src/tmux_input.rs` (test) and/or `crates/tmux_session/tests/`.

- [ ] **Step 1: Headless plugin test (no real tmux)**

Add a test that a focused non-chord key produces a `send-keys -K` command and a GUI chord does not. Since sending requires a live `TmuxConnection`, test the PURE seam instead: assert `gui_chord(...)` classification (open-picker / quit / other-super-swallowed / None-for-plain) and that `send_keys_command` is built from a batch. (The mapper + builders are already unit-tested in Task 2; this adds the chord-classification test.) Put `gui_chord` + a small `classify` helper behind `pub(crate)` if needed for the test.

- [ ] **Step 2: Gated real-tmux integration test**

Add `crates/tmux_session/tests/real_tmux_input.rs` (mirroring `real_tmux_boot.rs`), `#[ignore = "requires a real tmux binary and a controlling PTY"]`: attach a `tmux -CC`, query the client name (`client_name_command`), `send-keys -K -c <client> a`, and assert the resulting `%output` contains `a`. This is the **verify-live** gate for `send-keys -K -c` + the client-name source.

- [ ] **Step 3: Full check**

Run: `cargo build`, `cargo clippy --workspace --all-targets`, `cargo fmt --check`, `cargo test -p ozma_tty_engine -p ozmux_tmux -p ozmux_configs`, and `cargo test -p ozmux-gui tmux_input` (filtered; the binary suite has a known CEF segfault under parallel threads — use `-- --test-threads=1` if running broadly).

- [ ] **Step 4: Manual GUI verification** (desktop; run from OUTSIDE the attached tmux session)

`cargo run`, pick a session, type into a pane → keystrokes appear (forwarded via tmux). Verify the tmux prefix works (e.g. `C-b c` creates a window — tmux's own binding fires). Verify `Cmd+Shift+P` reopens the picker and `Cmd+Q` quits. Note any gap (esp. the verify-live `-K -c` behavior).

- [ ] **Step 5: Commit any fixes**

```bash
git add -A && git commit -m "test(tmux): phase 3a keyboard input tests + verification" || echo "nothing to commit"
```

---

## Out of scope (3b / 3c / later)

- Re-targeting pane/window actions to tmux commands (`split-window`/`select-pane`/…) + workspace→window rename → **3b**.
- Click-to-focus (`select-pane`) + focus/dim → **3c**.
- IME text → tmux, mouse wheel/buttons → tmux, full clipboard copy, `list-keys` keybind mirror, detach/reconnect (Phase 4).
