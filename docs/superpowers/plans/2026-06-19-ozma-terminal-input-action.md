# ozma_terminal Internalized Input + Action — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `crates/ozma_terminal` a self-contained terminal — a consumer spawns one bundle and gets working key input + paste with no input wiring — by internalizing the default keyboard dispatcher (`input`) and PTY-level actions (`action`), behind an `InputDisabled` marker and a host-supplied `TerminalInputBindings` seam.

**Architecture:** The engine (`ozma_tty_engine`) already owns raw-key→PTY via the `TerminalKeyInput` EntityEvent + observer. This plan adds, in the crate: a `clipboard` module (moved from the binary), an `action` module (`PasteAction` EntityEvent + observer), an `input` module (a `KeyboardInput` dispatcher that fires `PasteAction`/`TerminalKeyInput` or skips host-reserved chords), and a self-contained `OzmaTerminalBundle::spawn`. The binary keeps app-level shortcuts (Quit/OpenPicker/DetachSession/ReleaseInlineFocus) and maintains the `InputDisabled` marker from its coarse guards.

**Tech Stack:** Rust edition 2024 (toolchain 1.95), Bevy 0.18 ECS, `arboard` clipboard, `alacritty_terminal` VT (via `ozma_tty_engine`).

## Global Constraints

- No `mod.rs`: a module is `foo.rs` + optional `foo/` dir.
- Comments: only `// TODO:` / `// NOTE:` (critical caveats only) / `// SAFETY:`. All comments in English. No commented-out code, no narrative comments.
- Doc comments (`///`) required on every externally-`pub` item; `//!` on every module file.
- All `use` at the top of the file, one contiguous block (no blank lines between groups). No inline fully-qualified paths in signatures/bodies. No glob imports in consumer code. Exception: `use super::*;` inside `#[cfg(test)] mod tests`.
- Visibility minimized: private by default; widen only for real cross-module callers. `pub(crate)` for crate-internal cross-module items; `pub` only for the crate's external API.
- Item ordering: `pub` items before private; private helpers last.
- Parameter ordering: mutable params (`mut x`, `&mut`, `Commands`, `ResMut`, `Query<&mut …>`) before immutable. Exception: a fixed leading `On<E>` trigger or `&self`.
- System gating: use `run_if(...)` run conditions, not in-body `is_changed()`/`is_added()` early returns.
- Change detection: mutate conditionally so normal `DerefMut` drives it; no `set_changed()` / `bypass_change_detection()`.
- `Plugin::build` bodies: a single `app.` method chain.
- `Query` params: descriptive noun, never a `_q` suffix.
- Lint/format gate per task: `cargo clippy --workspace --all-targets` clean and `cargo fmt` applied.

---

### Task 1: Move `clipboard` into the `ozma_terminal` crate

**Files:**
- Create: `crates/ozma_terminal/src/clipboard.rs` (moved from `src/clipboard.rs`)
- Modify: `crates/ozma_terminal/Cargo.toml` (add `arboard`)
- Modify: `crates/ozma_terminal/src/lib.rs` (declare + re-export `clipboard`)
- Delete: `src/clipboard.rs`
- Modify: `src/main.rs:4` (drop `mod clipboard;`)
- Modify (re-point imports): `src/ozma_input.rs:8`, `src/tmux/mouse.rs:17`, `src/tmux/input.rs:10`, `src/tmux/copy_mode.rs:16,841,1022`, `src/ui/copy_mode.rs:8`
- Test: `crates/ozma_terminal/src/clipboard.rs` (moved tests run under the crate)

**Interfaces:**
- Produces: `ozma_terminal::Clipboard` (pub Resource: `new()`, `read(&mut self) -> Option<String>`, `write(&mut self, String)`); `ozma_terminal::build_paste_bytes(text: &str, bracketed: bool) -> Vec<u8>` (pub).
- Consumes: nothing new.

- [ ] **Step 1: Add `arboard` to the crate manifest**

In `crates/ozma_terminal/Cargo.toml`, under `[dependencies]`, add the line (keep the existing aligned style):

```toml
arboard           = { workspace = true }
```

Verify `arboard` is a workspace dependency (it is — used by the root crate). Run:

```bash
rg -n '^arboard' Cargo.toml
```
Expected: a line like `arboard = "..."` (or under `[workspace.dependencies]`). If it is NOT in `[workspace.dependencies]`, instead copy the exact version spec from the root `Cargo.toml`'s `[dependencies] arboard = ...` into the crate manifest.

- [ ] **Step 2: Create the crate clipboard module (verbatim move, `build_paste_bytes` → `pub`)**

Copy the entire current contents of `src/clipboard.rs` into `crates/ozma_terminal/src/clipboard.rs`, changing **only** the visibility of `build_paste_bytes` from `pub(crate)` to `pub` and adding a doc comment. The file begins:

```rust
//! Clipboard Bevy Resource wrapping `arboard::Clipboard`, plus `build_paste_bytes`,
//! the pure helper that turns clipboard text into the PTY byte stream.

use bevy::ecs::resource::Resource;
```

Keep `Clipboard` (already `pub`), its `Default`/`new`/`write`/`read` impls, and the `#[cfg(test)] mod tests` block unchanged. Change the helper signature line to:

```rust
/// Constructs the byte sequence that `TerminalHandle::write` should send to
/// the PTY for a paste of `text`. See the module docs for the bracketed vs.
/// unbracketed normalization rules.
pub fn build_paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
```

- [ ] **Step 3: Declare and re-export the module in `lib.rs`**

In `crates/ozma_terminal/src/lib.rs`, add `mod clipboard;` to the module declarations (alphabetical with the others: after `mod action;`/before `mod exit;` — adjust to existing order) and add to the re-export block:

```rust
pub use clipboard::{Clipboard, build_paste_bytes};
```

(The existing `pub use spawn::{...}` line stays; keep all `pub use` lines contiguous.)

- [ ] **Step 4: Delete the binary module and re-point imports**

Delete `src/clipboard.rs`. In `src/main.rs`, remove the line `mod clipboard;` (line 4).

Re-point every binary import. In each listed file, replace the `crate::clipboard::` path with `ozma_terminal::`:

