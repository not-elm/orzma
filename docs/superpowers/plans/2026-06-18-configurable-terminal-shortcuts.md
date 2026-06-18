# Configurable terminal shortcuts Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the four ozmux-GUI-owned shortcuts (paste, release-inline-focus, open-picker, quit) editable from `~/.config/ozmux/config.toml`, wiring the existing-but-unused config schema into the runtime key dispatcher.

**Architecture:** Add `open-picker`/`quit` to the config `Bindings` schema. Resolve each configured logical chord to a physical `KeyCode` once at startup into a `ResolvedShortcuts` resource (translation lives in the binary so `ozmux_configs` stays bevy-free). The tmux keyboard dispatcher (`forward_keys_to_tmux`) matches incoming key events against that resolved table instead of the hardcoded `gui_chord()`.

**Tech Stack:** Rust 2024 (toolchain 1.95), Bevy 0.18 ECS, serde/toml, `ozmux_configs` crate, `ozmux_tmux` crate.

## Global Constraints

- Edition 2024, toolchain pinned to 1.95.
- No `mod.rs`: module files are `foo.rs` + `foo/bar.rs`.
- Comments only `// TODO:` / `// NOTE:` (critical caveat) / `// SAFETY:`; all comments in English.
- Doc comments (`///`) on every `pub` item; `//!` on every module file.
- All `use` at top of file, single contiguous block, no blank lines between import groups.
- Visibility minimized: items used only in their defining module are private; new resolution items are `pub(crate)` (consumed cross-module within the binary).
- Item ordering: `pub`/`pub(crate)` items before private items in a module/impl.
- Parameter ordering: mutable params before immutable (exception: the `interner` builder convention, not relevant here).
- `ozmux_configs` MUST NOT depend on `bevy`. Logical→physical key translation lives in `src/`.
- tmux key bindings (`crates/tmux_session/src/keybindings.rs`, `list-keys`) are NOT touched.
- Matching is layout-stable: physical `KeyCode` + exact `Modifiers` equality (`meta` ⇔ `super`).
- Load-at-startup only; no hot reload.

**Known test caveat (this repo):** a pre-existing IME test failure and a parallel-teardown SIGSEGV mean the *full* suite needs `cargo test -- --test-threads=1 --skip <flaky>` for a green run. Per-task commands below target specific crates/filters to stay fast and unaffected.

---

### Task 1: Add `open-picker` and `quit` to the shortcut config schema

**Files:**
- Modify: `crates/configs/src/shortcuts.rs` (`Bindings` struct, `Bindings::iter`, `Bindings::default`, `ShortcutAction` enum, tests)

**Interfaces:**
- Consumes: existing `KeyChord`, `parse_default_chord`, `deser_chord_or_unbind`.
- Produces:
  - `Bindings.open_picker: Option<KeyChord>` (default `Cmd+Shift+P`)
  - `Bindings.quit: Option<KeyChord>` (default `Cmd+Q`)
  - `ShortcutAction::OpenPicker`, `ShortcutAction::Quit`
  - `Bindings::iter()` yields 4 tuples `(&'static str, &Option<KeyChord>, ShortcutAction)`.

- [ ] **Step 1: Update the JSON snapshot test to expect four bindings (failing test first)**

In `crates/configs/src/shortcuts.rs`, replace the `expected` string inside `default_shortcuts_json_snapshot` with:

```rust
        let expected = r#"{"bindings":{"paste":{"key":"v","modifiers":{"ctrl":false,"shift":false,"alt":false,"meta":true}},"release-inline-focus":{"key":"Escape","modifiers":{"ctrl":true,"shift":true,"alt":false,"meta":false}},"open-picker":{"key":"p","modifiers":{"ctrl":false,"shift":true,"alt":false,"meta":true}},"quit":{"key":"q","modifiers":{"ctrl":false,"shift":false,"alt":false,"meta":true}}}}"#;
```

Also rename `iter_yields_2_entries` to `iter_yields_4_entries` and update its body:

```rust
    #[test]
    fn iter_yields_4_entries() {
        let b = Bindings::default();
        assert_eq!(b.iter().count(), 4);
    }
```

Add two new default-value tests after `bindings_default_paste_is_cmd_v`:

```rust
    #[test]
    fn bindings_default_open_picker_is_cmd_shift_p() {
        let b = Bindings::default();
        let chord = b.open_picker.as_ref().unwrap();
        assert_eq!(chord.key, Key::Char('p'));
        assert!(chord.modifiers.meta && chord.modifiers.shift);
        assert!(!chord.modifiers.ctrl && !chord.modifiers.alt);
    }

    #[test]
    fn bindings_default_quit_is_cmd_q() {
        let b = Bindings::default();
        let chord = b.quit.as_ref().unwrap();
        assert_eq!(chord.key, Key::Char('q'));
        assert!(chord.modifiers.meta);
        assert!(!chord.modifiers.ctrl && !chord.modifiers.shift && !chord.modifiers.alt);
    }
```

And extend `bindings_default_has_active_fields_some`:

```rust
    #[test]
    fn bindings_default_has_active_fields_some() {
        let b = Bindings::default();
        assert!(b.paste.is_some());
        assert!(b.release_inline_focus.is_some());
        assert!(b.open_picker.is_some());
        assert!(b.quit.is_some());
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p ozmux_configs shortcuts`
Expected: FAIL — `no field open_picker on type Bindings` / `no variant OpenPicker` (compile errors), and `iter_yields_4_entries` count mismatch.

- [ ] **Step 3: Add the two fields to `Bindings`**

In `crates/configs/src/shortcuts.rs`, immediately AFTER the `pub release_inline_focus: Option<KeyChord>,` field (and its doc comment) and BEFORE the deprecated `close_surface` block, insert:

```rust
    /// Opens the tmux session/window picker overlay.
    #[serde(deserialize_with = "deser_chord_or_unbind")]
    pub open_picker: Option<KeyChord>,
    /// Quits the ozmux application.
    #[serde(deserialize_with = "deser_chord_or_unbind")]
    pub quit: Option<KeyChord>,
```

- [ ] **Step 4: Seed the defaults in `Bindings::default`**

In the `impl Default for Bindings` block, immediately AFTER the `release_inline_focus: Some(parse_default_chord("Ctrl+Shift+Escape")),` line, insert:

```rust
            open_picker: Some(parse_default_chord("Cmd+Shift+P")),
            quit: Some(parse_default_chord("Cmd+Q")),
```

- [ ] **Step 5: Add the two `ShortcutAction` variants**

In `enum ShortcutAction`, after the `ReleaseInlineFocus,` variant, insert:

```rust
    /// Opens the tmux session/window picker overlay.
    OpenPicker,
    /// Quits the ozmux application.
    Quit,
```

- [ ] **Step 6: Extend `Bindings::iter` to yield all four**

Replace the array literal in `Bindings::iter` with:

```rust
        [
            ("paste", &self.paste, ShortcutAction::Paste),
            (
                "release-inline-focus",
                &self.release_inline_focus,
                ShortcutAction::ReleaseInlineFocus,
            ),
            ("open-picker", &self.open_picker, ShortcutAction::OpenPicker),
            ("quit", &self.quit, ShortcutAction::Quit),
        ]
        .into_iter()
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test -p ozmux_configs shortcuts`
Expected: PASS (all shortcut tests, including the updated snapshot, count, conflict, and new default tests).

- [ ] **Step 8: Commit**

```bash
git add crates/configs/src/shortcuts.rs
git commit -m "feat(configs): add open-picker and quit shortcut actions"
```

---

### Task 2: Add the logical→physical resolution layer

**Files:**
- Create: `src/input/shortcuts.rs`
- Modify: `src/input.rs` (declare the module)

**Interfaces:**
- Consumes: `ozmux_configs::shortcuts::{Bindings, Key, Modifiers, ShortcutAction}`, `Bindings::iter`, `KeyChord` Display, `bevy::prelude::KeyCode`, `crate::configs::OzmuxConfigsResource`.
- Produces:
  - `pub(crate) struct ResolvedShortcut { keycode: KeyCode, modifiers: Modifiers, action: ShortcutAction }`
  - `pub(crate) struct ResolvedShortcuts(pub(crate) Vec<ResolvedShortcut>)` (Bevy `Resource`, `Default`)
  - `ResolvedShortcuts::match_gui_action(&self, KeyCode, Modifiers) -> Option<ShortcutAction>` (excludes `ReleaseInlineFocus`)
  - `ResolvedShortcuts::is_release_inline_focus(&self, KeyCode, Modifiers) -> bool`
  - `pub(crate) fn resolve_from_bindings(&Bindings) -> Vec<ResolvedShortcut>`

