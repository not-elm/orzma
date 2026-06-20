# ozma_terminal Multi-Terminal Keyboard Input Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `ozma_terminal`'s keyboard path route to a sticky `KeyboardFocused` terminal so it works with multiple live `OzmaTerminal` entities, mirroring the multi-terminal mouse effort (#164).

**Architecture:** Add a positive `KeyboardFocused` marker component to the crate; `dispatch_input` routes to the single focused, non-disabled terminal via `.single()`. Focus (`KeyboardFocused`, sticky, host-owned) and modal gating (`KeyboardDisabled`, transient) stay orthogonal. The host inserts the marker on spawn and makes its two IME systems (`read_ime_events` commit routing, `ime_policy_system` enable + candidate-window anchoring) follow the focused terminal.

**Tech Stack:** Rust 2024, Bevy 0.18 ECS, `cargo test`.

## Global Constraints

- Rust edition 2024, toolchain 1.95. No `mod.rs`.
- Comments only `// TODO:` / `// NOTE:` / `// SAFETY:`, English only.
- Every `pub` item has a `///` doc; module files have `//!`.
- All `use` in one contiguous top-of-file block; no inline fully-qualified paths. Test-local `use` inside `#[cfg(test)] mod tests` is allowed.
- Bevy: mutable `SystemParam`s before immutable; `Query` params are descriptive nouns (no `_q`); whole-system change gates via `run_if`, not in-body early return; `Plugin::build` is one method chain.
- Visibility minimized; private items declared after `pub` ones; `#[expect(reason=…)]` over `#[allow]`.
- Spec: `docs/superpowers/specs/2026-06-20-ozma-terminal-multi-terminal-keyboard-design.md`.

---

### Task 1: `KeyboardFocused` marker + `dispatch_input` routing (crate `ozma_terminal`)

**Files:**
- Modify: `crates/ozma_terminal/src/input.rs` (marker, `dispatch_input` query + NOTE, tests)
- Modify: `crates/ozma_terminal/src/lib.rs:21` (export)
- Modify: `crates/ozma_terminal/src/spawn.rs:11-15` (doc)
- Test: `crates/ozma_terminal/src/input.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Produces: `pub struct KeyboardFocused;` (in `ozma_terminal::input`, re-exported as `ozma_terminal::KeyboardFocused`). A unit marker component. `dispatch_input` routes raw keys to the entity matching `(With<OzmaTerminal>, With<KeyboardFocused>, Without<KeyboardDisabled>)`, requiring exactly one such entity.

- [ ] **Step 1: Add the `KeyboardFocused` marker**

In `crates/ozma_terminal/src/input.rs`, immediately after the `KeyboardDisabled` definition (the `#[derive(Component)] pub struct KeyboardDisabled;` block ending at line 18), add:

```rust
/// When present on an `OzmaTerminal` entity, that terminal is the keyboard
/// focus: the crate's keyboard dispatcher routes raw keys to it, and the host
/// routes IME commits and anchors the OS candidate window to it. The host owns
/// focus policy and maintains the "exactly one focused" invariant; a terminal
/// with no `KeyboardFocused` receives no keyboard input.
#[derive(Component)]
pub struct KeyboardFocused;
```

- [ ] **Step 2: Write the failing tests**

In `crates/ozma_terminal/src/input.rs`, inside `#[cfg(test)] mod tests`, add three tests (place them after `keyboard_disabled_entity_fires_nothing`):

```rust
    #[test]
    fn routes_to_keyboard_focused_terminal() {
        let mut app = test_app();
        app.world_mut().spawn(OzmaTerminal);
        app.world_mut().spawn((OzmaTerminal, KeyboardFocused));
        press(&mut app, KeyCode::KeyA, Key::Character("a".into()));
        app.update();
        let c = app.world().resource::<Captured>();
        assert_eq!(c.keys, vec![TerminalKey::Text("a".into())]);
    }

    #[test]
    fn no_focused_terminal_drops_keys() {
        let mut app = test_app();
        app.world_mut().spawn(OzmaTerminal);
        press(&mut app, KeyCode::KeyA, Key::Character("a".into()));
        app.update();
        assert!(app.world().resource::<Captured>().keys.is_empty());
    }

    #[test]
    fn two_focused_terminals_drop_keys() {
        let mut app = test_app();
        app.world_mut().spawn((OzmaTerminal, KeyboardFocused));
        app.world_mut().spawn((OzmaTerminal, KeyboardFocused));
        press(&mut app, KeyCode::KeyA, Key::Character("a".into()));
        app.update();
        assert!(app.world().resource::<Captured>().keys.is_empty());
    }
```