- `src/ozma_input.rs:8` → `use ozma_terminal::{Clipboard, build_paste_bytes};`
- `src/tmux/mouse.rs:17` → `use ozma_terminal::Clipboard;`
- `src/tmux/input.rs:10` → `use ozma_terminal::{Clipboard, build_paste_bytes};`
- `src/tmux/copy_mode.rs:16` → `use ozma_terminal::Clipboard;`
- `src/ui/copy_mode.rs:8` → `use ozma_terminal::Clipboard;`

For the two **inline** `use crate::clipboard::Clipboard;` statements inside `src/tmux/copy_mode.rs` (lines 841, 1022): if they are inside `#[cfg(test)]` code, change them to `use ozma_terminal::Clipboard;`; if they are inside non-test function bodies, hoist them to the file's top `use` block as `use ozma_terminal::Clipboard;` and delete the inline statements (the top-of-file rule). Verify none remain:

```bash
rg -n "crate::clipboard" src/ ; echo "exit: $?"
```
Expected: no matches (rg exits non-zero / prints nothing).

- [ ] **Step 5: Build and run the moved tests**

Run:

```bash
cargo build 2>&1 | tail -5
cargo test -p ozma_terminal build_paste_bytes 2>&1 | tail -15
```
Expected: build succeeds; the `build_paste_bytes_*` and `read_returns_none_*` tests pass under `ozma_terminal`.

- [ ] **Step 6: Lint, format, commit**

```bash
cargo clippy --workspace --all-targets 2>&1 | tail -5
cargo fmt
git add -A
git commit -m "refactor(ozma_terminal): move Clipboard + build_paste_bytes into the crate"
```

---

### Task 2: `action` module — `PasteAction` + observer

**Files:**
- Create: `crates/ozma_terminal/src/action.rs`
- Modify: `crates/ozma_terminal/src/lib.rs` (declare `mod action;`, re-export `PasteAction`, add `OzmaActionPlugin` to `OzmaTerminalPlugin`)
- Test: `crates/ozma_terminal/src/action.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `Clipboard`, `build_paste_bytes` (Task 1); `TerminalHandle`, `PtyHandle`, `Coalescer` (from `ozma_tty_engine`).
- Produces: `ozma_terminal::PasteAction { entity: Entity }` (pub `EntityEvent`); `OzmaActionPlugin` (pub(crate) plugin registering `on_paste`).

- [ ] **Step 1: Write the failing test**

Create `crates/ozma_terminal/src/action.rs` with the test first:

```rust
//! PTY-level terminal actions as `EntityEvent`s. Each action is one event +
//! one observer, aggregated by `OzmaActionPlugin`. The first citizen is
//! `PasteAction`; mouse-driven scroll/copy actions join with the mouse module.

use crate::clipboard::{Clipboard, build_paste_bytes};
use crate::spawn::OzmaTerminal;
use bevy::prelude::*;
use ozma_tty_engine::{Coalescer, PtyHandle, TerminalHandle};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paste_action_on_entity_without_terminal_does_not_panic() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(Clipboard::new())
            .add_plugins(OzmaActionPlugin);
        let entity = app.world_mut().spawn_empty().id();
        app.world_mut().trigger(PasteAction { entity });
        app.update();
        // Reaching here proves the observer handled the missing-terminal and
        // unavailable/empty-clipboard paths without panicking. Byte correctness
        // is covered by the clipboard `build_paste_bytes_*` tests.
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ozma_terminal paste_action_on_entity_without_terminal -v 2>&1 | tail -15`
Expected: FAIL to compile — `PasteAction` / `OzmaActionPlugin` not found.

- [ ] **Step 3: Write minimal implementation**

Insert, between the `use` block and the `#[cfg(test)]` module:

```rust
/// Pastes the system clipboard into the target terminal entity's PTY.
#[derive(EntityEvent, Debug, Clone)]
pub struct PasteAction {
    /// The terminal entity to paste into.
    #[event_target]
    pub entity: Entity,
}

/// Registers the crate's PTY-level action observers.
pub(crate) struct OzmaActionPlugin;

impl Plugin for OzmaActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_paste);
    }
}

fn on_paste(
    ev: On<PasteAction>,
    mut clipboard: ResMut<Clipboard>,
    mut terminals: Query<(&mut TerminalHandle, &mut PtyHandle, &mut Coalescer), With<OzmaTerminal>>,
) {
    let Some(text) = clipboard.read() else {
        return;
    };
    if text.is_empty() {
        return;
    }
    let Ok((mut handle, mut pty, mut coalescer)) = terminals.get_mut(ev.entity) else {
        return;
    };
    if !handle.is_at_bottom() {
        handle.scroll_to_bottom(&mut coalescer);
    }
    let bracketed = handle.bracketed_paste_enabled();
    let bytes = build_paste_bytes(&text, bracketed);
    if let Err(e) = handle.write(&mut pty, &bytes) {
        tracing::warn!(?e, entity = ?ev.entity, "ozma paste write failed");
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p ozma_terminal paste_action_on_entity_without_terminal -v 2>&1 | tail -15`
Expected: PASS.

- [ ] **Step 5: Wire into `lib.rs`**

In `crates/ozma_terminal/src/lib.rs`: add `mod action;`, add `pub use action::PasteAction;` to the re-export block, and add `OzmaActionPlugin` to the `OzmaTerminalPlugin::build` plugin tuple:

```rust
use crate::action::OzmaActionPlugin;
```

and change the plugin chain to:

```rust
app.insert_resource(OzmaTerminalConfig {
    shell: self.config_shell.clone(),
})
.add_plugins((ExitPlugin, LayoutPlugin, OzmaActionPlugin));
```

(`OzmaActionPlugin` is dormant until Task 6 wires the dispatcher that triggers `PasteAction`.)

- [ ] **Step 6: Build, lint, format, commit**

```bash
cargo test -p ozma_terminal 2>&1 | tail -8
cargo clippy --workspace --all-targets 2>&1 | tail -5
cargo fmt
git add -A
git commit -m "feat(ozma_terminal): add PasteAction EntityEvent + OzmaActionPlugin"
```

---

### Task 3: `input` module — dispatcher, `InputDisabled`, `TerminalInputBindings`

