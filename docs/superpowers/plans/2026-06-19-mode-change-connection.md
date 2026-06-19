# Mode Change Connection Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire `AppMode` Bevy state into the running app so startup mode (ozma/ozmux/auto-attach) is config-driven, tmux plugins activate only in `AppMode::Ozmux`, and a detach shortcut transitions back to `AppMode::Ozma`.

**Architecture:** `OzmaModePlugin` owns `AppMode` state and takes an `initial_mode` from config. `OzmuxTmuxPlugin` responds to `OnEnter`/`OnExit(AppMode::Ozmux)` to manage `TmuxPresence` and the tmux connection lifecycle. All tmux UI Update systems are gated via a shared `OzmuxActiveSet` SystemSet.

**Tech Stack:** Rust 1.95, Bevy 0.18, `crates/ozma_mode`, `crates/tmux_session` (`ozmux_tmux`), `crates/configs` (`ozmux_configs`), `serde` with `rename_all = "kebab-case"`.

## Global Constraints

- Edition 2024, toolchain 1.95 (see `rust-toolchain.toml`)
- No `mod.rs` — module files are `foo.rs` + `foo/bar.rs`
- Comments: only `// TODO:`, `// NOTE:`, `// SAFETY:`
- Every externally-`pub` item needs a `///` doc comment
- `pub(crate)` visibility by default; widen only when needed
- Mutable system params before immutable
- No inline fully-qualified paths in signatures — add a `use`
- All comments in English
- Run `cargo test -p <crate>` after each crate-level task; `cargo build` after `main.rs` changes
- Working directory for all commands: `/Users/taiga/workspace/ozmux/wt/mode`

---

### Task 1: `StartupMode` config

**Files:**
- Create: `crates/configs/src/startup.rs`
- Modify: `crates/configs/src/lib.rs`
- Modify: `crates/configs/src/raw.rs`

**Interfaces:**
- Produces: `pub enum StartupMode { Ozma, Ozmux, AutoAttach }` in `ozmux_configs::StartupMode`
- Produces: `pub startup_mode: StartupMode` field on `OzmuxConfigs`

- [ ] **Step 1: Create `startup.rs`**

```rust
// crates/configs/src/startup.rs
//! Startup mode: which application mode launches on boot.

use serde::Deserialize;

/// Determines which mode the application enters on launch.
#[derive(Deserialize, Default, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum StartupMode {
    /// Single PTY terminal, no tmux (default).
    #[default]
    Ozma,
    /// Show the tmux session picker.
    Ozmux,
    /// Auto-attach to the most recently active tmux session.
    AutoAttach,
}
```

- [ ] **Step 2: Add module and field to `lib.rs`**

In `crates/configs/src/lib.rs`, add `pub mod startup;` and `pub use startup::StartupMode;` to the existing `pub mod` and `pub use` list. Add `pub startup_mode: StartupMode,` to `OzmuxConfigs`:

```rust
// Add alongside the other pub mod declarations:
pub mod startup;
pub use startup::StartupMode;

// In OzmuxConfigs struct, add after the last field:
/// Startup mode: which application mode launches on boot.
pub startup_mode: StartupMode,
```