> NOTE: `bevy::prelude` and `ozmux_configs::shortcuts` both export a type named `Key`. This module aliases the config one to `ConfigKey` to avoid the clash, since `bevy::prelude::*` is the established import idiom across `src/`.

- [ ] **Step 1: Declare the module in `src/input.rs`**

In `src/input.rs`, after the line `pub(crate) mod option_as_alt;`, add:

```rust
pub(crate) mod shortcuts;
```

- [ ] **Step 2: Write the new module with its failing tests**

Create `src/input/shortcuts.rs` with this exact content:

```rust
//! Resolves configured shortcut chords (logical keys) into physical
//! `KeyCode`-based entries the runtime input dispatcher matches against.
//! The translation lives here (not in `ozmux_configs`) so the config crate
//! stays free of any `bevy` dependency.

use crate::configs::OzmuxConfigsResource;
use bevy::prelude::*;
use ozmux_configs::shortcuts::{Bindings, Key as ConfigKey, Modifiers, ShortcutAction};

/// One configured shortcut resolved to a physical key: the `KeyCode` to match,
/// the exact modifier set required, and the action to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedShortcut {
    pub(crate) keycode: KeyCode,
    pub(crate) modifiers: Modifiers,
    pub(crate) action: ShortcutAction,
}

/// The startup-resolved ozmux shortcut table. Built once from
/// `OzmuxConfigsResource`; consumed by the tmux keyboard dispatcher.
#[derive(Resource, Default, Debug, Clone)]
pub(crate) struct ResolvedShortcuts(pub(crate) Vec<ResolvedShortcut>);

impl ResolvedShortcuts {
    /// Returns the GUI action bound to `(keycode, mods)`, if any. Excludes
    /// `ReleaseInlineFocus`, which is meaningful only while an inline webview
    /// holds focus and is matched separately via `is_release_inline_focus`.
    pub(crate) fn match_gui_action(
        &self,
        keycode: KeyCode,
        mods: Modifiers,
    ) -> Option<ShortcutAction> {
        self.0
            .iter()
            .filter(|s| s.action != ShortcutAction::ReleaseInlineFocus)
            .find(|s| s.keycode == keycode && s.modifiers == mods)
            .map(|s| s.action.clone())
    }

    /// True when `(keycode, mods)` matches the configured release-inline-focus
    /// chord.
    pub(crate) fn is_release_inline_focus(&self, keycode: KeyCode, mods: Modifiers) -> bool {
        self.0.iter().any(|s| {
            s.action == ShortcutAction::ReleaseInlineFocus
                && s.keycode == keycode
                && s.modifiers == mods
        })
    }
}

/// Resolves every bound chord in `bindings` to a `ResolvedShortcut`, skipping
/// (with a warning) any chord whose logical key has no physical `KeyCode`.
pub(crate) fn resolve_from_bindings(bindings: &Bindings) -> Vec<ResolvedShortcut> {
    let mut out = Vec::new();
    for (label, bound, action) in bindings.iter() {
        let Some(chord) = bound else { continue };
        match key_to_keycode(&chord.key) {
            Some(keycode) => out.push(ResolvedShortcut {
                keycode,
                modifiers: chord.modifiers.clone(),
                action,
            }),
            None => tracing::warn!(
                label,
                chord = %chord,
                "shortcut key has no physical KeyCode mapping; ignoring binding"
            ),
        }
    }
    out
}

/// Maps a config logical `Key` to the physical `KeyCode` ozmux matches on.
/// Returns `None` for keys with no stable physical mapping (`Plus`, `Other`,
/// non-alphanumeric chars).
fn key_to_keycode(key: &ConfigKey) -> Option<KeyCode> {
    Some(match key {
        ConfigKey::Char(c) => match c.to_ascii_lowercase() {
            'a' => KeyCode::KeyA,
            'b' => KeyCode::KeyB,
            'c' => KeyCode::KeyC,
            'd' => KeyCode::KeyD,
            'e' => KeyCode::KeyE,
            'f' => KeyCode::KeyF,
            'g' => KeyCode::KeyG,
            'h' => KeyCode::KeyH,
            'i' => KeyCode::KeyI,
            'j' => KeyCode::KeyJ,
            'k' => KeyCode::KeyK,
            'l' => KeyCode::KeyL,
            'm' => KeyCode::KeyM,
            'n' => KeyCode::KeyN,
            'o' => KeyCode::KeyO,
            'p' => KeyCode::KeyP,
            'q' => KeyCode::KeyQ,
            'r' => KeyCode::KeyR,
            's' => KeyCode::KeyS,
            't' => KeyCode::KeyT,
            'u' => KeyCode::KeyU,
            'v' => KeyCode::KeyV,
            'w' => KeyCode::KeyW,
            'x' => KeyCode::KeyX,
            'y' => KeyCode::KeyY,
            'z' => KeyCode::KeyZ,
            '0' => KeyCode::Digit0,
            '1' => KeyCode::Digit1,
            '2' => KeyCode::Digit2,
            '3' => KeyCode::Digit3,
            '4' => KeyCode::Digit4,
            '5' => KeyCode::Digit5,
            '6' => KeyCode::Digit6,
            '7' => KeyCode::Digit7,
            '8' => KeyCode::Digit8,
            '9' => KeyCode::Digit9,
            _ => return None,
        },
        ConfigKey::Escape => KeyCode::Escape,
        ConfigKey::Space => KeyCode::Space,
        ConfigKey::Enter => KeyCode::Enter,
        ConfigKey::Tab => KeyCode::Tab,
        ConfigKey::Backspace => KeyCode::Backspace,
        ConfigKey::ArrowUp => KeyCode::ArrowUp,
        ConfigKey::ArrowDown => KeyCode::ArrowDown,
        ConfigKey::ArrowLeft => KeyCode::ArrowLeft,
        ConfigKey::ArrowRight => KeyCode::ArrowRight,
        ConfigKey::Plus => return None,
        ConfigKey::Other(_) => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mods(ctrl: bool, shift: bool, alt: bool, meta: bool) -> Modifiers {
        Modifiers {
            ctrl,
            shift,
            alt,
            meta,
        }
    }

    #[test]
    fn char_letter_maps_to_keycode_case_insensitive() {
        assert_eq!(key_to_keycode(&ConfigKey::Char('v')), Some(KeyCode::KeyV));
        assert_eq!(key_to_keycode(&ConfigKey::Char('P')), Some(KeyCode::KeyP));
    }

    #[test]
    fn digit_maps_to_keycode() {
        assert_eq!(key_to_keycode(&ConfigKey::Char('1')), Some(KeyCode::Digit1));
    }

    #[test]
    fn named_keys_map() {
        assert_eq!(key_to_keycode(&ConfigKey::Escape), Some(KeyCode::Escape));
        assert_eq!(key_to_keycode(&ConfigKey::ArrowUp), Some(KeyCode::ArrowUp));
    }

    #[test]
    fn unmappable_keys_are_none() {
        assert_eq!(key_to_keycode(&ConfigKey::Plus), None);
        assert_eq!(key_to_keycode(&ConfigKey::Other("f12".into())), None);
    }

    #[test]
    fn default_bindings_resolve_to_four() {
        let r = ResolvedShortcuts(resolve_from_bindings(&Bindings::default()));
        assert_eq!(r.0.len(), 4);
    }

    #[test]
    fn match_gui_action_resolves_defaults() {
        let r = ResolvedShortcuts(resolve_from_bindings(&Bindings::default()));
        assert_eq!(
            r.match_gui_action(KeyCode::KeyV, mods(false, false, false, true)),
            Some(ShortcutAction::Paste)
        );
        assert_eq!(
            r.match_gui_action(KeyCode::KeyP, mods(false, true, false, true)),
            Some(ShortcutAction::OpenPicker)
        );
        assert_eq!(
            r.match_gui_action(KeyCode::KeyQ, mods(false, false, false, true)),
            Some(ShortcutAction::Quit)
        );
    }

    #[test]
    fn match_gui_action_requires_exact_modifiers() {
        let r = ResolvedShortcuts(resolve_from_bindings(&Bindings::default()));
        assert_eq!(
            r.match_gui_action(KeyCode::KeyV, mods(false, true, false, true)),
            None
        );
        assert_eq!(
            r.match_gui_action(KeyCode::KeyQ, mods(false, true, false, true)),
            None
        );
    }

    #[test]
    fn match_gui_action_excludes_release_inline_focus() {
        let r = ResolvedShortcuts(resolve_from_bindings(&Bindings::default()));
        assert_eq!(
            r.match_gui_action(KeyCode::Escape, mods(true, true, false, false)),
            None
        );
    }

    #[test]
    fn unmatched_chord_is_none() {
        let r = ResolvedShortcuts(resolve_from_bindings(&Bindings::default()));
        assert_eq!(
            r.match_gui_action(KeyCode::KeyH, mods(false, false, false, true)),
            None
        );
        assert_eq!(
            r.match_gui_action(KeyCode::KeyA, mods(false, false, false, false)),
            None
        );
    }

    #[test]
    fn is_release_inline_focus_matches_default_chord() {
        let r = ResolvedShortcuts(resolve_from_bindings(&Bindings::default()));
        assert!(r.is_release_inline_focus(KeyCode::Escape, mods(true, true, false, false)));
        assert!(!r.is_release_inline_focus(KeyCode::KeyV, mods(false, false, false, true)));
    }
}
```