**Files:**
- Create: `crates/ozma_terminal/src/input.rs`
- Modify: `crates/ozma_terminal/src/lib.rs` (declare `mod input;`, re-export public types — but do NOT add `OzmaInputPlugin` to `OzmaTerminalPlugin` yet; that happens in Task 6)
- Test: `crates/ozma_terminal/src/input.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `OzmaTerminal` (`spawn.rs`); `PasteAction` (Task 2); `TerminalKey`, `TerminalKeyInput`, `TerminalModifiers` (from `ozma_tty_engine`).
- Produces:
  - `ozma_terminal::InputDisabled` (pub marker Component).
  - `ozma_terminal::ReservedChord { key_code: KeyCode, ctrl: bool, shift: bool, alt: bool, meta: bool }` (pub, `Clone, Copy, PartialEq, Eq`).
  - `ozma_terminal::TerminalInputBindings { paste: ReservedChord, reserved: Vec<ReservedChord> }` (pub Resource; `Default` = `Cmd+V` paste + empty reserved).
  - `ozma_terminal::OzmaTerminalInputSet` (pub `SystemSet`).
  - `OzmaInputPlugin` (pub(crate) plugin; registers `TerminalInputBindings` default + the dispatcher in `OzmaTerminalInputSet`).

- [ ] **Step 1: Write the failing tests**

Create `crates/ozma_terminal/src/input.rs`:

```rust
//! Default terminal keyboard dispatcher. Reads `KeyboardInput` and, per press,
//! fires `PasteAction`, forwards a raw key as `TerminalKeyInput`, or skips it:
//! host-reserved chords and unhandled meta/Cmd chords are dropped. Gated per
//! entity by the `InputDisabled` marker.