- [ ] **Step 3: Run the new tests — verify they fail**

Run: `cargo test -p ozma_terminal routes_to_keyboard_focused_terminal`
Expected: FAIL. `dispatch_input` still calls `.single()` over `Without<KeyboardDisabled>`; with two `OzmaTerminal` entities present, `.single()` returns `Err`, every key is dropped, and the asserted `["a"]` is empty.

- [ ] **Step 4: Update `dispatch_input` to route by focus**

In `crates/ozma_terminal/src/input.rs`, change the `terminal` query parameter (currently line 93):

```rust
    terminal: Query<Entity, (With<OzmaTerminal>, With<KeyboardFocused>, Without<KeyboardDisabled>)>,
```

Replace the existing `// NOTE:` block above the `let Ok(entity) = terminal.single()` line (currently lines 95-97) with:

```rust
    // NOTE: keyboard routes to the single `KeyboardFocused` terminal. The host
    // owns focus policy and MUST keep exactly one OzmaTerminal both
    // `KeyboardFocused` and not `KeyboardDisabled`, or this `.single()` returns
    // Err and every keypress is silently dropped.
```

- [ ] **Step 5: Update existing tests to mark the spawned terminal focused**

In `crates/ozma_terminal/src/input.rs` tests, the four single-terminal tests must spawn a focused terminal:

- `plain_key_forwards_as_terminal_key`: change `app.world_mut().spawn(OzmaTerminal);` to `app.world_mut().spawn((OzmaTerminal, KeyboardFocused));`
- `paste_chord_fires_paste_action`: same change.
- `reserved_chord_is_skipped`: same change.
- `unhandled_meta_chord_is_dropped`: same change.

Then convert `keyboard_disabled_entity_fires_nothing` to prove disabled overrides focus — rename it and add the focus marker:

```rust
    #[test]
    fn keyboard_disabled_overrides_focus() {
        let mut app = test_app();
        app.world_mut()
            .spawn((OzmaTerminal, KeyboardFocused, KeyboardDisabled));
        press(&mut app, KeyCode::KeyA, Key::Character("a".into()));
        app.update();
        let c = app.world().resource::<Captured>();
        assert!(c.keys.is_empty());
        assert_eq!(c.paste, 0);
    }
```

- [ ] **Step 6: Run the full crate test suite — verify green**

Run: `cargo test -p ozma_terminal`
Expected: PASS (all tests, including the three new ones and the renamed disabled-overrides-focus test).

- [ ] **Step 7: Export the marker and fix the `OzmaTerminal` doc**

In `crates/ozma_terminal/src/lib.rs`, replace the `input` re-export (line 21):

```rust
pub use input::{
    KeyboardDisabled, KeyboardFocused, OzmaTerminalInputSet, ReservedChord, TerminalInputBindings,
};
```

In `crates/ozma_terminal/src/spawn.rs`, update the `OzmaTerminal` doc body (lines 13-15) to:

```rust
/// One or more entities may carry this marker; mouse input routes to the
/// topmost under the cursor, while keyboard input (raw keys and IME) targets the
/// single entity the host marks `KeyboardFocused`.
```

- [ ] **Step 8: Confirm the crate builds clean and commit**

Run: `cargo test -p ozma_terminal && cargo clippy -p ozma_terminal`
Expected: PASS, no warnings.

```bash
git add crates/ozma_terminal/src/input.rs crates/ozma_terminal/src/lib.rs crates/ozma_terminal/src/spawn.rs
git commit -m "feat(ozma_terminal): route keyboard to KeyboardFocused terminal"
```

---

### Task 2: Host inserts `KeyboardFocused` on the spawned terminal