- [ ] **Step 3: Run the new module's tests to verify they pass**

Run: `cargo test --bin ozmux-gui input::shortcuts`
Expected: PASS (all `input::shortcuts::tests::*`). Compiles the GUI binary's test harness; requires CEF provisioned (`make setup-cef`).

> NOTE: `build_resolved_shortcuts` is intentionally NOT defined here yet — it would be unused (dead code) until Task 3 registers it. The pure logic above is fully exercised by the tests in this task.

- [ ] **Step 4: Commit**

```bash
git add src/input.rs src/input/shortcuts.rs
git commit -m "feat(input): add startup shortcut resolution layer (logical key -> KeyCode)"
```

---

### Task 3: Wire the resolved table into the runtime dispatcher

**Files:**
- Modify: `src/input/shortcuts.rs` (add the `build_resolved_shortcuts` startup system)
- Modify: `src/input.rs` (`OzmuxShortcutPlugin::build` — init resource + register startup system)
- Modify: `src/tmux_input.rs` (consume `ResolvedShortcuts`, replace both dispatch sites, delete `gui_chord`/`GuiChord` and their tests)

**Interfaces:**
- Consumes: `ResolvedShortcuts`, `ResolvedShortcuts::match_gui_action`, `ResolvedShortcuts::is_release_inline_focus`, `resolve_from_bindings` (Task 2); `ShortcutAction`, `Modifiers` (Task 1); `crate::configs::OzmuxConfigsResource`.
- Produces: `pub(crate) fn build_resolved_shortcuts(Commands, Res<OzmuxConfigsResource>)` (Bevy `Startup` system).