use crate::action::PasteAction;
use crate::spawn::OzmaTerminal;
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;
use ozma_tty_engine::{TerminalKey, TerminalKeyInput, TerminalModifiers};

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Resource, Default)]
    struct Captured {
        paste: u32,
        keys: Vec<TerminalKey>,
    }

    fn test_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<KeyboardInput>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<Captured>()
            .add_plugins(OzmaInputPlugin)
            .add_observer(|ev: On<PasteAction>, mut c: ResMut<Captured>| {
                let _ = ev;
                c.paste += 1;
            })
            .add_observer(|ev: On<TerminalKeyInput>, mut c: ResMut<Captured>| {
                c.keys.push(ev.key.clone());
            });
        app
    }

    fn press(app: &mut App, key_code: KeyCode, logical: Key) {
        app.world_mut().write_message(KeyboardInput {
            key_code,
            logical_key: logical,
            state: ButtonState::Pressed,
            text: None,
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
    }

    fn hold_meta(app: &mut App) {
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::SuperLeft);
    }

    #[test]
    fn plain_key_forwards_as_terminal_key() {
        let mut app = test_app();
        app.world_mut().spawn(OzmaTerminal);
        press(&mut app, KeyCode::KeyA, Key::Character("a".into()));
        app.update();
        let c = app.world().resource::<Captured>();
        assert_eq!(c.keys, vec![TerminalKey::Text("a".into())]);
        assert_eq!(c.paste, 0);
    }

    #[test]
    fn paste_chord_fires_paste_action() {
        let mut app = test_app();
        app.world_mut().spawn(OzmaTerminal);
        hold_meta(&mut app);
        press(&mut app, KeyCode::KeyV, Key::Character("v".into()));
        app.update();
        let c = app.world().resource::<Captured>();
        assert_eq!(c.paste, 1);
        assert!(c.keys.is_empty());
    }

    #[test]
    fn reserved_chord_is_skipped() {
        let mut app = test_app();
        app.world_mut().spawn(OzmaTerminal);
        app.world_mut()
            .resource_mut::<TerminalInputBindings>()
            .reserved = vec![ReservedChord {
            key_code: KeyCode::KeyQ,
            ctrl: false,
            shift: false,
            alt: false,
            meta: true,
        }];
        hold_meta(&mut app);
        press(&mut app, KeyCode::KeyQ, Key::Character("q".into()));
        app.update();
        let c = app.world().resource::<Captured>();
        assert_eq!(c.paste, 0);
        assert!(c.keys.is_empty());
    }

    #[test]
    fn unhandled_meta_chord_is_dropped() {
        let mut app = test_app();
        app.world_mut().spawn(OzmaTerminal);
        hold_meta(&mut app);
        press(&mut app, KeyCode::KeyJ, Key::Character("j".into()));
        app.update();
        let c = app.world().resource::<Captured>();
        assert_eq!(c.paste, 0);
        assert!(c.keys.is_empty(), "Cmd+J must not reach the PTY");
    }

    #[test]
    fn input_disabled_entity_fires_nothing() {
        let mut app = test_app();
        app.world_mut().spawn((OzmaTerminal, InputDisabled));
        press(&mut app, KeyCode::KeyA, Key::Character("a".into()));
        app.update();
        let c = app.world().resource::<Captured>();
        assert!(c.keys.is_empty());
        assert_eq!(c.paste, 0);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ozma_terminal --lib input:: 2>&1 | tail -15`
Expected: FAIL to compile — `OzmaInputPlugin`, `TerminalInputBindings`, `ReservedChord`, `InputDisabled` not found.

- [ ] **Step 3: Write the types and plugin**

Insert, between the `use` block and `#[cfg(test)]`:

```rust
/// When present on an `OzmaTerminal` entity, the crate's default input
/// dispatcher skips it entirely — the host routes input elsewhere (tmux, a
/// focused webview, an open picker, IME composition).
#[derive(Component)]
pub struct InputDisabled;

/// A keyboard chord, as a physical `KeyCode` plus the four modifier bits.
/// Config-agnostic plain data the host supplies in `TerminalInputBindings`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ReservedChord {
    /// The physical key.
    pub key_code: KeyCode,
    /// Control held.
    pub ctrl: bool,
    /// Shift held.
    pub shift: bool,
    /// Alt/Option held.
    pub alt: bool,
    /// Meta/Cmd/Super held.
    pub meta: bool,
}

/// Host-supplied input policy: the chord that triggers the built-in Paste
/// action, plus the chords the dispatcher must skip (the host handles those).
/// Both are populated together so the "paste is not also reserved" invariant
/// lives in one place.
///
/// `Default` is `Cmd+V` paste + empty reserved, so a spawn-and-go consumer
/// still gets working paste and forwards everything else.
#[derive(Resource)]
pub struct TerminalInputBindings {
    /// The chord that triggers `PasteAction`.
    pub paste: ReservedChord,
    /// Chords the dispatcher skips for the host to handle.
    pub reserved: Vec<ReservedChord>,
}

impl Default for TerminalInputBindings {
    fn default() -> Self {
        Self {
            paste: ReservedChord {
                key_code: KeyCode::KeyV,
                ctrl: false,
                shift: false,
                alt: false,
                meta: true,
            },
            reserved: Vec::new(),
        }
    }
}

/// System set containing the default terminal keyboard dispatcher. Hosts that
/// maintain `InputDisabled` should schedule their maintainer
/// `.before(OzmaTerminalInputSet)`.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct OzmaTerminalInputSet;

/// Registers `TerminalInputBindings` and the default keyboard dispatcher.
pub(crate) struct OzmaInputPlugin;

impl Plugin for OzmaInputPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TerminalInputBindings>().add_systems(
            Update,
            dispatch_input
                .in_set(OzmaTerminalInputSet)
                .run_if(on_message::<KeyboardInput>),
        );
    }
}

fn dispatch_input(
    mut commands: Commands,
    mut events: MessageReader<KeyboardInput>,
    bindings: Res<TerminalInputBindings>,
    keys: Res<ButtonInput<KeyCode>>,
    terminals: Query<Entity, (With<OzmaTerminal>, Without<InputDisabled>)>,
) {
    let Ok(entity) = terminals.single() else {
        events.clear();
        return;
    };
    let mods = current_terminal_modifiers(&keys);
    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        if bindings
            .reserved
            .iter()
            .any(|c| chord_matches(c, ev.key_code, &mods))
        {
            continue;
        }
        if chord_matches(&bindings.paste, ev.key_code, &mods) {
            commands.trigger(PasteAction { entity });
            continue;
        }
        if mods.meta {
            continue;
        }
        if let Some(key) = bevy_key_to_terminal_key(&ev.logical_key) {
            commands.trigger(TerminalKeyInput {
                entity,
                key,
                modifiers: mods,
            });
        }
    }
}

fn current_terminal_modifiers(keys: &ButtonInput<KeyCode>) -> TerminalModifiers {
    TerminalModifiers {
        ctrl: keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight),
        shift: keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight),
        alt: keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight),
        meta: keys.pressed(KeyCode::SuperLeft) || keys.pressed(KeyCode::SuperRight),
    }
}

fn chord_matches(chord: &ReservedChord, key_code: KeyCode, mods: &TerminalModifiers) -> bool {
    chord.key_code == key_code
        && chord.ctrl == mods.ctrl
        && chord.shift == mods.shift
        && chord.alt == mods.alt
        && chord.meta == mods.meta
}

fn bevy_key_to_terminal_key(logical_key: &Key) -> Option<TerminalKey> {
    match logical_key {
        Key::Character(s) => Some(TerminalKey::Text(s.to_string())),
        Key::Space => Some(TerminalKey::Text(" ".to_string())),
        Key::Enter => Some(TerminalKey::Enter),
        Key::Backspace => Some(TerminalKey::Backspace),
        Key::Tab => Some(TerminalKey::Tab),
        Key::Escape => Some(TerminalKey::Escape),
        Key::Delete => Some(TerminalKey::Delete),
        Key::ArrowUp => Some(TerminalKey::ArrowUp),
        Key::ArrowDown => Some(TerminalKey::ArrowDown),
        Key::ArrowLeft => Some(TerminalKey::ArrowLeft),
        Key::ArrowRight => Some(TerminalKey::ArrowRight),
        Key::Home => Some(TerminalKey::Home),
        Key::End => Some(TerminalKey::End),
        Key::PageUp => Some(TerminalKey::PageUp),
        Key::PageDown => Some(TerminalKey::PageDown),
        _ => None,
    }
}
```

- [ ] **Step 4: Port the `bevy_key_to_terminal_key` unit tests**

Append the five `bevy_key_to_terminal_key` unit tests from `src/ozma_input.rs:180-262` into this file's `#[cfg(test)] mod tests` (verbatim — `printable_char_maps_to_text`, `space_maps_to_text`, `control_keys_map_correctly`, `navigation_keys_map_correctly`, `modifier_and_unrecognized_keys_return_none`). They reference `bevy_key_to_terminal_key` and `Key`/`TerminalKey`, all in scope via `use super::*;`.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p ozma_terminal --lib input:: 2>&1 | tail -20`
Expected: PASS for all dispatcher + key-mapping tests.

- [ ] **Step 6: Re-export public types (but not the plugin)**

In `crates/ozma_terminal/src/lib.rs`: add `mod input;` and extend the re-export block:

```rust
pub use input::{InputDisabled, OzmaTerminalInputSet, ReservedChord, TerminalInputBindings};
```

Do NOT add `OzmaInputPlugin` to `OzmaTerminalPlugin` here — wiring it while the binary still runs `forward_keys_to_ozma` would double-handle keys. Task 6 wires it together with removing the binary forwarder.

- [ ] **Step 7: Lint, format, commit**

```bash
cargo test -p ozma_terminal 2>&1 | tail -8
cargo clippy --workspace --all-targets 2>&1 | tail -5
cargo fmt
git add -A
git commit -m "feat(ozma_terminal): add input dispatcher, InputDisabled, TerminalInputBindings"
```

---

### Task 4: `spawn` — `OzmaSpawnOptions`, `OzmaTerminalBundle::spawn`, render-injection observer

**Files:**
- Modify: `crates/ozma_terminal/src/spawn.rs` (add options, bundle, spawn, observer)
- Modify: `crates/ozma_terminal/src/lib.rs` (re-export the new public items; do NOT wire the observer into `OzmaTerminalPlugin` yet — Task 7)
- Modify: `crates/ozma_terminal/Cargo.toml` (add `anyhow`)
- Test: `crates/ozma_terminal/src/spawn.rs` (`#[cfg(test)] mod tests` — extend existing)

**Interfaces:**
- Consumes: `OzmaTerminal`, `resolve_shell` (this file); `SpawnOptions`, `TerminalBundle` (from `ozma_tty_engine`); `TerminalUiMaterial`, `TerminalRenderBundle` (from `ozma_tty_renderer`).
- Produces:
  - `ozma_terminal::OzmaSpawnOptions { shell: Option<String>, cwd: Option<PathBuf>, env: Vec<(String, String)> }` (pub, `Default`).
  - `ozma_terminal::OzmaTerminalBundle` (pub `Bundle`) with `spawn(opts: OzmaSpawnOptions) -> anyhow::Result<Self>`.
  - `on_add_inject_render` (pub(crate) observer fn; inserts `TerminalRenderBundle` on `On<Add, OzmaTerminal>`).

- [ ] **Step 1: Add `anyhow` to the crate manifest**

In `crates/ozma_terminal/Cargo.toml` `[dependencies]`:

```toml
anyhow            = { workspace = true }
```

- [ ] **Step 2: Write the failing test (render injection)**

In `crates/ozma_terminal/src/spawn.rs`, add to the existing `#[cfg(test)] mod tests` block:

```rust
    #[test]
    fn on_add_injects_render_bundle() {
        use bevy::asset::AssetPlugin;
        use ozma_tty_renderer::material::TerminalUiMaterial;
        use ozma_tty_renderer::schema::TerminalGrid;

        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()));
        app.init_resource::<Assets<TerminalUiMaterial>>();
        app.add_observer(on_add_inject_render);
        let entity = app.world_mut().spawn(OzmaTerminal).id();
        app.update();
        assert!(
            app.world().entity(entity).contains::<TerminalGrid>(),
            "On<Add, OzmaTerminal> must inject TerminalRenderBundle (TerminalGrid)",
        );
    }
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p ozma_terminal on_add_injects_render_bundle -v 2>&1 | tail -15`
Expected: FAIL to compile — `on_add_inject_render` not found.

- [ ] **Step 4: Implement options, bundle, spawn, observer**

In `crates/ozma_terminal/src/spawn.rs`, extend the top `use` block to:

```rust
use bevy::prelude::*;
use ozma_tty_engine::{SpawnOptions, TerminalBundle};
use ozma_tty_renderer::material::TerminalUiMaterial;
use ozma_tty_renderer::prelude::TerminalRenderBundle;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
```

Add the public items (after the existing `OzmaTerminal` / `OzmaTerminalConfig` definitions, before the helper fns, to keep `pub` items first):

```rust
/// Options for spawning a standalone Ozma terminal.
#[derive(Default)]
pub struct OzmaSpawnOptions {
    /// Shell override; `None` falls back to `$SHELL` then `/bin/sh`.
    pub shell: Option<String>,
    /// Working directory for the PTY; `None` inherits the process cwd.
    pub cwd: Option<PathBuf>,
    /// Extra environment variables for the PTY.
    pub env: Vec<(String, String)>,
}

/// Self-contained spawn bundle for a standalone Ozma terminal: the engine PTY
/// bundle, the `OzmaTerminal` marker, and a default full-screen `Node`. The
/// GPU render bundle is injected by `on_add_inject_render` on insertion.
#[derive(Bundle)]
pub struct OzmaTerminalBundle {
    terminal: TerminalBundle,
    marker: OzmaTerminal,
    node: Node,
}

impl OzmaTerminalBundle {
    /// Spawns the PTY at a provisional 80x24 (the window-fill resize system
    /// corrects it on the first frame) and returns the bundle. Errors when the
    /// PTY fails to spawn.
    pub fn spawn(opts: OzmaSpawnOptions) -> anyhow::Result<Self> {
        let shell = resolve_shell(opts.shell.as_deref(), std::env::var("SHELL").ok().as_deref());
        let terminal = TerminalBundle::spawn(SpawnOptions {
            cols: 80,
            rows: 24,
            shell,
            cwd: opts.cwd,
            env: opts.env,
            osc_webview_gate: Arc::new(AtomicBool::new(false)),
        })?;
        Ok(Self {
            terminal,
            marker: OzmaTerminal,
            node: Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
        })
    }
}

pub(crate) fn on_add_inject_render(
    ev: On<Add, OzmaTerminal>,
    mut commands: Commands,
    mut materials: ResMut<Assets<TerminalUiMaterial>>,
) {
    let material = materials.add(TerminalUiMaterial::default());
    commands
        .entity(ev.event_target())
        .insert(TerminalRenderBundle::new(material));
}
```

> NOTE: confirm the engine `SpawnOptions` field names against `crates/ozma_tty_engine/src/bundle.rs` (the binary's current `src/ozma.rs:68-75` constructs it as `cols, rows, shell, cwd, env, osc_webview_gate`). If `cwd` there is `Option<PathBuf>` and `env` is `Vec<(String, String)>`, the above matches; otherwise adapt the two fields to the engine's exact types.

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p ozma_terminal on_add_injects_render_bundle -v 2>&1 | tail -15`
Expected: PASS.

- [ ] **Step 6: Re-export public items**

In `crates/ozma_terminal/src/lib.rs`, extend the spawn re-export:

```rust
pub use spawn::{OzmaSpawnOptions, OzmaTerminal, OzmaTerminalBundle, OzmaTerminalConfig, cells_for, resolve_shell};
```

Do NOT add `.add_observer(on_add_inject_render)` to `OzmaTerminalPlugin` here — the binary's `spawn_terminal` still inserts `TerminalRenderBundle` manually until Task 7, and wiring the observer now would double-insert.

- [ ] **Step 7: Build, lint, format, commit**

```bash
cargo test -p ozma_terminal 2>&1 | tail -8
cargo clippy --workspace --all-targets 2>&1 | tail -5
cargo fmt
git add -A
git commit -m "feat(ozma_terminal): add OzmaTerminalBundle::spawn + render-injection observer"
```

---

### Task 5: Binary — derive `TerminalInputBindings` from resolved shortcuts

**Files:**
- Modify: `src/input/shortcuts.rs` (add `ResolvedShortcuts::input_bindings` + a `Startup` population system + register it in `OzmuxShortcutPlugin`)
- Modify: `src/input.rs` (if the plugin lives there — confirm `OzmuxShortcutPlugin` location; it is in `src/input.rs:31`)
- Test: `src/input/shortcuts.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `ReservedChord`, `TerminalInputBindings` (Task 3); `ResolvedShortcuts`, `ShortcutAction` (binary).
- Produces: `ResolvedShortcuts::input_bindings(&self) -> TerminalInputBindings` (pub(crate)); `populate_input_bindings` Startup system that inserts the resource.

- [ ] **Step 1: Write the failing test**

In `src/input/shortcuts.rs` `#[cfg(test)] mod tests`, add:

```rust
    #[test]
    fn input_bindings_excludes_paste_from_reserved() {
        let r = ResolvedShortcuts(resolve_from_bindings(&Bindings::default()));
        let b = r.input_bindings();
        assert_eq!(b.paste.key_code, KeyCode::KeyV);
        assert!(b.paste.meta && !b.paste.ctrl && !b.paste.shift && !b.paste.alt);
        assert_eq!(b.reserved.len(), 4, "Quit, OpenPicker, ReleaseInlineFocus, DetachSession");
        assert!(
            !b.reserved.iter().any(|c| c.key_code == KeyCode::KeyV && c.meta),
            "the paste chord must not appear in reserved",
        );
    }
```

Add `use ozma_terminal::{ReservedChord, TerminalInputBindings};` to the file's top `use` block.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ozmux-gui input_bindings_excludes_paste -v 2>&1 | tail -15`
Expected: FAIL to compile — `input_bindings` not found.

- [ ] **Step 3: Implement `input_bindings` + population system**

In `src/input/shortcuts.rs`, add to the `impl ResolvedShortcuts` block (after `match_gui_action` / `is_release_inline_focus`, keeping pub(crate) items grouped):

```rust
    /// Derives the crate's `TerminalInputBindings` from the resolved table:
    /// the Paste chord becomes `paste`; every other resolved chord becomes a
    /// `reserved` entry the crate dispatcher skips for the host to handle.
    pub(crate) fn input_bindings(&self) -> TerminalInputBindings {
        let mut paste = None;
        let mut reserved = Vec::new();
        for s in &self.0 {
            let chord = ReservedChord {
                key_code: s.keycode,
                ctrl: s.modifiers.ctrl,
                shift: s.modifiers.shift,
                alt: s.modifiers.alt,
                meta: s.modifiers.meta,
            };
            if s.action == ShortcutAction::Paste {
                paste = Some(chord);
            } else {
                reserved.push(chord);
            }
        }
        TerminalInputBindings {
            paste: paste.unwrap_or_else(|| TerminalInputBindings::default().paste),
            reserved,
        }
    }
```

Add the population system at module scope (after `build_resolved_shortcuts`):

```rust
/// `Startup` system: inserts `TerminalInputBindings` derived from the resolved
/// shortcut table, replacing the crate default. Runs after
/// `build_resolved_shortcuts`.
pub(crate) fn populate_input_bindings(mut commands: Commands, resolved: Res<ResolvedShortcuts>) {
    commands.insert_resource(resolved.input_bindings());
}
```

- [ ] **Step 4: Register the system after `build_resolved_shortcuts`**

In `src/input.rs`, `OzmuxShortcutPlugin::build`, change the `Startup` registration so population runs after resolution:

```rust
.add_systems(
    Startup,
    (shortcuts::build_resolved_shortcuts, shortcuts::populate_input_bindings).chain(),
)
```

(Replace the existing single `add_systems(Startup, shortcuts::build_resolved_shortcuts)` call.)

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p ozmux-gui input_bindings_excludes_paste -v 2>&1 | tail -15`
Expected: PASS.

- [ ] **Step 6: Build, lint, format, commit**

```bash
cargo build 2>&1 | tail -5
cargo clippy --workspace --all-targets 2>&1 | tail -5
cargo fmt
git add -A
git commit -m "feat(ozmux): derive TerminalInputBindings from resolved shortcuts at startup"
```

---

### Task 6: Binary switchover — wire crate dispatcher, replace `ozma_input` with host plugin

**Files:**
- Modify: `crates/ozma_terminal/src/lib.rs` (add `OzmaInputPlugin` to `OzmaTerminalPlugin`)
- Rewrite: `src/ozma_input.rs` (replace `OzmaInputPlugin`/`forward_keys_to_ozma` with `OzmaHostInputPlugin`: `maintain_input_disabled` + `app_shortcut_handler`)
- Modify: `src/main.rs:33,97` (`OzmaInputPlugin` → `OzmaHostInputPlugin`)
- Test: `src/ozma_input.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `InputDisabled`, `OzmaTerminalInputSet`, `OzmaTerminal` (Task 3); `SessionPicker`, `ImeState`, `FocusedWebview`, `ResolvedShortcuts`, `ShortcutAction` (binary).
- Produces: `OzmaHostInputPlugin` (pub(crate)); pure helpers `should_disable_input(...)`, `gui_action_suppressed_by_webview(...)`.

- [ ] **Step 1: Wire the crate dispatcher into `OzmaTerminalPlugin`**

In `crates/ozma_terminal/src/lib.rs`, add `OzmaInputPlugin` to the build chain and its `use`:

```rust
use crate::input::OzmaInputPlugin;
```

```rust
.add_plugins((ExitPlugin, LayoutPlugin, OzmaActionPlugin, OzmaInputPlugin));
```

- [ ] **Step 2: Write the failing tests (pure helpers)**

Replace the body of `src/ozma_input.rs` (see Step 4 for the full rewrite). First, write the new test module so it drives the helpers:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disables_input_on_any_guard() {
        assert!(!should_disable_input(false, false, true, false));
        assert!(should_disable_input(true, false, true, false));
        assert!(should_disable_input(false, true, true, false));
        assert!(should_disable_input(false, false, false, false));
        assert!(should_disable_input(false, false, true, true));
    }

    #[test]
    fn webview_focus_suppresses_all_but_release() {
        assert!(gui_action_suppressed_by_webview(true, ShortcutAction::Quit));
        assert!(gui_action_suppressed_by_webview(true, ShortcutAction::OpenPicker));
        assert!(gui_action_suppressed_by_webview(true, ShortcutAction::DetachSession));
        assert!(!gui_action_suppressed_by_webview(true, ShortcutAction::ReleaseInlineFocus));
        assert!(!gui_action_suppressed_by_webview(false, ShortcutAction::Quit));
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p ozmux-gui disables_input_on_any_guard -v 2>&1 | tail -15`
Expected: FAIL to compile (old `OzmaInputPlugin` body still present / helpers missing).

- [ ] **Step 4: Rewrite `src/ozma_input.rs`**

Replace the entire file with:

```rust
//! Host-side input for `AppMode::Ozma`: maintains the crate's `InputDisabled`
//! marker from the coarse guards (picker, IME, focus, webview), and handles the
//! application-level GUI shortcuts the terminal crate does not own (Quit,
//! OpenPicker, DetachSession, ReleaseInlineFocus). Raw-key forwarding and paste
//! are owned by `ozma_terminal`'s dispatcher and `PasteAction`.

use crate::input::InputPhase;
use crate::input::ime::ImeState;
use crate::input::shortcuts::ResolvedShortcuts;
use crate::ozma::AppMode;
use crate::picker::SessionPicker;
use bevy::input::ButtonState;
use bevy::input::keyboard::KeyboardInput;
use bevy::prelude::*;
use bevy::window::{PrimaryWindow, Window};
use bevy_cef::prelude::FocusedWebview;
use ozma_terminal::{InputDisabled, OzmaTerminal, OzmaTerminalInputSet};
use ozmux_configs::shortcuts::{Modifiers, ShortcutAction};

/// Registers the host-side input systems for `AppMode::Ozma`.
pub(crate) struct OzmaHostInputPlugin;

impl Plugin for OzmaHostInputPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            maintain_input_disabled
                .before(OzmaTerminalInputSet)
                .run_if(in_state(AppMode::Ozma)),
        )
        .add_systems(
            Update,
            app_shortcut_handler
                .in_set(InputPhase::FocusedKey)
                .run_if(in_state(AppMode::Ozma))
                .run_if(on_message::<KeyboardInput>),
        );
    }
}

fn maintain_input_disabled(
    mut commands: Commands,
    picker: Res<SessionPicker>,
    ime: Res<ImeState>,
    focused_webview: Res<FocusedWebview>,
    windows: Query<&Window, With<PrimaryWindow>>,
    terminals: Query<(Entity, Has<InputDisabled>), With<OzmaTerminal>>,
) {
    let focused = windows.single().map(|w| w.focused).unwrap_or(false);
    let disable = should_disable_input(
        picker.open,
        ime.is_composing(),
        focused,
        focused_webview.0.is_some(),
    );
    for (entity, has) in terminals.iter() {
        if disable && !has {
            commands.entity(entity).insert(InputDisabled);
        } else if !disable && has {
            commands.entity(entity).remove::<InputDisabled>();
        }
    }
}

fn app_shortcut_handler(
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
    mut events: MessageReader<KeyboardInput>,
    mut picker: ResMut<SessionPicker>,
    mut focused_webview: ResMut<FocusedWebview>,
    shortcuts: Res<ResolvedShortcuts>,
    ime: Res<ImeState>,
    bevy_keys: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let _ = &mut commands;
    let focused = windows.single().map(|w| w.focused).unwrap_or(false);
    if picker.open || ime.is_composing() || !focused {
        events.clear();
        return;
    }
    let mods = current_modifiers(&bevy_keys);
    let webview_focused = focused_webview.0.is_some();
    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        if webview_focused && shortcuts.is_release_inline_focus(ev.key_code, mods) {
            focused_webview.0 = None;
            continue;
        }
        let Some(action) = shortcuts.match_gui_action(ev.key_code, mods) else {
            continue;
        };
        if gui_action_suppressed_by_webview(webview_focused, action) {
            continue;
        }
        match action {
            ShortcutAction::Quit => {
                exit.write(AppExit::Success);
            }
            ShortcutAction::OpenPicker => {
                picker.open = true;
            }
            ShortcutAction::DetachSession => {}
            ShortcutAction::Paste | ShortcutAction::ReleaseInlineFocus => {}
        }
    }
}

