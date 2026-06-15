# Tmux Migration Phase 1a — Connection Lifecycle Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add boot-time tmux connection orchestration: a `[tmux]` config section, a pure session-selection policy (attach-MRU-or-create), an auto-connect-at-startup system that drives `ConnectionState`, and a modal error dialog shown when tmux is unavailable — all without rendering tmux panes yet (Phase 1b does the projection; Phase 2 does rendering).

**Architecture:** Builds on Phase 0's `ozmux_tmux` crate (`ConnectionState`, `TmuxConnection`, `TmuxSessionPlugin`). ozmux fronts the user's **default** tmux server (iTerm2-style). At `Startup`, if `tmux.auto_connect` is enabled, a system queries `list-sessions`, picks a target via a pure `select_attach_target`, opens a `tmux -CC` connection, installs it into `TmuxConnection`, and sets `ConnectionState::Connecting` (Phase 0's drain flips it to `Attached`). On any failure (e.g. tmux not installed), it sets `ConnectionState::Error { reason }`, which a `TmuxDialogPlugin` overlay surfaces. `auto_connect` defaults to **false** for Phase 1 (no rendering exists yet, so we don't touch the user's real tmux on every launch); Phase 2 flips the default to true.

**Tech Stack:** Rust (edition 2024, toolchain 1.95), Bevy 0.18, the in-repo `ozmux_configs`, `tmux_control`, and `ozmux_tmux` crates, `serde`/`toml`.

---

## Background — verified facts the implementer must rely on

Trust these; they were confirmed by reading the code.

- **Config pattern** (`crates/configs/src/osc_webview.rs` is the template): a `FooConfig` struct with `Default`, a `pub(crate) FooPatch` with `Option` fields + `apply_to(self, base) -> FooConfig`, wired into `OzmuxConfigs` (`crates/configs/src/lib.rs:26`) and `RawConfigs` (`crates/configs/src/raw.rs:19` + the `apply_to` chain at `:33`). The configs crate has `#![warn(missing_docs)]`, so every `pub` item needs a `///` doc.
- **`OzmuxConfigsResource`** (`src/configs.rs:10`): `#[derive(Resource, Debug, Default, Deref)] pub(crate) struct OzmuxConfigsResource(pub(crate) OzmuxConfigs);`. It is inserted at plugin-build time, so it is present by `Startup`. `Deref` means a system reads `configs.tmux` directly.
- **`tmux_control` API**: `TmuxServer::new().program(&str).socket_name(&str)`; `TmuxServer::list_sessions() -> TmuxResult<Vec<SessionInfo>>` (returns `Ok(vec![])` when no server is running; returns `Err(TmuxError::Spawn(..))` when the binary is missing); `TmuxServer::attach(&str) -> TmuxResult<TmuxClient>`; `TmuxServer::new_session() -> TmuxResult<TmuxClient>`. `SessionInfo { id: SessionId, name: String, windows: u32, attached: bool, created: u64 }` — **no last-activity field** (`created` is the recency proxy). `SessionId(pub u32)`. `TmuxError: Display`.
- **`ozmux_tmux` (Phase 0)** exports `ConnectionState` (`Idle`/`Connecting`/`Attached`/`Detached`/`Error{reason}`, derives `Resource`+`Default`=`Idle`), `TmuxConnection` (NonSend resource; `set`/`client`/`take`), `TmuxSessionPlugin`.
- **Bevy 0.18 overlay pattern** (`src/ui/ime_overlay.rs:372`): spawn a `Node { position_type: PositionType::Absolute, display: Display::None, .. }` + `GlobalZIndex(z)` + a marker `Component`; toggle visibility by setting `Node.display` to `Display::Flex`/`Display::None` in a `PostUpdate` system. `Query::single_mut()` returns a `Result`. **Taffy caveat:** a `Text` node that HAS UI children renders at size (0,0); a `Text` node that is a leaf CHILD of a flex container is fine. The workspace enables Bevy's `default_font`, so `Text::new(..)` renders without an explicit `TextFont`.
- **Repo Rust rules:** no `mod.rs`; only `// TODO:`/`// NOTE:`/`// SAFETY:` comments; `//!` per module file; `///` on every `pub` item; all `use` at top in one block; mutable params before immutable; private items after public; `[lints] workspace = true`. No `#[allow]`/`#[expect]` without a justified `// NOTE:`.

## File Structure

- Create `crates/configs/src/tmux.rs` — `TmuxConfig` + `TmuxPatch`.
- Modify `crates/configs/src/lib.rs` — declare `pub mod tmux;`, add `pub tmux: tmux::TmuxConfig` field.
- Modify `crates/configs/src/raw.rs` — add `tmux: Option<TmuxPatch>` + merge.
- Create `crates/tmux_session/src/select.rs` — `AttachTarget` + `select_attach_target`.
- Create `crates/tmux_session/src/connect.rs` — `attach_or_create`.
- Modify `crates/tmux_session/src/lib.rs` — `mod select; mod connect;` + re-exports.
- Create `src/tmux_boot.rs` — `TmuxBootPlugin` (the `Startup` auto-connect system).
- Create `src/ui/tmux_dialog.rs` — `TmuxDialogPlugin` (error overlay).
- Modify `src/main.rs` — declare the two modules, register both plugins.
- Create `crates/tmux_session/tests/real_tmux_boot.rs` — gated end-to-end boot test.

---

### Task 1: `TmuxConfig` config module

**Files:** Create `crates/configs/src/tmux.rs`.

- [ ] **Step 1: Write the module with tests**

Create `crates/configs/src/tmux.rs`:

```rust
//! Configuration for the tmux control-mode backend.

use serde::{Deserialize, Serialize};

/// tmux backend settings.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct TmuxConfig {
    /// tmux binary to run (looked up on `PATH` unless absolute).
    pub program: String,
    /// Optional named server socket (`tmux -L <name>`); `None` targets the
    /// default server, which is what a normal CLI `tmux` uses.
    pub socket_name: Option<String>,
    /// Whether to connect to tmux automatically at startup.
    pub auto_connect: bool,
}

impl Default for TmuxConfig {
    fn default() -> Self {
        Self {
            program: "tmux".to_string(),
            socket_name: None,
            auto_connect: false,
        }
    }
}

/// Per-field-optional view of `[tmux]` for TOML deserialization.
#[derive(Deserialize, Default, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct TmuxPatch {
    /// Optional `[tmux].program` override.
    pub program: Option<String>,
    /// Optional `[tmux].socket_name` override.
    pub socket_name: Option<String>,
    /// Optional `[tmux].auto_connect` override.
    pub auto_connect: Option<bool>,
}

impl TmuxPatch {
    /// Applies this patch over `base`, keeping `base`'s value where unset.
    pub fn apply_to(self, base: TmuxConfig) -> TmuxConfig {
        TmuxConfig {
            program: self.program.unwrap_or(base.program),
            socket_name: self.socket_name.or(base.socket_name),
            auto_connect: self.auto_connect.unwrap_or(base.auto_connect),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_targets_path_tmux_default_socket_no_autoconnect() {
        let c = TmuxConfig::default();
        assert_eq!(c.program, "tmux");
        assert_eq!(c.socket_name, None);
        assert!(!c.auto_connect);
    }

    #[test]
    fn patch_overrides_set_fields_only() {
        let patched = TmuxPatch {
            program: Some("/opt/tmux".to_string()),
            socket_name: None,
            auto_connect: Some(true),
        }
        .apply_to(TmuxConfig::default());
        assert_eq!(patched.program, "/opt/tmux");
        assert_eq!(patched.socket_name, None);
        assert!(patched.auto_connect);
    }

    #[test]
    fn empty_patch_keeps_base() {
        let patched = TmuxPatch::default().apply_to(TmuxConfig::default());
        assert_eq!(patched, TmuxConfig::default());
    }
}
```

- [ ] **Step 2: Run the tests**

Run: `cargo test -p ozmux_configs tmux::`
Expected: 3 tests pass. (The module isn't declared in `lib.rs` yet, so it won't compile as part of the crate — declare it first in Task 2. To run THIS task's tests in isolation, do Task 2 Step 1's `pub mod tmux;` line first, then come back. Simplest: do Task 1 Step 1 and Task 2 Step 1 together, then run tests. If you prefer strict task isolation, add `pub mod tmux;` to `lib.rs` now as part of this task — it is required for the module to exist.)

ACTUAL INSTRUCTION: add `pub mod tmux;` to `crates/configs/src/lib.rs` now (in the existing `pub mod ...;` block, alphabetically near `pub mod theme;`), so this task compiles and tests run. Task 2 adds the `OzmuxConfigs` field and the `raw.rs` wiring.

Run again: `cargo test -p ozmux_configs tmux::` → 3 pass.

- [ ] **Step 3: Commit**

```bash
git add crates/configs/src/tmux.rs crates/configs/src/lib.rs
git commit -m "feat(configs): TmuxConfig section (program, socket_name, auto_connect)"
```

---

### Task 2: Wire `TmuxConfig` into `OzmuxConfigs` and `RawConfigs`

**Files:** Modify `crates/configs/src/lib.rs`, `crates/configs/src/raw.rs`.

- [ ] **Step 1: Add the field to `OzmuxConfigs`**

In `crates/configs/src/lib.rs`, in the `pub struct OzmuxConfigs { .. }` block, add a field after `osc_webview`:

```rust
    /// tmux backend configuration.
    pub tmux: tmux::TmuxConfig,
```

(`pub mod tmux;` was already added in Task 1.)

- [ ] **Step 2: Add the patch to `RawConfigs` and merge it**

In `crates/configs/src/raw.rs`:

1. Add to the `use` block: `use crate::tmux::TmuxPatch;`
2. In `struct RawConfigs`, add a field after `osc_webview`:

```rust
    pub(crate) tmux: Option<TmuxPatch>,
```

3. In `apply_to`, add before `base` is returned:

```rust
        if let Some(patch) = self.tmux {
            base.tmux = patch.apply_to(base.tmux);
        }
```

- [ ] **Step 3: Add an integration test for the `[tmux]` section**

In `crates/configs/src/raw.rs`, inside `mod tests`, add:

```rust
    #[test]
    fn tmux_section_merges_from_toml() {
        let toml_str = r#"
[tmux]
program = "/usr/local/bin/tmux"
auto_connect = true
"#;
        let raw: RawConfigs = toml::from_str(toml_str).unwrap();
        let merged = raw.apply_to(OzmuxConfigs::default());
        assert_eq!(merged.tmux.program, "/usr/local/bin/tmux");
        assert!(merged.tmux.auto_connect);
        assert_eq!(merged.tmux.socket_name, None);
    }

    #[test]
    fn missing_tmux_section_uses_defaults() {
        let raw: RawConfigs = toml::from_str("").unwrap();
        let merged = raw.apply_to(OzmuxConfigs::default());
        assert_eq!(merged.tmux, crate::tmux::TmuxConfig::default());
    }
```

- [ ] **Step 4: Run the tests + clippy**

Run: `cargo test -p ozmux_configs && cargo clippy -p ozmux_configs -- -D warnings`
Expected: all configs tests pass (including the two new ones and Task 1's three); clippy clean.

- [ ] **Step 5: Commit**

```bash
git add crates/configs/src/lib.rs crates/configs/src/raw.rs
git commit -m "feat(configs): wire [tmux] section into OzmuxConfigs merge"
```

---

### Task 3: `AttachTarget` + pure `select_attach_target`

**Files:** Create `crates/tmux_session/src/select.rs`; modify `crates/tmux_session/src/lib.rs`.

- [ ] **Step 1: Write the module with tests**

Create `crates/tmux_session/src/select.rs`:

```rust
//! Choosing which tmux session to attach to at startup.

use tmux_control::SessionInfo;

/// The session to connect to when attaching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachTarget {
    /// Attach to an existing session by name.
    Attach(String),
    /// No suitable session exists; create a fresh one.
    CreateNew,
}

/// Chooses which session to attach to from a `list-sessions` snapshot.
///
/// Prefers an already-attached session (someone is using it); otherwise the
/// most-recently-created (highest `created`, tie-broken by highest id).
/// Returns [`AttachTarget::CreateNew`] when `sessions` is empty. tmux's
/// `SessionInfo` exposes no last-activity field, so creation time is the
/// best available recency proxy.
pub fn select_attach_target(sessions: &[SessionInfo]) -> AttachTarget {
    match sessions.iter().max_by(|a, b| {
        a.attached
            .cmp(&b.attached)
            .then(a.created.cmp(&b.created))
            .then(a.id.0.cmp(&b.id.0))
    }) {
        Some(session) => AttachTarget::Attach(session.name.clone()),
        None => AttachTarget::CreateNew,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tmux_control::SessionId;

    fn session(id: u32, name: &str, attached: bool, created: u64) -> SessionInfo {
        SessionInfo {
            id: SessionId(id),
            name: name.to_string(),
            windows: 1,
            attached,
            created,
        }
    }

    #[test]
    fn empty_creates_new() {
        assert_eq!(select_attach_target(&[]), AttachTarget::CreateNew);
    }

    #[test]
    fn single_session_is_chosen() {
        let s = vec![session(0, "main", false, 10)];
        assert_eq!(
            select_attach_target(&s),
            AttachTarget::Attach("main".to_string())
        );
    }

    #[test]
    fn attached_beats_more_recent_unattached() {
        let s = vec![
            session(0, "old-attached", true, 10),
            session(1, "new-detached", false, 99),
        ];
        assert_eq!(
            select_attach_target(&s),
            AttachTarget::Attach("old-attached".to_string())
        );
    }

    #[test]
    fn among_unattached_most_recent_created_wins() {
        let s = vec![
            session(0, "older", false, 10),
            session(1, "newer", false, 20),
        ];
        assert_eq!(
            select_attach_target(&s),
            AttachTarget::Attach("newer".to_string())
        );
    }

    #[test]
    fn created_tie_broken_by_highest_id() {
        let s = vec![
            session(0, "a", false, 10),
            session(2, "c", false, 10),
            session(1, "b", false, 10),
        ];
        assert_eq!(
            select_attach_target(&s),
            AttachTarget::Attach("c".to_string())
        );
    }
}
```

- [ ] **Step 2: Declare the module and re-export**

In `crates/tmux_session/src/lib.rs`, add `mod select;` to the module block and `pub use select::{AttachTarget, select_attach_target};` to the re-export block.

- [ ] **Step 3: Run the tests**

Run: `cargo test -p ozmux_tmux select::`
Expected: 5 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/tmux_session/src/select.rs crates/tmux_session/src/lib.rs
git commit -m "feat(tmux_session): AttachTarget + select_attach_target policy"
```

---

### Task 4: `attach_or_create` connection helper

**Files:** Create `crates/tmux_session/src/connect.rs`; modify `crates/tmux_session/src/lib.rs`.

- [ ] **Step 1: Write the helper**

Create `crates/tmux_session/src/connect.rs`:

```rust
//! Opening a control-mode connection from a chosen [`AttachTarget`].

use crate::select::AttachTarget;
use tmux_control::{TmuxClient, TmuxResult, TmuxServer};

/// Opens a `tmux -CC` connection for `target`: attaches to the named
/// session, or starts a fresh one.
pub fn attach_or_create(server: &TmuxServer, target: &AttachTarget) -> TmuxResult<TmuxClient> {
    match target {
        AttachTarget::Attach(name) => server.attach(name),
        AttachTarget::CreateNew => server.new_session(),
    }
}
```

- [ ] **Step 2: Declare the module and re-export**

In `crates/tmux_session/src/lib.rs`, add `mod connect;` and `pub use connect::attach_or_create;`.

- [ ] **Step 3: Verify it compiles**

Run: `cargo build -p ozmux_tmux`
Expected: builds. (No unit test: `TmuxClient` cannot be constructed without spawning tmux; this thin dispatch is exercised by the gated boot test in Task 7.)

- [ ] **Step 4: Commit**

```bash
git add crates/tmux_session/src/connect.rs crates/tmux_session/src/lib.rs
git commit -m "feat(tmux_session): attach_or_create connection helper"
```

---

### Task 5: `TmuxBootPlugin` — auto-connect at startup

**Files:** Create `src/tmux_boot.rs`; modify `src/main.rs`.

- [ ] **Step 1: Write the plugin + Startup system + headless test**

Create `src/tmux_boot.rs`:

```rust
//! Boot-time tmux auto-connect: queries sessions, picks a target, opens a
//! control-mode connection, and drives `ConnectionState`.

use crate::configs::OzmuxConfigsResource;
use bevy::prelude::*;
use ozmux_tmux::{ConnectionState, TmuxConnection, attach_or_create, select_attach_target};
use tmux_control::TmuxServer;

/// Registers the `Startup` auto-connect system.
pub(crate) struct TmuxBootPlugin;

impl Plugin for TmuxBootPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, auto_connect_tmux);
    }
}