- [ ] **Step 1: Add the startup system to `src/input/shortcuts.rs`**

Insert this function immediately AFTER `resolve_from_bindings` and BEFORE `fn key_to_keycode` (keeps `pub(crate)` items above the private helper):

```rust
/// `Startup` system: resolves the configured shortcut bindings into
/// `ResolvedShortcuts`, replacing the empty default inserted at plugin build.
pub(crate) fn build_resolved_shortcuts(
    mut commands: Commands,
    configs: Res<OzmuxConfigsResource>,
) {
    commands.insert_resource(ResolvedShortcuts(resolve_from_bindings(
        &configs.shortcuts.bindings,
    )));
}
```

- [ ] **Step 2: Register the resource and startup system in `OzmuxShortcutPlugin`**

In `src/input.rs`, replace the body of `impl Plugin for OzmuxShortcutPlugin`'s `build` so it reads:

```rust
    fn build(&self, app: &mut App) {
        app.init_resource::<shortcuts::ResolvedShortcuts>()
            .add_systems(Startup, shortcuts::build_resolved_shortcuts)
            .configure_sets(
                Update,
                (
                    InputPhase::Hover,
                    InputPhase::Dispatch,
                    InputPhase::FocusedKey,
                )
                    .chain()
                    .in_set(OzmuxSystems::Input),
            );
    }
```