fn should_disable_input(
    picker_open: bool,
    composing: bool,
    window_focused: bool,
    webview_focused: bool,
) -> bool {
    picker_open || composing || !window_focused || webview_focused
}

fn gui_action_suppressed_by_webview(webview_focused: bool, action: ShortcutAction) -> bool {
    webview_focused && action != ShortcutAction::ReleaseInlineFocus
}

fn current_modifiers(keys: &ButtonInput<KeyCode>) -> Modifiers {
    Modifiers {
        ctrl: keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight),
        shift: keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight),
        alt: keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight),
        meta: keys.pressed(KeyCode::SuperLeft) || keys.pressed(KeyCode::SuperRight),
    }
}
```

Then append the `#[cfg(test)] mod tests` block from Step 2.

> NOTE: `let _ = &mut commands;` is a placeholder to satisfy the unused-mut lint only if `commands` ends up unused. If `app_shortcut_handler` does not need `Commands`, drop the `mut commands: Commands` param and the `let _` line entirely. Verify against clippy in Step 6 and remove whichever is unused. (The maintainer needs `Commands`; the shortcut handler may not.)

- [ ] **Step 5: Update `main.rs` plugin registration**

In `src/main.rs`: change the import (line 33) `use ozma_input::OzmaInputPlugin;` → `use ozma_input::OzmaHostInputPlugin;`, and the registration (line 97) `OzmaInputPlugin,` → `OzmaHostInputPlugin,`.