And add `startup_mode: StartupMode::default(),` to `OzmuxConfigs::default()` (or rely on `#[derive(Default)]` if that's used — check the impl).

- [ ] **Step 3: Wire `raw.rs`**

In `crates/configs/src/raw.rs`, add to `RawConfigs`:
```rust
pub(crate) startup_mode: Option<crate::startup::StartupMode>,
```

In `RawConfigs::apply_to`, add:
```rust
if let Some(m) = self.startup_mode {
    base.startup_mode = m;
}
```

- [ ] **Step 4: Write tests**

In `crates/configs/src/raw.rs` test module, add:

```rust
#[test]
fn startup_mode_defaults_to_ozma() {
    let raw: RawConfigs = toml::from_str("").unwrap();
    let merged = raw.apply_to(OzmuxConfigs::default());
    assert_eq!(merged.startup_mode, crate::startup::StartupMode::Ozma);
}

#[test]
fn startup_mode_auto_attach_parses() {
    let raw: RawConfigs = toml::from_str(r#"startup_mode = "auto-attach""#).unwrap();
    let merged = raw.apply_to(OzmuxConfigs::default());
    assert_eq!(merged.startup_mode, crate::startup::StartupMode::AutoAttach);
}

#[test]
fn startup_mode_ozmux_parses() {
    let raw: RawConfigs = toml::from_str(r#"startup_mode = "ozmux""#).unwrap();
    let merged = raw.apply_to(OzmuxConfigs::default());
    assert_eq!(merged.startup_mode, crate::startup::StartupMode::Ozmux);
}

#[test]
fn unknown_startup_mode_is_rejected() {
    assert!(toml::from_str::<RawConfigs>(r#"startup_mode = "invalid""#).is_err());
}
```

- [ ] **Step 5: Run tests**

```bash
cargo test -p ozmux_configs
```

Expected: all existing tests pass; new startup_mode tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/configs/src/startup.rs crates/configs/src/lib.rs crates/configs/src/raw.rs
git commit -m "feat(configs): add StartupMode enum and startup_mode config field"
```

---

### Task 2: `TmuxConnectionReset` pub re-export + `TmuxSessionPlugin` gates

**Files:**
- Modify: `crates/tmux_session/src/events.rs`
- Modify: `crates/tmux_session/src/lib.rs`
- Modify: `crates/tmux_session/src/plugin.rs`

**Interfaces:**
- Consumes: `TmuxPresence` from `crates/tmux_session/src/plugin.rs` (already pub)
- Produces: `pub struct TmuxConnectionReset` accessible as `ozmux_tmux::TmuxConnectionReset`
- Produces: `drain_tmux_events` and `request_pane_captures` gated on `resource_exists::<TmuxPresence>()`

- [ ] **Step 1: Make `TmuxConnectionReset` public**

In `crates/tmux_session/src/events.rs`, line with `pub(crate) struct TmuxConnectionReset;`:

```rust
// Before:
pub(crate) struct TmuxConnectionReset;

// After:
/// Triggers a full teardown of the tmux ECS projection when the transport closes.
pub struct TmuxConnectionReset;
```

- [ ] **Step 2: Re-export from `lib.rs`**

In `crates/tmux_session/src/lib.rs`, add to the existing `pub use` block:

```rust
pub use events::TmuxConnectionReset;
```

- [ ] **Step 3: Remove `TmuxPresence` insert from `TmuxSessionPlugin::build`**

In `crates/tmux_session/src/plugin.rs`, remove the `.insert_resource(TmuxPresence)` line from `TmuxSessionPlugin::build`. The resource is now inserted by `on_enter_ozmux` in `src/tmux.rs` (Task 5).

- [ ] **Step 4: Gate drain systems on `TmuxPresence`**

In `crates/tmux_session/src/plugin.rs`, change the two `add_systems` calls:

```rust
// Before:
.add_systems(Update, drain_tmux_events.in_set(TmuxProjectionSet))
.add_systems(Update, request_pane_captures.after(TmuxProjectionSet));

// After:
.add_systems(
    Update,
    drain_tmux_events
        .in_set(TmuxProjectionSet)
        .run_if(resource_exists::<TmuxPresence>),
)
.add_systems(
    Update,
    request_pane_captures
        .after(TmuxProjectionSet)
        .run_if(resource_exists::<TmuxPresence>),
);
```

Ensure `resource_exists` is imported: it is in `bevy::prelude::*`.

- [ ] **Step 5: Verify tests still pass**

```bash
cargo test -p ozmux_tmux
```

Expected: existing tests pass. The `plugin_registers_resources_and_stays_idle_without_connection` test does not check `TmuxPresence` so it passes unchanged. Observer tests don't involve the drain system.

- [ ] **Step 6: Commit**

```bash
git add crates/tmux_session/src/events.rs crates/tmux_session/src/lib.rs crates/tmux_session/src/plugin.rs
git commit -m "feat(tmux_session): pub-export TmuxConnectionReset; gate drain systems on TmuxPresence"
```

---

### Task 3: `OzmaModePlugin` `initial_mode` parameter

**Files:**
- Modify: `crates/ozma_mode/src/lib.rs`

**Interfaces:**
- Consumes: `AppMode` (already defined here)
- Produces: `OzmaModePlugin::new(config_shell: Option<String>, initial_mode: AppMode) -> Self`

- [ ] **Step 1: Add `initial_mode` field and update `new`**

In `crates/ozma_mode/src/lib.rs`, change `OzmaModePlugin`:

```rust
// Before:
pub struct OzmaModePlugin {
    config_shell: Option<String>,
}

impl OzmaModePlugin {
    pub fn new(config_shell: Option<String>) -> Self {
        Self { config_shell }
    }
}

// After:
pub struct OzmaModePlugin {
    config_shell: Option<String>,
    initial_mode: AppMode,
}

impl OzmaModePlugin {
    /// Constructs the plugin. `initial_mode` is the `AppMode` to insert as the
    /// starting state; derive it from `OzmuxConfigs::startup_mode` at the call site.
    pub fn new(config_shell: Option<String>, initial_mode: AppMode) -> Self {
        Self { config_shell, initial_mode }
    }
}
```

- [ ] **Step 2: Switch `init_state` → `insert_state`**

In `OzmaModePlugin::build`:

```rust
// Before:
app.init_state::<AppMode>()

// After:
app.insert_state(self.initial_mode.clone())
```

- [ ] **Step 3: Update the existing test**

In the `#[cfg(test)]` block, update the test that constructs `OzmaModePlugin`:

```rust
// Before:
OzmaModePlugin::new(None)

// After:
OzmaModePlugin::new(None, AppMode::Ozma)
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p ozma_mode
```

Expected: `plugin_registers_state_and_defaults_to_ozma` passes (now tests `AppMode::Ozma` which is inserted via `insert_state`).

- [ ] **Step 5: Commit**

```bash
git add crates/ozma_mode/src/lib.rs
git commit -m "feat(ozma_mode): add initial_mode param to OzmaModePlugin; use insert_state"
```

---

### Task 4: `ShortcutAction::DetachSession` + `Bindings::detach_session`

**Files:**
- Modify: `crates/configs/src/shortcuts.rs`

**Interfaces:**
- Produces: `ShortcutAction::DetachSession` variant
- Produces: `Bindings::detach_session: Option<KeyChord>` (default `None`)
- `Bindings::iter()` yields `("detach-session", &self.detach_session, ShortcutAction::DetachSession)`

- [ ] **Step 1: Add `DetachSession` variant to `ShortcutAction`**

In `crates/configs/src/shortcuts.rs`, in the `ShortcutAction` enum, add after the last variant:

```rust
/// Detaches from the tmux session and returns to Ozma single-terminal mode.
DetachSession,
```

- [ ] **Step 2: Add `detach_session` field to `Bindings`**

In the `Bindings` struct (look for the block of `pub ... Option<KeyChord>` fields), add after the last field (after `copy`):

```rust
/// Detach the current tmux session and switch to Ozma mode.
#[serde(deserialize_with = "deser_chord_or_unbind", default)]
pub detach_session: Option<KeyChord>,
```

- [ ] **Step 3: Add `None` to `Bindings::default()`**

In `impl Default for Bindings`, add:

```rust
detach_session: None,
```

- [ ] **Step 4: Add to `Bindings::iter()`**

In `impl Bindings`, in the `iter` method's array literal, add:

```rust
("detach-session", &self.detach_session, ShortcutAction::DetachSession),
```

- [ ] **Step 5: Run tests**

```bash
cargo test -p ozmux_configs -- shortcuts
```

Expected: `default_bindings_resolve_to_four` still passes (detach_session defaults to None so resolve count stays 4). `validate_no_conflicts` tests still pass.

- [ ] **Step 6: Commit**

```bash
git add crates/configs/src/shortcuts.rs
git commit -m "feat(configs): add ShortcutAction::DetachSession and Bindings::detach_session"
```

---

### Task 5: Wire `main.rs` + `OzmuxTmuxPlugin` lifecycle

**Files:**
- Modify: `Cargo.toml` (root)
- Modify: `src/main.rs`
- Modify: `src/tmux.rs`

**Interfaces:**
- Consumes: `OzmaModePlugin::new(shell, initial_mode)` from Task 3
- Consumes: `StartupMode` from Task 1
- Consumes: `TmuxConnectionReset` from Task 2
- Produces: `pub(crate) struct OzmuxActiveSet` (SystemSet for gating all tmux Update systems)
- Produces: `OnEnter(AppMode::Ozmux)` inserts `TmuxPresence`; `OnExit(AppMode::Ozmux)` disconnects and triggers cleanup

- [ ] **Step 1: Add `ozma_mode` dep to root `Cargo.toml`**

In `Cargo.toml` `[dependencies]`, add alongside the other crate path deps:

```toml
ozma_mode = { path = "crates/ozma_mode" }
```

- [ ] **Step 2: Add imports to `src/main.rs`**

At the top of `src/main.rs`, in the existing `use` block, add:

```rust
use ozma_mode::{AppMode, OzmaModePlugin};
use ozmux_configs::StartupMode;
```

- [ ] **Step 3: Load config early and map to `AppMode` in `main()`**

At the start of `fn main()`, before `let dyn_registry = ...`, add:

```rust
let pre_configs = ozmux_configs::OzmuxConfigs::load_blocking().unwrap_or_default();
let initial_mode = match pre_configs.startup_mode {
    StartupMode::Ozma => AppMode::Ozma,
    StartupMode::Ozmux | StartupMode::AutoAttach => AppMode::Ozmux,
};
```

- [ ] **Step 4: Add `OzmaModePlugin` to the App**

In `fn main()`, add `OzmaModePlugin` as the FIRST plugin (before `OzmuxConfigsPlugin`, before `OzmuxTmuxPlugin`) so `AppMode` state is registered before any plugin that uses `in_state(AppMode::Ozmux)`:

```rust
.add_plugins(OzmaModePlugin::new(pre_configs.ozma.shell.clone(), initial_mode))
```

Place it immediately after `DefaultPlugins` and `cef_plugin` in the first `.add_plugins(...)` call block, or in its own `.add_plugins(...)` before the others. `OzmaModePlugin` must precede `OzmuxTmuxPlugin`.

- [ ] **Step 5: Define `OzmuxActiveSet` in `src/tmux.rs`**

In `src/tmux.rs`, add the SystemSet definition and import after the existing `use` declarations:

```rust
use bevy::prelude::*;
use ozma_mode::AppMode;
use ozmux_configs::StartupMode;
use ozmux_tmux::{
    CopyModeQueries, EnumerationState, KeyBindings, TmuxConnection,
    TmuxConnectionReset, TmuxPresence,
};

/// SystemSet applied to every tmux Update system. Gated to `AppMode::Ozmux`
/// so all tmux UI is a no-op in single-terminal Ozma mode.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct OzmuxActiveSet;
```

- [ ] **Step 6: Add lifecycle systems in `OzmuxTmuxPlugin::build`**

In `OzmuxTmuxPlugin::build`, before `.add_plugins(...)`, add:

```rust
app.configure_sets(Update, OzmuxActiveSet.run_if(in_state(AppMode::Ozmux)));
app.add_systems(OnEnter(AppMode::Ozmux), on_enter_ozmux);
app.add_systems(OnExit(AppMode::Ozmux), on_exit_ozmux);
```

- [ ] **Step 7: Implement `on_enter_ozmux` and `on_exit_ozmux`**

Add at the bottom of `src/tmux.rs` (private helpers, after the `impl Plugin` block):

```rust
fn on_enter_ozmux(mut commands: Commands) {
    commands.insert_resource(TmuxPresence);
    // Picker/auto-attach logic runs in src/picker.rs via its own OnEnter(AppMode::Ozmux).
}

fn on_exit_ozmux(
    mut commands: Commands,
    mut connection: NonSendMut<TmuxConnection>,
    mut enumeration: ResMut<EnumerationState>,
    mut keybindings: ResMut<KeyBindings>,
    mut copy_queries: ResMut<CopyModeQueries>,
) {
    if let Some(client) = connection.client() {
        let _ = client.handle().send("detach-client");
    }
    connection.take();
    *enumeration = EnumerationState::default();
    keybindings.clear();
    copy_queries.clear();
    commands.remove_resource::<TmuxPresence>();
    commands.trigger(TmuxConnectionReset);
}
```

- [ ] **Step 8: Build check**

```bash
cargo build 2>&1 | head -40
```

Expected: compiles without errors. Some `unused import` warnings for `StartupMode` in `src/tmux.rs` are fine until Task 7.

- [ ] **Step 9: Commit**

```bash
git add Cargo.toml src/main.rs src/tmux.rs
git commit -m "feat: wire OzmaModePlugin and OzmuxTmuxPlugin lifecycle into main.rs"
```

---

### Task 6: Gate tmux UI Update systems with `OzmuxActiveSet`

**Files:**
- Modify: `src/tmux/render.rs`
- Modify: `src/tmux/input.rs`
- Modify: `src/tmux/mouse.rs`
- Modify: `src/tmux/copy_mode.rs`
- Modify: `src/tmux/window_bar.rs`
- Modify: `src/tmux/dialog.rs`
- Modify: `src/tmux/divider_handle.rs`
- Modify: `src/tmux/pane_focus.rs`

**Interfaces:**
- Consumes: `super::OzmuxActiveSet` from `src/tmux.rs`

Pattern: in every sub-plugin's `build()`, add `.in_set(super::OzmuxActiveSet)` to every `Update`-schedule `add_systems` call. For `PostUpdate` systems in dialog and window_bar, add `.run_if(in_state(crate::tmux::AppMode... ))` — but since importing `AppMode` from `ozma_mode` is available, use `in_state(super::super_mode())` or just use the direct import.

Import needed in each file that uses `OzmuxActiveSet`:
```rust
use ozma_mode::AppMode;
```
And for `in_state`:
```rust
use bevy::prelude::*;  // already present in all files
```

- [ ] **Step 1: Gate `render.rs` Update systems**

In `src/tmux/render.rs`, in `RenderPlugin::build`, for every `.add_systems(Update, ...)` call, append `.in_set(super::OzmuxActiveSet)`:

```rust
// Example pattern — apply to ALL Update add_systems calls in this file:
.add_systems(Update, sync_client_size.after(TmuxProjectionSet).in_set(super::OzmuxActiveSet))
// ... repeat for each Update system
```

Find all `.add_systems(Update, ...)` in `render.rs` and chain `.in_set(super::OzmuxActiveSet)`.

- [ ] **Step 2: Gate `input.rs` Update systems**

In `src/tmux/input.rs`, apply the same pattern to all `Update` systems in `InputPlugin::build`.

- [ ] **Step 3: Gate `mouse.rs` Update systems**

In `src/tmux/mouse.rs`, apply to all `Update` systems in `MousePlugin::build`.

- [ ] **Step 4: Gate `copy_mode.rs` Update systems**

In `src/tmux/copy_mode.rs`, apply to all `Update` systems.

- [ ] **Step 5: Gate `window_bar.rs` systems**

In `src/tmux/window_bar.rs`:
- All `Update` systems: add `.in_set(super::OzmuxActiveSet)`
- The `PostStartup` system `spawn_window_bar` spawns hidden UI — leave as-is (the bar is empty in Ozma mode; no user-visible impact)

- [ ] **Step 6: Gate `dialog.rs` systems**

In `src/tmux/dialog.rs`:
- `Update` systems: add `.in_set(super::OzmuxActiveSet)`
- `PostUpdate` sync system: add `.run_if(bevy::prelude::in_state(ozma_mode::AppMode::Ozmux))`
  ```rust
  use ozma_mode::AppMode;
  // In build():
  .add_systems(PostUpdate, sync_tmux_dialog.run_if(in_state(AppMode::Ozmux)))
  ```
- `Startup` `spawn_tmux_dialog` spawns hidden UI — leave as-is

- [ ] **Step 7: Gate `divider_handle.rs` and `pane_focus.rs`**

Apply `.in_set(super::OzmuxActiveSet)` to all `Update` systems in each file.

- [ ] **Step 8: Build and test**

```bash
cargo build 2>&1 | head -40
cargo test -p ozmux_tmux
```

Expected: builds cleanly; tests pass.

- [ ] **Step 9: Commit**

```bash
git add src/tmux/render.rs src/tmux/input.rs src/tmux/mouse.rs src/tmux/copy_mode.rs \
        src/tmux/window_bar.rs src/tmux/dialog.rs src/tmux/divider_handle.rs src/tmux/pane_focus.rs
git commit -m "feat(tmux): gate all tmux UI Update systems via OzmuxActiveSet"
```

---

### Task 7: Picker auto-attach integration

**Files:**
- Modify: `src/picker.rs`

**Interfaces:**
- Consumes: `StartupMode` from `ozmux_configs`
- Consumes: `select_attach_target` from `ozmux_tmux`
- Consumes: `OzmuxActiveSet` from `crate::tmux`
- Produces: `OnEnter(AppMode::Ozmux)` system that either shows picker or auto-attaches

- [ ] **Step 1: Add imports to `src/picker.rs`**

```rust
use ozma_mode::AppMode;
use ozmux_configs::StartupMode;
use ozmux_tmux::select_attach_target;
```

- [ ] **Step 2: Move session listing to `OnEnter`; add auto-attach path**

Replace the current `Startup` registration of `list_sessions_into_picker` with an `OnEnter(AppMode::Ozmux)` system, and rename the function to `on_enter_ozmux_picker`:

In `OzmuxPickerPlugin::build`, replace:
```rust
.add_systems(Startup, (list_sessions_into_picker, spawn_picker_ui))
```
with:
```rust
.add_systems(Startup, spawn_picker_ui)
.add_systems(OnEnter(AppMode::Ozmux), on_enter_ozmux_picker)
```

- [ ] **Step 3: Implement `on_enter_ozmux_picker`**

Rename the existing `list_sessions_into_picker` function to `on_enter_ozmux_picker` and expand it:

```rust
fn on_enter_ozmux_picker(
    mut picker: ResMut<SessionPicker>,
    mut state: ResMut<ConnectionState>,
    mut connection: NonSendMut<TmuxConnection>,
    configs: Res<OzmuxConfigsResource>,
    control: Option<Res<ControlPlaneHandle>>,
) {
    match &configs.startup_mode {
        StartupMode::Ozmux => {
            let server = build_server(&configs);
            match server.list_sessions() {
                Ok(sessions) => {
                    picker.sessions = sessions;
                    picker.selected = 0;
                    picker.open = true;
                }
                Err(e) => {
                    *state = ConnectionState::Error {
                        reason: format!("tmux unavailable: {e}"),
                    };
                }
            }
        }
        StartupMode::AutoAttach => {
            let mut server = build_server(&configs);
            if let Some(handle) = &control {
                server = server.env("OZMA_SOCK", &handle.sock_path.to_string_lossy());
            }
            match server.list_sessions() {
                Ok(sessions) => {
                    let target = select_attach_target(&sessions);
                    match attach_or_create(&server, &target) {
                        Ok(client) => {
                            connection.set(client);
                            *state = ConnectionState::Connecting;
                        }
                        Err(e) => {
                            *state = ConnectionState::Error {
                                reason: format!("auto-attach failed: {e}"),
                            };
                        }
                    }
                }
                Err(e) => {
                    *state = ConnectionState::Error {
                        reason: format!("tmux unavailable: {e}"),
                    };
                }
            }
        }
        StartupMode::Ozma => {}
    }
}
```

- [ ] **Step 4: Gate picker Update systems**

In `OzmuxPickerPlugin::build`, add `.in_set(crate::tmux::OzmuxActiveSet)` to every `Update` system:

```rust
.add_systems(
    Update,
    handle_picker_input
        .after(crate::input::InputPhase::FocusedKey)
        .in_set(crate::tmux::OzmuxActiveSet),
)
.add_systems(Update, refresh_picker_on_open.in_set(crate::tmux::OzmuxActiveSet))
.add_systems(
    Update,
    refresh_session_ozmux_sock
        .run_if(resource_exists_and_changed::<ConnectionState>)
        .in_set(crate::tmux::OzmuxActiveSet),
)
```

The `PostUpdate` `sync_picker_ui` system runs on `SessionPicker` change and is cheap enough to leave ungated. The `Last` `cleanup_session_ozmux_sock` runs only on `AppExit` and can stay ungated.

- [ ] **Step 5: Build check**

```bash
cargo build 2>&1 | head -40
```

Expected: compiles cleanly.

- [ ] **Step 6: Commit**

```bash
git add src/picker.rs
git commit -m "feat(picker): move session listing to OnEnter(Ozmux); add auto-attach path"
```

---

### Task 8: `DetachSession` shortcut dispatch

**Files:**
- Modify: `src/tmux/input.rs`

**Interfaces:**
- Consumes: `ShortcutAction::DetachSession` from Task 4
- Consumes: `NextState<AppMode>` from `bevy::prelude`
- Produces: pressing the configured `detach-session` chord in Ozmux mode transitions to `AppMode::Ozma`

- [ ] **Step 1: Add `NextState<AppMode>` to `forward_keys_to_tmux`**

In `src/tmux/input.rs`, add `use ozma_mode::AppMode;` to the `use` block.

In `forward_keys_to_tmux`'s parameter list, add `mut next_mode: ResMut<NextState<AppMode>>` as the first (mutable) parameter — per the mutable-params-first rule, it goes before the immutable params. The existing `commands: Commands` is also mutable, so order as:

```rust
fn forward_keys_to_tmux(
    mut commands: Commands,
    mut next_mode: ResMut<NextState<AppMode>>,
    mut picker: ResMut<SessionPicker>,
    // ... rest of mutable params unchanged
    // ... then immutable params unchanged
```

- [ ] **Step 2: Handle `DetachSession` in the `match action` block**

In `forward_keys_to_tmux`, find the `match action { ... }` block and add after the existing arms:

```rust
ShortcutAction::DetachSession => {
    next_mode.set(AppMode::Ozma);
}
```

- [ ] **Step 3: Build check**

```bash
cargo build 2>&1 | head -40
```

Expected: compiles cleanly.

- [ ] **Step 4: Run all workspace tests**

```bash
cargo test 2>&1 | tail -20
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/tmux/input.rs
git commit -m "feat(tmux/input): handle ShortcutAction::DetachSession to transition AppMode::Ozma"
```

---

## Self-Review

**Spec coverage check:**

| Spec requirement | Task |
|---|---|
| `StartupMode` config (ozma/ozmux/auto-attach) | Task 1 |
| `TmuxConnectionReset` pub re-export | Task 2 |
| `TmuxSessionPlugin` drain systems gated on `TmuxPresence` | Task 2 |
| `OzmaModePlugin::new(shell, initial_mode)` | Task 3 |
| `insert_state` instead of `init_state` | Task 3 |
| Test update for `OzmaModePlugin::new(None, AppMode::Ozma)` | Task 3 |
| `ShortcutAction::DetachSession` + `KeyChord` | Task 4 |
| `ozma_mode` dep in root `Cargo.toml` | Task 5 |
| Config loaded early in `main()` for initial mode | Task 5 |
| `OzmuxActiveSet` SystemSet defined | Task 5 |
| `on_enter_ozmux` inserts `TmuxPresence` | Task 5 |
| `on_exit_ozmux` disconnects, resets resources, triggers `TmuxConnectionReset` | Task 5 |
| Gate tmux UI Update systems | Task 6 |
| Picker moves session listing to `OnEnter(AppMode::Ozmux)` | Task 7 |
| Auto-attach uses `select_attach_target` | Task 7 |
| `DetachSession` dispatch in shortcut handler | Task 8 |

**Placeholder scan:** No TBD/TODO/placeholders. All code blocks are complete.

**Type consistency:**
- `OzmaModePlugin::new(Option<String>, AppMode)` defined in Task 3, called in Task 5 ✓
- `OzmuxActiveSet` defined in Task 5, used in Tasks 6 & 7 as `super::OzmuxActiveSet` / `crate::tmux::OzmuxActiveSet` ✓
- `TmuxConnectionReset` made `pub` in Task 2, used in Task 5's `on_exit_ozmux` ✓
- `StartupMode::DetachSession` — NOTE: `ShortcutAction::DetachSession` is in Task 4 (shortcuts), `StartupMode` is in Task 1 (startup). These are separate types. ✓
- `select_attach_target(&sessions)` — signature is `fn select_attach_target(sessions: &[SessionInfo]) -> AttachTarget` per `ozmux_tmux::select`. Matches usage in Task 7 ✓