**Files:**
- Modify: `src/ozma.rs:4` (import), `src/ozma.rs:33-50` (`spawn_terminal`)

**Interfaces:**
- Consumes: `ozma_terminal::KeyboardFocused` (Task 1).
- Produces: the host's single `OzmaTerminal` always carries `KeyboardFocused`, so `dispatch_input.single()` finds exactly one focused terminal. No code in later tasks depends on new symbols here.

- [ ] **Step 1: Import `KeyboardFocused`**

In `src/ozma.rs`, extend the import (line 4):

```rust
use ozma_terminal::{
    KeyboardFocused, OzmaSpawnOptions, OzmaTerminal, OzmaTerminalBundle, OzmaTerminalConfig,
};
```

- [ ] **Step 2: Spawn the terminal focused**

In `src/ozma.rs`, in `spawn_terminal`, change the spawn call so the bundle is tupled with the marker. Replace:

```rust
        Ok(bundle) => {
            commands.spawn(bundle);
        }
```

with:

```rust
        Ok(bundle) => {
            commands.spawn((bundle, KeyboardFocused));
        }
```

- [ ] **Step 3: Update the `OzmaModePlugin` / `spawn_terminal` doc**

In `src/ozma.rs`, update the `OzmaModePlugin` doc line that reads "Spawns one `OzmaTerminal` entity on `OnEnter(AppMode::Ozma)`" (line 19) to note focus:

```rust
/// Spawns one `OzmaTerminal` entity (marked `KeyboardFocused`, the keyboard
/// target) on `OnEnter(AppMode::Ozma)` and despawns it on `OnExit(AppMode::Ozma)`.
```

(Keep the remaining doc sentences about plugin ordering unchanged.)

- [ ] **Step 4: Build the binary — verify it compiles**

Run: `cargo build -p ozmux-gui`
Expected: PASS, no warnings. (There is no unit test for `spawn_terminal`; it spawns a real PTY. The crate-level routing tests from Task 1 prove the dispatch behavior; this task only wires the host to satisfy the focus invariant.)

- [ ] **Step 5: Manual verification**

Run: `cargo run`
Expected: the terminal accepts keyboard input (type `echo hi` + Enter; the shell echoes and runs it). This confirms the host's spawned terminal is `KeyboardFocused` and `dispatch_input` routes to it. If keys do nothing, the marker is not being inserted.

- [ ] **Step 6: Commit**

```bash
git add src/ozma.rs
git commit -m "feat: mark the host Ozma terminal KeyboardFocused on spawn"
```

---

### Task 3: Host IME follows `KeyboardFocused` (`src/input/ime.rs`)

**Files:**
- Modify: `src/input/ime.rs:25` (import), `:171` (`ime_policy_system` param), `:224-234` (Ozma surface resolution), `:306` (`read_ime_events` param), `:352-365` (Ozma commit arm)
- Test: `src/input/ime.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `ozma_terminal::KeyboardFocused` (Task 1).
- Produces: in `AppMode::Ozma`, IME commit text routes to the `KeyboardFocused` terminal (`TerminalKeyInput { entity: <focused>, key: Text(..), .. }`), and `ime_policy_system` enables IME + anchors `ime_position` at that terminal's cursor.

- [ ] **Step 1: Write the failing IME tests**

In `src/input/ime.rs`, inside `#[cfg(test)] mod tests`, add a test-local import as the first line after `use super::*;` (line ~403):

```rust
    use ozma_terminal::KeyboardFocused;
```

Then add these two tests at the end of the `tests` module (before its closing `}`):