- [ ] **Step 6: Run tests, lint, format**

```bash
cargo test -p ozmux-gui webview_focus_suppresses_all_but_release -v 2>&1 | tail -15
cargo test -p ozmux-gui disables_input_on_any_guard -v 2>&1 | tail -10
cargo clippy --workspace --all-targets 2>&1 | tail -15
cargo fmt
```
Expected: both tests PASS; clippy clean (resolve the unused-`Commands` NOTE from Step 4 if it fires).

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(ozmux): switch Ozma input to crate dispatcher + InputDisabled maintainer"
```

---

### Task 7: Binary switchover — collapse `spawn_terminal` to the bundle

**Files:**
- Modify: `crates/ozma_terminal/src/lib.rs` (wire `on_add_inject_render` into `OzmaTerminalPlugin`)
- Modify: `src/ozma.rs` (collapse `spawn_terminal` to `OzmaTerminalBundle::spawn`)
- Test: build + existing `ozma_terminal` tests; manual smoke

**Interfaces:**
- Consumes: `OzmaTerminalBundle`, `OzmaSpawnOptions`, `on_add_inject_render` (Task 4); `OzmaTerminalConfig` (existing).
- Produces: nothing new.

- [ ] **Step 1: Wire the render observer into `OzmaTerminalPlugin`**

In `crates/ozma_terminal/src/lib.rs`, add the observer to the build chain and its `use`:

```rust
use crate::spawn::on_add_inject_render;
```

```rust
.add_plugins((ExitPlugin, LayoutPlugin, OzmaActionPlugin, OzmaInputPlugin))
.add_observer(on_add_inject_render);
```

- [ ] **Step 2: Collapse `spawn_terminal` in `src/ozma.rs`**

Replace the `spawn_terminal` system body (currently `src/ozma.rs:40-99`) with one that builds the bundle. The system keeps reading `OzmaTerminalConfig` for the shell override and writes `AppExit` on failure. New `spawn_terminal`:

```rust
fn spawn_terminal(
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
    config: Res<OzmaTerminalConfig>,
) {
    match OzmaTerminalBundle::spawn(OzmaSpawnOptions {
        shell: config.shell.clone(),
        ..default()
    }) {
        Ok(bundle) => {
            commands.spawn(bundle);
        }
        Err(e) => {
            tracing::error!(?e, "failed to spawn ozma terminal");
            exit.write(AppExit::Success);
        }
    }
}
```

Update the file's top `use` block: drop the now-unused imports (`SpawnOptions`, `TerminalBundle`, `TerminalCellMetricsResource`, `TerminalUiMaterial`, `TerminalRenderBundle`, `cells_for`, `resolve_shell`, `PrimaryWindow`, `Window`, `Arc`, `AtomicBool`) and add:

```rust
use ozma_terminal::{OzmaSpawnOptions, OzmaTerminal, OzmaTerminalBundle, OzmaTerminalConfig};
```

Keep `despawn_terminal` unchanged. Run `cargo build` and remove exactly the imports clippy reports as unused — do not guess; let the compiler list them.

- [ ] **Step 3: Build and verify no double render-bundle insert**

Run:

```bash
cargo build 2>&1 | tail -5
cargo test -p ozma_terminal 2>&1 | tail -8
cargo clippy --workspace --all-targets 2>&1 | tail -10
```
Expected: build + tests pass; clippy clean (no unused imports in `src/ozma.rs`).

- [ ] **Step 4: Manual smoke test**

Run the app and verify Ozma mode end-to-end:

```bash
cargo run 2>&1 | tail -20
```
Verify by hand: (a) typing reaches the shell exactly once (no doubled characters), (b) `Cmd+V` pastes once, (c) `Cmd+Q` quits, (d) `Cmd+Shift+P` opens the picker, (e) `Ctrl+Shift+D` does not emit stray characters, (f) the terminal fills the window with no size flash, (g) entering Ozmux (tmux) mode leaves tmux input working. If any fails, stop and debug before committing.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(ozma_terminal): self-contained OzmaTerminalBundle::spawn; collapse binary spawn_terminal"
```