fn auto_connect_tmux(
    mut state: ResMut<ConnectionState>,
    mut connection: NonSendMut<TmuxConnection>,
    configs: Res<OzmuxConfigsResource>,
) {
    let cfg = &configs.tmux;
    if !cfg.auto_connect {
        return;
    }
    let mut server = TmuxServer::new().program(&cfg.program);
    if let Some(name) = &cfg.socket_name {
        server = server.socket_name(name);
    }
    let sessions = match server.list_sessions() {
        Ok(sessions) => sessions,
        Err(e) => {
            *state = ConnectionState::Error {
                reason: format!("tmux unavailable: {e}"),
            };
            return;
        }
    };
    match attach_or_create(&server, &select_attach_target(&sessions)) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use ozmux_tmux::TmuxSessionPlugin;

    #[test]
    fn stays_idle_when_auto_connect_disabled() {
        let mut app = App::new();
        app.add_plugins((TmuxSessionPlugin, TmuxBootPlugin));
        app.insert_resource(OzmuxConfigsResource::default());
        app.update();
        assert_eq!(
            *app.world().resource::<ConnectionState>(),
            ConnectionState::Idle
        );
    }
}
```

- [ ] **Step 2: Declare the module and register the plugin in `src/main.rs`**

In `src/main.rs`:
1. Add `mod tmux_boot;` to the module declarations at the top (near `mod multiplexer;`).
2. Add `use tmux_boot::TmuxBootPlugin;` to the `use` block (near the other plugin imports).
3. Add `TmuxBootPlugin,` to one of the `.add_plugins((...))` tuples — put it right after the existing `TmuxSessionPlugin,` line (added in Phase 0).

- [ ] **Step 3: Run the targeted test**

Run: `cargo test --bin ozmux-gui tmux_boot::`
Expected: 1 test passes (`stays_idle_when_auto_connect_disabled`). It must reach the assertion (no panic from a missing resource), proving the Startup system is a safe no-op when `auto_connect` is false.

NOTE: a full `cargo test --bin ozmux-gui` SIGSEGVs in this headless CEF/GPU environment — a known pre-existing issue. Run only the `tmux_boot::` filter for this task; it does not touch the GPU/CEF path.

- [ ] **Step 4: Commit**

```bash
git add src/tmux_boot.rs src/main.rs
git commit -m "feat: TmuxBootPlugin auto-connects to tmux at startup (gated by config)"
```

---

### Task 6: `TmuxDialogPlugin` — tmux error dialog overlay

**Files:** Create `src/ui/tmux_dialog.rs`; modify `src/main.rs` (and `src/ui.rs` if it declares the `ui` submodules — check which file declares `pub mod ime_overlay;` etc. and add `tmux_dialog` there).

- [ ] **Step 1: Find where `ui` submodules are declared**

Run: `grep -rn "mod ime_overlay" src/`
This tells you whether `src/ui.rs` (or `src/ui/mod`-equivalent) declares the submodules. Add `pub mod tmux_dialog;` in the SAME place, matching the existing visibility of its siblings.

- [ ] **Step 2: Write the overlay plugin + headless test**

Create `src/ui/tmux_dialog.rs`:

```rust
//! A modal overlay shown when the tmux backend reports an error.