```rust
    #[test]
    fn ime_commit_routes_to_focused_ozma_terminal() {
        use bevy::prelude::On;

        #[derive(Resource, Default)]
        struct Hits(Vec<Entity>);

        let mut app = App::new();
        app.add_plugins((MinimalPlugins, StatesPlugin))
            .add_systems(Update, read_ime_events);
        app.init_resource::<ImeState>();
        app.init_resource::<FocusedWebview>();
        app.init_resource::<Hits>();
        app.init_state::<AppMode>();
        app.insert_non_send_resource(TmuxConnection::default());
        app.add_message::<Ime>();
        app.add_observer(|ev: On<TerminalKeyInput>, mut hits: ResMut<Hits>| {
            hits.0.push(ev.entity);
        });

        app.world_mut().spawn(OzmaTerminal);
        let focused = app.world_mut().spawn((OzmaTerminal, KeyboardFocused)).id();

        app.world_mut()
            .resource_mut::<bevy::ecs::message::Messages<Ime>>()
            .write(Ime::Commit {
                window: Entity::PLACEHOLDER,
                value: "あ".into(),
            });
        app.update();

        assert_eq!(app.world().resource::<Hits>().0, vec![focused]);
    }

    #[test]
    fn ime_enabled_and_anchored_for_focused_ozma_terminal() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, StatesPlugin));
        app.init_resource::<FocusedWebview>();
        app.init_state::<AppMode>();
        app.insert_resource(TerminalCellMetricsResource {
            metrics: CellMetrics {
                advance_phys: 8.0,
                line_height_phys: 16.0,
                ascent_phys: 12.0,
                descent_phys: 4.0,
                underline_position_phys: -2.0,
                underline_thickness_phys: 1.0,
                max_overflow_phys: 0.0,
            },
            phys_font_size: 12,
        });

        app.world_mut().spawn(OzmaTerminal);
        app.world_mut().spawn((
            OzmaTerminal,
            KeyboardFocused,
            ComputedNode::default(),
            UiGlobalTransform::default(),
            TerminalGrid {
                cursor: Some(Cursor::default()),
                ..default()
            },
        ));
        app.world_mut().spawn((
            Window {
                focused: true,
                resolution: WindowResolution::new(800, 600),
                ime_enabled: false,
                ..default()
            },
            PrimaryWindow,
        ));

        app.world_mut().run_system_once(ime_policy_system).unwrap();

        let mut q = app
            .world_mut()
            .query_filtered::<&Window, With<PrimaryWindow>>();
        let window = q.single(app.world()).expect("primary window");
        assert!(
            window.ime_enabled,
            "IME must enable for the focused Ozma terminal even with another terminal present"
        );
        assert_eq!(
            window.ime_position,
            Vec2::new(0.0, 16.0),
            "candidate window anchors one row below the focused terminal's cursor"
        );
    }
```

- [ ] **Step 2: Run the new tests — verify they fail**

Run: `cargo test -p ozmux-gui ime_commit_routes_to_focused_ozma_terminal ime_enabled_and_anchored_for_focused_ozma_terminal`
Expected: FAIL.
- `ime_commit_routes_to_focused_ozma_terminal`: with two `OzmaTerminal` entities the Ozma arm's `ozma_terminal.single()` returns `Err`, so no `TerminalKeyInput` fires and `Hits` is empty.
- `ime_enabled_and_anchored_for_focused_ozma_terminal`: with two terminals the old `ozma_terminal.single().is_ok()` is `false` so IME stays disabled, and the old Ozma arm never anchors `ime_position` (stays `Vec2::ZERO`).

- [ ] **Step 3: Make `read_ime_events` route to the focused terminal**

In `src/input/ime.rs`, change the import (line 25):

```rust
use ozma_terminal::{KeyboardFocused, OzmaTerminal};
```

Change the `read_ime_events` `ozma_terminal` parameter (line 306):

```rust
    ozma_terminal: Query<Entity, (With<OzmaTerminal>, With<KeyboardFocused>)>,
```

In the `AppMode::Ozma` arm of `read_ime_events` (lines 352-365), add a `// NOTE:` directly above the `let Ok(entity) = ozma_terminal.single() else { continue; };` line:

```rust
                AppMode::Ozma => {
                    // NOTE: bevy_cef delivers the commit to the webview independently; suppress here to prevent duplicate input.
                    if focused_webview.0.is_some() {
                        continue;
                    }
                    // NOTE: route the commit to the focused terminal but do NOT
                    // also filter on `KeyboardDisabled` — IME composition itself
                    // sets `KeyboardDisabled` (suppressing raw keys via
                    // `dispatch_input`), yet the commit must still land here.
                    let Ok(entity) = ozma_terminal.single() else {
                        continue;
                    };
                    commands.trigger(TerminalKeyInput {
                        entity,
                        key: TerminalKey::Text(commit_text),
                        modifiers: TerminalModifiers::default(),
                    });
                }
```