---

## Self-Review

**Spec coverage:**
- `clipboard.rs` move + `build_paste_bytes` `pub` + consumer fan-out → Task 1.
- `action.rs` `PasteAction` + observer + plugin → Task 2.
- `input.rs` `InputDisabled`, `ReservedChord`, `TerminalInputBindings`, `OzmaTerminalInputSet`, dispatcher (incl. meta-drop), `bevy_key_to_terminal_key`, crate-local modifiers → Task 3.
- `spawn.rs` `OzmaSpawnOptions`, `OzmaTerminalBundle::spawn`, `on_add_inject_render` (deferred-command timing, Assets precondition) → Task 4.
- `ResolvedShortcuts::input_bindings` + population → Task 5.
- Binary split: `InputDisabled` maintainer `.before(OzmaTerminalInputSet)` + app-shortcut handler (webview guard, no paste/raw-forward) → Task 6.
- `spawn_terminal` collapse + render-observer wiring → Task 7.
- Behavior parity (meta-drop, webview suppression, Ctrl+Shift+D no-leak) → covered by Task 3 (`unhandled_meta_chord_is_dropped`), Task 6 (`webview_focus_suppresses_all_but_release`), and Task 7 manual smoke.

**Placeholder scan:** the two `> NOTE:` blocks (engine `SpawnOptions` field check in Task 4; unused-`Commands` check in Task 6) are explicit verification instructions with the exact resolution, not deferred work. No "TBD"/"implement later".

**Type consistency:** `TerminalInputBindings { paste: ReservedChord, reserved: Vec<ReservedChord> }`, `ReservedChord { key_code, ctrl, shift, alt, meta }`, `OzmaTerminalInputSet`, `OzmaInputPlugin`, `OzmaActionPlugin`, `OzmaHostInputPlugin`, `OzmaTerminalBundle::spawn(OzmaSpawnOptions) -> anyhow::Result<Self>`, `on_add_inject_render`, `ResolvedShortcuts::input_bindings`, `should_disable_input`, `gui_action_suppressed_by_webview` — names are consistent across Tasks 2-7. `PasteAction { entity }` triggered in Task 3, observed in Task 2. `OzmaInputPlugin` defined in Task 3, wired in Task 6. `on_add_inject_render` defined in Task 4, wired in Task 7.

**Sequencing safety:** crate capabilities are defined+tested in isolation (Tasks 2-4) without changing the running binary; the two switchover tasks (6 wires the dispatcher + removes the binary forwarder; 7 wires the render observer + removes the manual insert) keep every committed state both compiling and runtime-correct (no double-input / double-render window if the app is run between tasks).