- [ ] **Step 3: Add imports to `src/tmux_input.rs`**

In the top import block of `src/tmux_input.rs`, add (keep the block contiguous, no blank lines between groups):

```rust
use crate::input::shortcuts::ResolvedShortcuts;
```

and add `ShortcutAction` + `Modifiers` from the config crate:

```rust
use ozmux_configs::shortcuts::{Modifiers, ShortcutAction};
```

- [ ] **Step 4: Add the `resolved` param to `forward_keys_to_tmux`**

In the parameter list of `fn forward_keys_to_tmux`, add the following line immediately after `bindings: Res<KeyBindings>,` (it is an immutable `Res`, so it belongs in the immutable group):

```rust
    resolved: Res<ResolvedShortcuts>,
```

- [ ] **Step 5: Derive the config `Modifiers` once per frame**

In `forward_keys_to_tmux`, immediately AFTER the `let mods = KeyMods { ... };` block (the one ending `};` around the `super_:` line), insert:

```rust
    let cfg_mods = Modifiers {
        ctrl: mods.ctrl,
        shift: mods.shift,
        alt: mods.alt,
        meta: mods.super_,
    };
```

- [ ] **Step 6: Replace the release-inline-focus condition**

In the `if focused_webview.0.is_some()` branch, replace the inner `if` condition:

```rust
            if ev.state == ButtonState::Pressed
                && ev.key_code == KeyCode::Escape
                && mods.ctrl
                && mods.shift
            {
```

with:

```rust
            if ev.state == ButtonState::Pressed
                && resolved.is_release_inline_focus(ev.key_code, cfg_mods)
            {
```

- [ ] **Step 7: Replace the GUI-chord dispatch in the main key loop**

In the `for ev in events.read()` loop, replace the whole block that begins `if let Some(chord) = gui_chord(&ev.key_code, mods) {` and ends with its closing `}` + `continue;` (the `match chord { ... }` block) with:

```rust
        if let Some(action) = resolved.match_gui_action(ev.key_code, cfg_mods) {
            // A GUI action abandons any pending tmux prefix sequence.
            *prefix_pending = false;
            match action {
                ShortcutAction::OpenPicker => picker.open = true,
                ShortcutAction::Quit => {
                    exit.write(AppExit::Success);
                }
                ShortcutAction::Paste => {
                    let Some(text) = clipboard.read() else {
                        continue;
                    };
                    if text.is_empty() {
                        continue;
                    }
                    let (Some(target), Some(client)) = (target.as_deref(), connection.client())
                    else {
                        continue;
                    };
                    let bytes = build_paste_bytes(&text, false);
                    for chunk in bytes.chunks(PASTE_CHUNK_BYTES) {
                        if let Err(e) = client.handle().send(&send_bytes_command(target, chunk)) {
                            tracing::warn!(?e, "paste send failed");
                            break;
                        }
                    }
                }
                ShortcutAction::ReleaseInlineFocus => {}
            }
            continue;
        }
        // Any Cmd/Super-modified key that matched no ozmux shortcut is swallowed:
        // tmux/PTY has no Super modifier, so it must never be forwarded.
        if mods.super_ {
            *prefix_pending = false;
            continue;
        }
```

- [ ] **Step 8: Delete the now-dead `gui_chord` function and `GuiChord` enum**

Delete the `enum GuiChord { ... }` definition (the `OpenPicker / Quit / Paste / Other` enum near the top of the file) and the entire `fn gui_chord(key_code: &KeyCode, mods: KeyMods) -> Option<GuiChord> { ... }` function.

- [ ] **Step 9: Delete the obsolete `gui_chord` tests and the `m()` helper**

In the `#[cfg(test)] mod tests` block of `src/tmux_input.rs`, delete the helper `fn m(shift: bool, super_: bool) -> KeyMods { ... }` and these five tests (their behavior is now covered by `input::shortcuts::tests`): `cmd_shift_p_opens_picker`, `cmd_q_quits`, `cmd_v_is_paste`, `other_super_chord_is_swallowed`, `non_super_key_is_not_a_chord`.