use bevy::prelude::*;
use ozmux_tmux::ConnectionState;

const TMUX_DIALOG_Z: i32 = 300;

/// Spawns and toggles the tmux error dialog overlay.
pub(crate) struct TmuxDialogPlugin;

impl Plugin for TmuxDialogPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_tmux_dialog);
        app.add_systems(PostUpdate, sync_tmux_dialog);
    }
}

#[derive(Component)]
struct TmuxDialogBackdrop;

#[derive(Component)]
struct TmuxDialogText;

fn spawn_tmux_dialog(mut commands: Commands) {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                display: Display::None,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.7)),
            GlobalZIndex(TMUX_DIALOG_Z),
            TmuxDialogBackdrop,
        ))
        .with_children(|parent| {
            parent.spawn((Text::new("tmux unavailable"), TmuxDialogText));
        });
}

fn sync_tmux_dialog(
    mut backdrop: Query<&mut Node, With<TmuxDialogBackdrop>>,
    mut text: Query<&mut Text, With<TmuxDialogText>>,
    state: Res<ConnectionState>,
) {
    let Ok(mut node) = backdrop.single_mut() else {
        return;
    };
    match &*state {
        ConnectionState::Error { reason } => {
            node.display = Display::Flex;
            if let Ok(mut label) = text.single_mut() {
                **label = format!("tmux unavailable\n{reason}");
            }
        }
        _ => node.display = Display::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dialog_shows_only_on_error_state() {
        let mut app = App::new();
        app.init_resource::<ConnectionState>();
        app.add_plugins(TmuxDialogPlugin);
        app.update();

        let show = |app: &mut App| {
            let mut q = app
                .world_mut()
                .query_filtered::<&Node, With<TmuxDialogBackdrop>>();
            q.single(app.world()).unwrap().display
        };

        assert_eq!(show(&mut app), Display::None);

        app.insert_resource(ConnectionState::Error {
            reason: "tmux: command not found".to_string(),
        });
        app.update();
        assert_eq!(show(&mut app), Display::Flex);

        app.insert_resource(ConnectionState::Attached);
        app.update();
        assert_eq!(show(&mut app), Display::None);
    }
}
```

NOTE: if `Query::single`/`single_mut` signatures in this Bevy version differ from the above (the IME overlay code is the source of truth for the exact API — see `src/ui/ime_overlay.rs`), adjust to match the repo's actual usage rather than guessing. The test's `query_filtered` + `single(world)` form mirrors how other headless UI tests in this repo read nodes — if a sibling test uses a different accessor, copy that.

- [ ] **Step 3: Register the plugin in `src/main.rs`**

Add `use ui::tmux_dialog::TmuxDialogPlugin;` (or the path matching how other `ui::` plugins are imported) and add `TmuxDialogPlugin,` to a `.add_plugins((...))` tuple, near `TmuxBootPlugin`.

- [ ] **Step 4: Run the targeted test + build**

Run: `cargo test --bin ozmux-gui tmux_dialog:: && cargo build`
Expected: the `dialog_shows_only_on_error_state` test passes; the binary builds. (Again, run only the filtered test — the full binary test suite SIGSEGVs pre-existingly.)

- [ ] **Step 5: Commit**

```bash
git add src/ui/tmux_dialog.rs src/main.rs src/ui.rs
git commit -m "feat(ui): tmux error dialog overlay driven by ConnectionState"
```

(Adjust the `git add` paths to whichever file you edited to declare the `tmux_dialog` submodule.)

---

### Task 7: Gated real-tmux boot integration test + final verification

**Files:** Create `crates/tmux_session/tests/real_tmux_boot.rs`.

- [ ] **Step 1: Write the gated end-to-end boot test**

This test exercises the real selection + connect path (not the Bevy plugin, which needs the binary's config resource) against a real tmux on a private socket, so it is self-contained and does not touch the user's default server.

Create `crates/tmux_session/tests/real_tmux_boot.rs`:

```rust
//! Gated end-to-end test of the boot connect path against a real tmux.
//! Run with: `cargo test -p ozmux_tmux --test real_tmux_boot -- --ignored`.

use ozmux_tmux::{AttachTarget, attach_or_create, select_attach_target};
use std::time::Duration;
use tmux_control::TmuxServer;

#[test]
#[ignore = "requires a real tmux binary and a controlling PTY"]
fn select_then_connect_against_real_tmux() {
    let socket = format!("ozmux-phase1a-{}", std::process::id());
    let server = TmuxServer::new().socket_name(&socket);

    // No server yet → empty list → CreateNew.
    let sessions = server.list_sessions().expect("list (no server)");
    assert_eq!(select_attach_target(&sessions), AttachTarget::CreateNew);

    // Create the session via the boot helper.
    let created = attach_or_create(&server, &AttachTarget::CreateNew).expect("new_session");
    std::thread::sleep(Duration::from_millis(500));

    // Now a session exists and the -CC client is attached → select attaches.
    let sessions = server.list_sessions().expect("list (with server)");
    assert!(!sessions.is_empty(), "a session should now exist");
    assert!(
        matches!(select_attach_target(&sessions), AttachTarget::Attach(_)),
        "an existing (attached) session should be chosen for attach"
    );

    created.handle().send("kill-server").ok();
}
```

- [ ] **Step 2: Compile, and run if tmux is present**

Run: `cargo test -p ozmux_tmux --test real_tmux_boot --no-run`
Expected: compiles.

Then check `command -v tmux`; if present, run:
`cargo test -p ozmux_tmux --test real_tmux_boot -- --ignored`
Expected: 1 test passes. If tmux is absent, note it; not a failure.

- [ ] **Step 3: Full crate test + clippy sweep**

Run: `cargo test -p ozmux_tmux && cargo test -p ozmux_configs && cargo clippy -p ozmux_tmux -p ozmux_configs -- -D warnings && cargo build`
Expected: all unit tests pass; clippy clean; binary builds. Do NOT run the full-workspace `cargo test` (the `ozmux-gui` binary harness SIGSEGVs pre-existingly in headless CEF/GPU); the per-`--bin` filtered tests from Tasks 5–6 already cover the binary-side additions.

- [ ] **Step 4: Commit**

```bash
git add crates/tmux_session/tests/real_tmux_boot.rs
git commit -m "test(tmux_session): gated real-tmux boot select+connect integration test"
```

---

## Done criteria for Phase 1a

- `[tmux]` config section parses and merges (`program`, `socket_name`, `auto_connect`), default `auto_connect = false`.
- `select_attach_target` policy is unit-tested (attached-first, then most-recent-created, tie-broken by id, empty → create).
- `TmuxBootPlugin` auto-connects at `Startup` only when `auto_connect` is true; failures set `ConnectionState::Error { reason }`; success installs the client and sets `Connecting` (Phase 0's drain advances to `Attached`).
- `TmuxDialogPlugin` shows a modal overlay iff `ConnectionState::Error`, displaying the reason.
- Gated real-tmux boot test passes where tmux is installed.
- `cargo clippy -p ozmux_tmux -p ozmux_configs -- -D warnings` clean; binary builds; old multiplexer untouched; with `auto_connect = false` (the default) real launches behave exactly as before.

## Next: Phase 1b — projection skeleton

Once 1a lands: the pure indexed reducer that turns the initial `list-windows -F "#{window_active}\t#{window_id}\t#{window_layout}\t#{window_visible_layout}\t#{window_name}"` reply (parsed via `WindowLayout::parse`, panes derived from each `Cell::Leaf { pane_id, dims }`) plus the ongoing `WindowAdd`/`WindowClose`/`LayoutChange`/`WindowPaneChanged` control events into `TmuxSession`/`TmuxWindow`/`TmuxPane` entities, with `HashMap<PaneId, Entity>` / `HashMap<WindowId, Entity>` indexes. No rendering (Phase 2). This is mostly a pure, heavily table-tested reducer.
```