- [ ] **Step 4: Make `ime_policy_system` use the focused terminal as the Ozma surface**

In `src/input/ime.rs`, change the `ime_policy_system` `ozma_terminal` parameter (line 171):

```rust
    ozma_terminal: Query<Entity, (With<OzmaTerminal>, With<KeyboardFocused>)>,
```

Replace the no-surface early-return block (currently lines 224-234):

```rust
    let Some(entity) = active_surface else {
        if *current_mode.get() == AppMode::Ozma {
            let desired = ozma_terminal.single().is_ok();
            if window.ime_enabled != desired {
                window.ime_enabled = desired;
            }
        } else if window.ime_enabled {
            window.ime_enabled = false;
        }
        return;
    };
```

with:

```rust
    let surface = match active_surface {
        Some(entity) => Some(entity),
        // NOTE: Ozma mode has no tmux ActivePane; the keyboard-focused terminal
        // is the IME surface, so the shared path below anchors `ime_position` at
        // its cursor (the tmux path's same px math).
        None if *current_mode.get() == AppMode::Ozma => ozma_terminal.single().ok(),
        None => None,
    };
    let Some(entity) = surface else {
        if window.ime_enabled {
            window.ime_enabled = false;
        }
        return;
    };
```

(The existing copy-mode / `desired` / anchoring code below this block is unchanged and now runs for the focused Ozma terminal: `copy_modes.get(entity)` is `Err` for it, so `desired` is `true`, and `anchors.get(entity)` succeeds because `OzmaTerminal` carries `ComputedNode` / `UiGlobalTransform` / `TerminalGrid`.)

- [ ] **Step 5: Remove the now-redundant test-local import**

In `src/input/ime.rs` tests module, delete the `use ozma_terminal::KeyboardFocused;` line added in Step 1 — `use super::*;` now re-exports `KeyboardFocused` because the top-of-file import (Step 3) brings it into the module. (The new tests reference `OzmaTerminal` via `super::*` and `KeyboardFocused` via `super::*`; no test-local `ozma_terminal` import remains.)

- [ ] **Step 6: Run the IME tests — verify green**

Run: `cargo test -p ozmux-gui ime`
Expected: PASS — the two new tests plus all existing IME tests (`commit_consumes_state_with_active_pane`, `ime_enabled_for_active_tmux_pane`, etc.) stay green.

- [ ] **Step 7: Build clean and commit**

Run: `cargo build -p ozmux-gui && cargo clippy -p ozmux-gui`
Expected: PASS, no warnings.

```bash
git add src/input/ime.rs
git commit -m "feat: route Ozma IME commit and anchoring to the focused terminal"
```

---

### Task 4: Workspace verification

**Files:** none (verification only).

- [ ] **Step 1: Full workspace test + lint**

Run: `cargo test && cargo clippy --workspace`
Expected: PASS, no warnings across the workspace.

- [ ] **Step 2: Manual end-to-end check (raw keys + IME)**

Run: `cargo run`
Expected:
- Typing reaches the shell (raw-key path).
- IME composition (switch to a Japanese IME, type, confirm) commits to the terminal and the candidate window appears at the cursor (IME path).

No commit for this task (verification only). If any step fails, return to the owning task.

---

## Notes for the implementer

- **Package names:** the crate is `ozma_terminal`; the host binary package is `ozmux-gui`. Use `-p ozma_terminal` / `-p ozmux-gui` to scope `cargo test`.
- **Why `.single()` is kept** in `dispatch_input` and both IME systems: the host maintains the "exactly one `KeyboardFocused`" invariant. Two focused terminals is a host bug and degrades to "keys dropped" (the `two_focused_terminals_drop_keys` test pins this contract), the same failure shape as before this change.
- **Out of scope (do not implement):** focus *policy* (what moves `KeyboardFocused` between terminals — click-to-focus, cycle keybind), multi-terminal layout/spawning, and multi-terminal exit policy. The host still spawns one terminal; this change is forward-looking capability.