- [ ] **Step 10: Build and run the affected tests**

Run: `cargo build --bin ozmux-gui`
Expected: SUCCESS, no `gui_chord`/`GuiChord` unresolved references, no unused-import warnings.

Run: `cargo test --bin ozmux-gui input::shortcuts && cargo test --bin ozmux-gui tmux_input`
Expected: PASS (resolution-layer tests and the remaining tmux_input tests).

- [ ] **Step 11: Commit**

```bash
git add src/input.rs src/input/shortcuts.rs src/tmux_input.rs
git commit -m "feat(input): drive ozmux GUI shortcuts from config instead of hardcoding"
```

---

### Task 4: Final integration verification

**Files:** none (verification only)

**Interfaces:** none.

- [ ] **Step 1: Workspace lint + format**

Run: `cargo clippy --workspace --all-targets && cargo fmt --check`
Expected: clippy clean (no warnings on changed crates), fmt reports no diffs.

- [ ] **Step 2: Config crate full tests**

Run: `cargo test -p ozmux_configs`
Expected: PASS.

- [ ] **Step 3: Binary unit tests (serialized to dodge the known SIGSEGV/IME flakiness)**

Run: `cargo test --bin ozmux-gui -- --test-threads=1`
Expected: PASS, except the pre-existing IME failure documented in the repo memory. If that single known failure appears, re-run skipping it; no NEW failures are acceptable.

- [ ] **Step 4: Manual GUI smoke (human-run; cannot be automated here)**

Document for the reviewer to verify in a real session:
1. Default config: `Cmd+V` pastes, `Cmd+Shift+P` opens the picker, `Cmd+Q` quits, `Ctrl+Shift+Escape` releases inline-webview focus — all unchanged.
2. Edit `~/.config/ozmux/config.toml`:
   ```toml
   [shortcuts.bindings]
   quit = "Cmd+Shift+Q"
   open-picker = "Cmd+K"
   ```
   Restart ozmux. Confirm the new chords work and the old ones (`Cmd+Q`, `Cmd+Shift+P`) no longer trigger those actions (`Cmd+Q` is swallowed, not forwarded to tmux).
3. Set `quit = ""` (unbind), restart, confirm `Cmd+Q` no longer quits.
4. Introduce a duplicate (`quit = "Cmd+V"`), restart, confirm ozmux exits with code 2 and prints the duplicate-chord error.

- [ ] **Step 5: Final confirmation**

No code changes in this task. If Steps 1–3 are green, the feature is complete; flag the manual smoke (Step 4) as the remaining human gate.

---

## Self-Review

**Spec coverage:**
- Config schema (`open-picker`, `quit`, `ShortcutAction`, `iter`, defaults, conflict validation) → Task 1.
- Resolution layer (`key_to_keycode`, `ResolvedShortcuts`, matchers, `resolve_from_bindings`) → Task 2.
- Runtime wiring (`build_resolved_shortcuts` startup system, `match_gui_action` in the main loop, `is_release_inline_focus` in the webview branch, Cmd-swallow safety net, delete `gui_chord`) → Task 3.
- Non-goals (no hot reload, no tmux-conflict detection, no new actions, layout-stable matching preserved) → respected; verified in Task 4 manual smoke.
- `Plus`/`Other` resolve to `None` (skipped) → Task 2 `key_to_keycode` + `unmappable_keys_are_none` test.

**Placeholder scan:** No TBD/TODO; all code blocks are complete (including all 26 letter + 10 digit arms).

**Type consistency:** `match_gui_action`/`is_release_inline_focus`/`resolve_from_bindings`/`build_resolved_shortcuts`/`ResolvedShortcuts` names and signatures are identical across Tasks 2–3. `Modifiers` field order (`ctrl, shift, alt, meta`) matches `ozmux_configs::shortcuts::Modifiers`. `ShortcutAction` variants (`Paste`, `ReleaseInlineFocus`, `OpenPicker`, `Quit`) are matched exhaustively in Task 3 Step 7.
