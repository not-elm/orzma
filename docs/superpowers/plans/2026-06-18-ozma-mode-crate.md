# ozma_mode Crate Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Create `crates/ozma_mode` — a Bevy plugin crate that owns `AppMode` State and implements Ozma mode (single PTY terminal, no tmux), plus the `[ozma]` config section in `crates/configs`.

**Architecture:** `OzmaModePlugin` registers `AppMode` State, inserts `OzmaModeConfig` from constructor args, and adds four lifecycle hooks: `OnEnter` spawns the terminal, an `Update` system resizes it to fill the window, a Bevy observer sends `AppExit` on shell exit, and `OnExit` despawns. Config is extended with a new `[ozma]` TOML section (`shell` field, `OzmaConfig`/`OzmaPatch`) wired into `RawConfigs`.

**Tech Stack:** Rust edition 2024, Bevy 0.18, `ozma_tty_engine` (PTY + VT), `ozma_tty_renderer` (`TerminalRenderBundle`, `TerminalCellMetricsResource`, `TerminalUiMaterial`), `ozmux_configs`.

## Global Constraints

- Edition 2024, toolchain pinned to 1.95 (`rust-toolchain.toml`).
- No `mod.rs` files — use `foo.rs` + `foo/bar.rs` layout.
- Comments: only `// TODO:`, `// NOTE:`, `// SAFETY:`. No narrative comments.
- Doc comments (`///`) required on every `pub` item.
- Mutable parameters first in function signatures.
- `run_if` for whole-system resource/event gates — no in-body early returns.
- `#[expect(..., reason = "...")]` over `#[allow(...)]` for lint suppressions.
- Lint: `cargo clippy --workspace --fix --allow-dirty --allow-staged && cargo fmt` (or `make fix-lint`).
- Test command: `cargo test -p ozmux_configs` (Task 1), `cargo test -p ozma_mode` (Tasks 2–5).

---

## File Map

| Path | Action | Responsibility |
|---|---|---|
| `crates/configs/src/ozma.rs` | CREATE | `OzmaConfig`, `OzmaPatch`, per-field tests |
| `crates/configs/src/lib.rs` | MODIFY | Add `pub mod ozma;`, `pub ozma: OzmaConfig` field to `OzmuxConfigs` |
| `crates/configs/src/raw.rs` | MODIFY | Add `ozma: Option<OzmaPatch>` to `RawConfigs`, arm in `apply_to` |
| `crates/ozma_mode/Cargo.toml` | CREATE | Crate metadata + deps |
| `crates/ozma_mode/src/lib.rs` | CREATE | `AppMode`, `OzmaModeConfig`, `OzmaModePlugin` |
| `crates/ozma_mode/src/spawn.rs` | CREATE | `resolve_shell`, `OzmaTerminal` marker, `spawn_terminal` system |
| `crates/ozma_mode/src/layout.rs` | CREATE | `OzmaLastSize` resource, `resize_to_window` system |
| `crates/ozma_mode/src/exit.rs` | CREATE | `on_child_exit` observer |

---

### Task 1: Add `[ozma]` config section to `crates/configs`

**Files:**
- Create: `crates/configs/src/ozma.rs`
- Modify: `crates/configs/src/lib.rs` (lines ~20–45 for mod/struct)
- Modify: `crates/configs/src/raw.rs` (RawConfigs struct + apply_to)

**Interfaces:**
- Produces: `ozmux_configs::ozma::OzmaConfig { pub shell: Option<String> }` — read by Task 2's `OzmaModePlugin::new`.

- [ ] **Step 1: Write failing tests in `crates/configs/src/ozma.rs`**

Create the file with tests only (no implementation yet):

```rust
//! Configuration for the Ozma single-terminal mode.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_shell_is_none() {
        assert!(OzmaConfig::default().shell.is_none());
    }

    #[test]
    fn patch_overrides_shell() {
        let patched = OzmaPatch {
            shell: Some("/bin/fish".to_string()),
        }
        .apply_to(OzmaConfig::default());
        assert_eq!(patched.shell.as_deref(), Some("/bin/fish"));
    }

    #[test]
    fn empty_patch_keeps_base() {
        let patched = OzmaPatch::default().apply_to(OzmaConfig::default());
        assert_eq!(patched, OzmaConfig::default());
    }

    #[test]
    fn ozma_section_parses_from_toml() {
        let toml_str = r#"
[ozma]
shell = "/usr/bin/zsh"
"#;
        let raw: crate::raw::RawConfigs = toml::from_str(toml_str).unwrap();
        let merged = raw.apply_to(crate::OzmuxConfigs::default());
        assert_eq!(merged.ozma.shell.as_deref(), Some("/usr/bin/zsh"));
    }

    #[test]
    fn missing_ozma_section_uses_defaults() {
        let raw: crate::raw::RawConfigs = toml::from_str("").unwrap();
        let merged = raw.apply_to(crate::OzmuxConfigs::default());
        assert!(merged.ozma.shell.is_none());
    }
}
```

- [ ] **Step 2: Run tests to confirm they fail**

```bash
cargo test -p ozmux_configs 2>&1 | grep -E "error|FAILED|cannot find"
```

Expected: compile errors — `OzmaConfig`, `OzmaPatch` not found.

- [ ] **Step 3: Implement `crates/configs/src/ozma.rs`**

Replace the file with the full implementation + tests:

```rust
//! Configuration for the Ozma single-terminal mode.

use serde::Deserialize;

/// Resolved Ozma mode settings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OzmaConfig {
    /// Shell program to launch. `None` means "resolve at runtime via `$SHELL`".
    pub shell: Option<String>,
}

/// Per-field-optional `[ozma]` view for TOML deserialization.
#[derive(Deserialize, Default, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct OzmaPatch {
    /// Optional shell override.
    pub shell: Option<String>,
}

impl OzmaPatch {
    /// Applies this patch over `base`, keeping `base`'s value where unset.
    pub(crate) fn apply_to(self, base: OzmaConfig) -> OzmaConfig {
        OzmaConfig {
            shell: self.shell.or(base.shell),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_shell_is_none() {
        assert!(OzmaConfig::default().shell.is_none());
    }

    #[test]
    fn patch_overrides_shell() {
        let patched = OzmaPatch {
            shell: Some("/bin/fish".to_string()),
        }
        .apply_to(OzmaConfig::default());
        assert_eq!(patched.shell.as_deref(), Some("/bin/fish"));
    }

    #[test]
    fn empty_patch_keeps_base() {
        let patched = OzmaPatch::default().apply_to(OzmaConfig::default());
        assert_eq!(patched, OzmaConfig::default());
    }

    #[test]
    fn ozma_section_parses_from_toml() {
        let toml_str = r#"
[ozma]
shell = "/usr/bin/zsh"
"#;
        let raw: crate::raw::RawConfigs = toml::from_str(toml_str).unwrap();
        let merged = raw.apply_to(crate::OzmuxConfigs::default());
        assert_eq!(merged.ozma.shell.as_deref(), Some("/usr/bin/zsh"));
    }

    #[test]
    fn missing_ozma_section_uses_defaults() {
        let raw: crate::raw::RawConfigs = toml::from_str("").unwrap();
        let merged = raw.apply_to(crate::OzmuxConfigs::default());
        assert!(merged.ozma.shell.is_none());
    }
}
```

- [ ] **Step 4: Add `pub mod ozma;` and `pub ozma: OzmaConfig` to `crates/configs/src/lib.rs`**

In the module declaration block (after `pub mod tmux;`), add:

```rust
pub mod ozma;
```

In the `OzmuxConfigs` struct, add the field after `pub tmux: tmux::TmuxConfig,`:

```rust
    /// Ozma single-terminal mode configuration.
    pub ozma: ozma::OzmaConfig,
```

The `Default` derive already handles `OzmaConfig::default()` — no `impl Default` change needed.

- [ ] **Step 5: Add `ozma: Option<OzmaPatch>` to `crates/configs/src/raw.rs`**

In `RawConfigs`, add after `pub(crate) tmux: Option<TmuxPatch>,`:

```rust
    pub(crate) ozma: Option<crate::ozma::OzmaPatch>,
```

In `RawConfigs::apply_to`, add after the `tmux` arm:

```rust
        if let Some(patch) = self.ozma {
            base.ozma = patch.apply_to(base.ozma);
        }
```

Also add the import at the top of `raw.rs` (already uses `use crate::tmux::TmuxPatch;` style — no new import needed since we reference via `crate::ozma::OzmaPatch` inline, or add `use crate::ozma::OzmaPatch;` to the use block):

Add to the `use` block at the top of `raw.rs`:
```rust
use crate::ozma::OzmaPatch;
```

Then use `ozma: Option<OzmaPatch>` (without `crate::` prefix).

- [ ] **Step 6: Run tests to verify all pass**

```bash
cargo test -p ozmux_configs 2>&1 | tail -5
```

Expected output ends with: `test result: ok. N passed; 0 failed`

- [ ] **Step 7: Lint**

```bash
cargo clippy -p ozmux_configs --fix --allow-dirty --allow-staged && cargo fmt
```

- [ ] **Step 8: Commit**

```bash
git add crates/configs/src/ozma.rs crates/configs/src/lib.rs crates/configs/src/raw.rs
git commit -m "feat(configs): add [ozma] config section with shell field"
```

---

### Task 2: Scaffold `crates/ozma_mode` — `AppMode` + plugin stub

**Files:**
- Create: `crates/ozma_mode/Cargo.toml`
- Create: `crates/ozma_mode/src/lib.rs`

**Interfaces:**
- Produces:
  - `ozma_mode::AppMode` — `pub enum AppMode { Ozma, Ozmux }`, `Default = Ozma`
  - `ozma_mode::OzmaModePlugin::new(config_shell: Option<String>) -> OzmaModePlugin`
  - `ozma_mode::OzmaModeConfig` (internal resource, `pub(crate)`) — holds `shell: Option<String>`

- [ ] **Step 1: Write failing test in a temporary inline location**

We can't write the test file until the crate exists. Instead, create the skeleton first (Step 2), then add the test.

- [ ] **Step 2: Create `crates/ozma_mode/Cargo.toml`**

```toml
[package]
name = "ozma_mode"
version.workspace = true
edition.workspace = true
license.workspace = true
readme.workspace = true
authors.workspace = true
publish.workspace = true

[dependencies]
bevy              = { workspace = true }
ozma_tty_engine   = { path = "../ozma_tty_engine" }
ozma_tty_renderer = { path = "../ozma_tty_renderer" }
ozmux_configs     = { path = "../configs" }
tracing           = { workspace = true }
anyhow            = { workspace = true }

[lints]
workspace = true
```

- [ ] **Step 3: Create `crates/ozma_mode/src/lib.rs`** with `AppMode`, `OzmaModeConfig`, and a stub `OzmaModePlugin`

```rust
//! Ozma single-terminal mode: owns `AppMode` State and the `OzmaModePlugin`.

mod exit;
mod layout;
mod spawn;

use bevy::prelude::*;

/// Application mode. `Ozma` is the default (single PTY, no tmux).
/// `Ozmux` activates the tmux multiplexer backend.
///
/// Owned here and re-exported so `crates/ozmux_mode` (future) can depend on
/// this crate for the shared state type rather than duplicating it.
#[derive(States, Debug, Clone, PartialEq, Eq, Hash, Default)]
pub enum AppMode {
    /// Single PTY terminal, Alacritty VT emulation, no tmux.
    #[default]
    Ozma,
    /// tmux backend, multiplexer layout.
    Ozmux,
}

/// Internal resource storing the constructor-injected shell override.
/// `None` means "fall back to `$SHELL` at spawn time".
#[derive(Resource)]
pub(crate) struct OzmaModeConfig {
    pub(crate) shell: Option<String>,
}

/// Bevy plugin implementing Ozma mode: spawns a single PTY terminal on
/// `OnEnter(AppMode::Ozma)`, resizes it to fill the window, and sends
/// `AppExit` when the shell process exits.
pub struct OzmaModePlugin {
    config_shell: Option<String>,
}

impl OzmaModePlugin {
    /// Constructs the plugin with the shell override from config.
    ///
    /// Pass `OzmuxConfigs.ozma.shell` here; the plugin resolves
    /// `$SHELL` and `/bin/sh` fallbacks at spawn time.
    pub fn new(config_shell: Option<String>) -> Self {
        Self { config_shell }
    }
}

impl Plugin for OzmaModePlugin {
    fn build(&self, app: &mut App) {
        app.init_state::<AppMode>()
            .insert_resource(OzmaModeConfig {
                shell: self.config_shell.clone(),
            })
            .add_observer(exit::on_child_exit)
            .add_systems(OnEnter(AppMode::Ozma), spawn::spawn_terminal)
            .add_systems(
                Update,
                layout::resize_to_window
                    .run_if(in_state(AppMode::Ozma))
                    .run_if(
                        resource_exists_and_changed::<layout::OzmaLastSize>
                            .or(resource_exists_and_changed::<ozma_tty_renderer::TerminalCellMetricsResource>)
                            .or(on_event::<bevy::window::WindowResized>),
                    ),
            )
            .add_systems(OnExit(AppMode::Ozma), spawn::despawn_terminal);
    }
}
```

- [ ] **Step 4: Write failing test at the bottom of `lib.rs`** (inside `#[cfg(test)]`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_registers_state_and_defaults_to_ozma() {
        let mut app = App::new();
        app.add_plugins(OzmaModePlugin::new(None));
        app.update();
        assert_eq!(
            *app.world().resource::<State<AppMode>>(),
            State::new(AppMode::Ozma),
        );
    }
}
```

- [ ] **Step 5: Run test to verify it fails (because `exit`, `layout`, `spawn` modules are empty)**

```bash
cargo test -p ozma_mode 2>&1 | grep -E "error|FAILED"
```

Expected: compile errors for missing items in `exit`, `layout`, `spawn`.

- [ ] **Step 6: Create stub files so the crate compiles**

Create `crates/ozma_mode/src/exit.rs`:
```rust
//! Child-process exit observer: sends `AppExit` when the shell quits.
use bevy::prelude::*;
use ozma_tty_engine::TerminalChildExit;

/// Observer fired when the PTY child exits. Sends `AppExit::Success`.
pub(crate) fn on_child_exit(_ev: On<TerminalChildExit>, mut exit: EventWriter<AppExit>) {
    exit.write(AppExit::Success);
}
```

Create `crates/ozma_mode/src/layout.rs`:
```rust
//! Window-fill resize system for the Ozma terminal.
use bevy::prelude::*;
use crate::spawn::OzmaTerminal;
use ozma_tty_engine::{Coalescer, PtyHandle, TerminalHandle};
use ozma_tty_renderer::TerminalCellMetricsResource;

/// Tracks the last (cols, rows) sent to the terminal to guard against
/// redundant resize calls.
#[derive(Resource, Default)]
pub(crate) struct OzmaLastSize(pub(crate) Option<(u16, u16)>);

/// Resizes the Ozma terminal to fill the primary window.
pub(crate) fn resize_to_window(
    mut _commands: Commands,
    mut _last_size: ResMut<OzmaLastSize>,
    mut _terminal_q: Query<
        (Entity, &mut TerminalHandle, &mut PtyHandle, &mut Coalescer),
        With<OzmaTerminal>,
    >,
    _metrics: Res<TerminalCellMetricsResource>,
    _window_q: Query<&Window, With<PrimaryWindow>>,
) {
    // TODO: implement in Task 5
}
```

Create `crates/ozma_mode/src/spawn.rs`:
```rust
//! Terminal spawn and despawn for Ozma mode.
use bevy::prelude::*;

/// Marker component identifying the single Ozma terminal entity.
#[derive(Component)]
pub(crate) struct OzmaTerminal;

/// Spawns the Ozma PTY terminal on mode entry.
pub(crate) fn spawn_terminal(_commands: Commands) {
    // TODO: implement in Task 3
}

/// Despawns the Ozma terminal on mode exit.
pub(crate) fn despawn_terminal(
    mut commands: Commands,
    terminal_q: Query<Entity, With<OzmaTerminal>>,
) {
    for entity in terminal_q.iter() {
        commands.entity(entity).despawn();
    }
}
```

- [ ] **Step 7: Run the test to confirm it passes**

```bash
cargo test -p ozma_mode plugin_registers_state 2>&1 | tail -5
```

Expected: `test tests::plugin_registers_state_and_defaults_to_ozma ... ok`

- [ ] **Step 8: Lint**

```bash
cargo clippy -p ozma_mode --fix --allow-dirty --allow-staged && cargo fmt
```

- [ ] **Step 9: Commit**

```bash
git add crates/ozma_mode/
git commit -m "feat(ozma_mode): scaffold crate with AppMode State and plugin stub"
```

---

### Task 3: Implement `spawn.rs` — shell resolution and terminal spawn

**Files:**
- Modify: `crates/ozma_mode/src/spawn.rs`

**Interfaces:**
- Consumes (from Task 2):
  - `OzmaModeConfig { shell: Option<String> }` — `Res<OzmaModeConfig>` in system
  - `OzmaTerminal` marker component — already defined
- Consumes (from `ozma_tty_engine`):
  - `TerminalBundle::spawn(SpawnOptions) -> anyhow::Result<TerminalBundle>`
  - `SpawnOptions { cols: u16, rows: u16, shell: String, cwd: Option<PathBuf>, env: Vec<(String, String)>, osc_webview_gate: Arc<AtomicBool> }`
- Consumes (from `ozma_tty_renderer`):
  - `TerminalRenderBundle::new(Handle<TerminalUiMaterial>) -> TerminalRenderBundle`
  - `TerminalCellMetricsResource { metrics: CellMetrics { advance_phys: f32, line_height_phys: f32 } }`
  - `TerminalUiMaterial` — from `ozma_tty_renderer::material`
- Produces:
  - `resolve_shell(config: Option<&str>, env_shell: Option<&str>) -> String` — `pub(crate)` free function
  - `spawn_terminal` system (replaces stub)
  - `despawn_terminal` system (already implemented)

- [ ] **Step 1: Write failing tests for `resolve_shell`**

Add to `crates/ozma_mode/src/spawn.rs` (replace stub contents):

```rust
//! Terminal spawn and despawn for Ozma mode.
use bevy::prelude::*;
use ozma_tty_engine::TerminalHandle;

/// Marker component identifying the single Ozma terminal entity.
#[derive(Component)]
pub(crate) struct OzmaTerminal;

/// Resolves the shell program to launch.
///
/// Priority: `config` → `env_shell` → `"/bin/sh"`.
pub(crate) fn resolve_shell(config: Option<&str>, env_shell: Option<&str>) -> String {
    todo!()
}

pub(crate) fn spawn_terminal(_commands: Commands) {}

pub(crate) fn despawn_terminal(
    mut commands: Commands,
    terminal_q: Query<Entity, With<OzmaTerminal>>,
) {
    for entity in terminal_q.iter() {
        commands.entity(entity).despawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_shell_uses_config_first() {
        assert_eq!(
            resolve_shell(Some("/bin/fish"), Some("/bin/zsh")),
            "/bin/fish"
        );
    }

    #[test]
    fn resolve_shell_falls_back_to_env() {
        assert_eq!(resolve_shell(None, Some("/bin/zsh")), "/bin/zsh");
    }

    #[test]
    fn resolve_shell_falls_back_to_sh() {
        assert_eq!(resolve_shell(None, None), "/bin/sh");
    }
}
```

- [ ] **Step 2: Run tests to confirm they fail**

```bash
cargo test -p ozma_mode resolve_shell 2>&1 | grep -E "FAILED|panicked"
```

Expected: tests panic on `todo!()`.

- [ ] **Step 3: Implement `resolve_shell`**

Replace the `todo!()` body:

```rust
pub(crate) fn resolve_shell(config: Option<&str>, env_shell: Option<&str>) -> String {
    config
        .or(env_shell)
        .unwrap_or("/bin/sh")
        .to_string()
}
```

- [ ] **Step 4: Run resolve_shell tests — all must pass**

```bash
cargo test -p ozma_mode resolve_shell 2>&1 | tail -5
```

Expected: `3 passed; 0 failed`

- [ ] **Step 5: Implement `spawn_terminal` system**

Replace the full `crates/ozma_mode/src/spawn.rs` with:

```rust
//! Terminal spawn and despawn for Ozma mode.
use crate::OzmaModeConfig;
use bevy::prelude::*;
use ozma_tty_engine::{SpawnOptions, TerminalBundle};
use ozma_tty_renderer::TerminalCellMetricsResource;
use ozma_tty_renderer::material::TerminalUiMaterial;
use ozma_tty_renderer::prelude::TerminalRenderBundle;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// Marker component identifying the single Ozma terminal entity.
#[derive(Component)]
pub(crate) struct OzmaTerminal;

/// Resolves the shell program to launch.
///
/// Priority: `config` → `env_shell` → `"/bin/sh"`.
pub(crate) fn resolve_shell(config: Option<&str>, env_shell: Option<&str>) -> String {
    config
        .or(env_shell)
        .unwrap_or("/bin/sh")
        .to_string()
}

/// Spawns the Ozma terminal on `OnEnter(AppMode::Ozma)`.
///
/// Reads `OzmaModeConfig.shell` for the configured shell override;
/// falls back to `$SHELL` then `/bin/sh`. Initial terminal dimensions
/// are derived from the primary window and font metrics if available,
/// otherwise default to 80×24.
pub(crate) fn spawn_terminal(
    mut commands: Commands,
    mut materials: ResMut<Assets<TerminalUiMaterial>>,
    config: Res<OzmaModeConfig>,
    metrics: Option<Res<TerminalCellMetricsResource>>,
    window_q: Query<&Window, With<PrimaryWindow>>,
) {
    let (cols, rows) = metrics
        .as_ref()
        .zip(window_q.single().ok())
        .map(|(m, w)| {
            let cell_w = m.metrics.advance_phys.floor().max(1.0);
            let cell_h = m.metrics.line_height_phys.floor().max(1.0);
            cells_for(
                w.resolution.physical_width(),
                w.resolution.physical_height(),
                cell_w,
                cell_h,
            )
        })
        .unwrap_or((80, 24));

    let shell = resolve_shell(
        config.shell.as_deref(),
        std::env::var("SHELL").ok().as_deref(),
    );

    let opts = SpawnOptions {
        cols,
        rows,
        shell,
        cwd: None,
        env: Vec::new(),
        osc_webview_gate: Arc::new(AtomicBool::new(false)),
    };

    match TerminalBundle::spawn(opts) {
        Ok(bundle) => {
            let material = materials.add(TerminalUiMaterial::default());
            commands.spawn((
                bundle,
                TerminalRenderBundle::new(material),
                OzmaTerminal,
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    top: Val::Px(0.0),
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    ..default()
                },
            ));
        }
        Err(e) => tracing::error!(?e, "failed to spawn ozma terminal"),
    }
}

/// Despawns the Ozma terminal on `OnExit(AppMode::Ozma)`.
pub(crate) fn despawn_terminal(
    mut commands: Commands,
    terminal_q: Query<Entity, With<OzmaTerminal>>,
) {
    for entity in terminal_q.iter() {
        commands.entity(entity).despawn();
    }
}

fn cells_for(w_px: u32, h_px: u32, cell_w: f32, cell_h: f32) -> (u16, u16) {
    let cols = ((w_px as f32 / cell_w).floor() as u16).max(1);
    let rows = ((h_px as f32 / cell_h).floor() as u16).max(1);
    (cols, rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_shell_uses_config_first() {
        assert_eq!(
            resolve_shell(Some("/bin/fish"), Some("/bin/zsh")),
            "/bin/fish"
        );
    }

    #[test]
    fn resolve_shell_falls_back_to_env() {
        assert_eq!(resolve_shell(None, Some("/bin/zsh")), "/bin/zsh");
    }

    #[test]
    fn resolve_shell_falls_back_to_sh() {
        assert_eq!(resolve_shell(None, None), "/bin/sh");
    }

    #[test]
    fn cells_for_divides_and_floors() {
        assert_eq!(cells_for(800, 600, 8.0, 16.0), (100, 37));
        assert_eq!(cells_for(0, 0, 8.0, 16.0), (1, 1));
        assert_eq!(cells_for(807, 607, 8.0, 16.0), (100, 37));
    }
}
```

- [ ] **Step 6: Run all tests**

```bash
cargo test -p ozma_mode 2>&1 | tail -8
```

Expected: all tests pass.

- [ ] **Step 7: Lint**

```bash
cargo clippy -p ozma_mode --fix --allow-dirty --allow-staged && cargo fmt
```

- [ ] **Step 8: Commit**

```bash
git add crates/ozma_mode/src/spawn.rs
git commit -m "feat(ozma_mode): implement shell resolution and terminal spawn"
```

---

### Task 4: Implement `exit.rs` — child exit observer

**Files:**
- Modify: `crates/ozma_mode/src/exit.rs`

**Interfaces:**
- Consumes (from `ozma_tty_engine`): `TerminalChildExit` — `EntityEvent`, triggered via `commands.trigger_targets(ev, entity)`.
- Produces: `on_child_exit` observer registered in `OzmaModePlugin::build`.

- [ ] **Step 1: Write a failing test**

Add to `crates/ozma_mode/src/exit.rs`:

```rust
//! Child-process exit observer: sends `AppExit` when the shell quits.

use bevy::prelude::*;
use ozma_tty_engine::TerminalChildExit;

/// Observer fired when the PTY child process exits.
///
/// Sends `AppExit::Success` regardless of the exit code — the user
/// closed their shell, so the application should quit.
pub(crate) fn on_child_exit(_ev: On<TerminalChildExit>, mut exit: EventWriter<AppExit>) {
    exit.write(AppExit::Success);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_exit_sends_app_exit() {
        #[derive(Resource, Default)]
        struct GotExit(bool);

        fn capture(mut reader: EventReader<AppExit>, mut flag: ResMut<GotExit>) {
            if reader.read().next().is_some() {
                flag.0 = true;
            }
        }

        let mut app = App::new();
        app.add_event::<AppExit>();
        app.add_observer(on_child_exit);
        app.init_resource::<GotExit>();
        app.add_systems(Update, capture);

        let entity = app.world_mut().spawn_empty().id();
        app.world_mut()
            .trigger_targets(TerminalChildExit { entity, code: Some(0) }, entity);
        app.update();

        assert!(
            app.world().resource::<GotExit>().0,
            "AppExit should have been sent on TerminalChildExit"
        );
    }
}
```

- [ ] **Step 2: Run the test to confirm it fails**

```bash
cargo test -p ozma_mode child_exit 2>&1 | grep -E "FAILED|error"
```

Expected: compile or test failure (stub has wrong signature or AppExit not registered).

- [ ] **Step 3: Check if `AppExit` needs `add_event` or `add_message`**

Run the test as written. If it fails with "resource not found" for `Events<AppExit>`, switch to:

```rust
app.add_message::<AppExit>();
// and use MessageReader<AppExit> instead of EventReader<AppExit>
```

The correct variant depends on how Bevy 0.18 registers `AppExit`. If `EventWriter<AppExit>` causes a compile error in `on_child_exit`, switch to `MessageWriter<AppExit>` (imported from `bevy::ecs::message::MessageWriter`) and adjust the test's `capture` system similarly.

Adjust `on_child_exit` accordingly:

```rust
// If MessageWriter is required:
use bevy::ecs::message::MessageWriter;

pub(crate) fn on_child_exit(_ev: On<TerminalChildExit>, mut exit: MessageWriter<AppExit>) {
    exit.write(AppExit::Success);
}
```

And test `capture`:
```rust
use bevy::ecs::message::MessageReader;
fn capture(mut reader: MessageReader<AppExit>, mut flag: ResMut<GotExit>) {
    if reader.read().next().is_some() { flag.0 = true; }
}
// and app.add_message::<AppExit>();
```

- [ ] **Step 4: Run test to confirm it passes**

```bash
cargo test -p ozma_mode child_exit 2>&1 | tail -5
```

Expected: `test exit::tests::child_exit_sends_app_exit ... ok`

- [ ] **Step 5: Lint**

```bash
cargo clippy -p ozma_mode --fix --allow-dirty --allow-staged && cargo fmt
```

- [ ] **Step 6: Commit**

```bash
git add crates/ozma_mode/src/exit.rs
git commit -m "feat(ozma_mode): add child-exit observer"
```

---

### Task 5: Implement `layout.rs` — resize terminal to window

**Files:**
- Modify: `crates/ozma_mode/src/layout.rs`

**Interfaces:**
- Consumes (from Task 3): `OzmaTerminal` marker component.
- Consumes (from `ozma_tty_engine`):
  - `TerminalHandle::resize(&mut self, pty: &mut PtyHandle, coalescer: &mut Coalescer, cols: u16, rows: u16) -> anyhow::Result<()>`
  - `TerminalHandle::emit_pending(&mut self, commands: &mut Commands, entity: Entity)`
- Consumes (from `ozma_tty_renderer`): `TerminalCellMetricsResource { metrics: CellMetrics { advance_phys: f32, line_height_phys: f32 } }`
- Produces: `OzmaLastSize` resource, `resize_to_window` system (replaces stub).

- [ ] **Step 1: Write a smoke test**

Add to `crates/ozma_mode/src/layout.rs`:

```rust
//! Window-fill resize system for the Ozma terminal.
use crate::spawn::OzmaTerminal;
use bevy::prelude::*;
use ozma_tty_engine::{Coalescer, PtyHandle, TerminalHandle};
use ozma_tty_renderer::TerminalCellMetricsResource;

/// Tracks the last (cols, rows) sent to the terminal to guard
/// against redundant resize calls when the run condition fires.
#[derive(Resource, Default)]
pub(crate) struct OzmaLastSize(pub(crate) Option<(u16, u16)>);

/// Resizes the Ozma terminal to fill the primary window.
///
/// Gated by `run_if` at registration — only runs on
/// `TerminalCellMetricsResource` change or `WindowResized`.
pub(crate) fn resize_to_window(
    mut commands: Commands,
    mut last_size: ResMut<OzmaLastSize>,
    mut terminal_q: Query<
        (Entity, &mut TerminalHandle, &mut PtyHandle, &mut Coalescer),
        With<OzmaTerminal>,
    >,
    metrics: Res<TerminalCellMetricsResource>,
    window_q: Query<&Window, With<PrimaryWindow>>,
) {
    let Ok(window) = window_q.single() else {
        return;
    };
    let Ok((entity, mut handle, mut pty, mut coalescer)) = terminal_q.single_mut() else {
        return;
    };

    let cell_w = metrics.metrics.advance_phys.floor().max(1.0);
    let cell_h = metrics.metrics.line_height_phys.floor().max(1.0);
    let cols = ((window.resolution.physical_width() as f32 / cell_w).floor() as u16).max(1);
    let rows = ((window.resolution.physical_height() as f32 / cell_h).floor() as u16).max(1);

    if last_size.0 == Some((cols, rows)) {
        return;
    }

    match handle.resize(&mut pty, &mut coalescer, cols, rows) {
        Ok(()) => {
            last_size.0 = Some((cols, rows));
            handle.emit_pending(&mut commands, entity);
        }
        Err(e) => tracing::warn!(?e, cols, rows, "failed to resize ozma terminal"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resize_to_window_does_not_panic_without_entities() {
        let mut app = App::new();
        app.init_resource::<OzmaLastSize>();
        app.add_systems(Update, resize_to_window);
        // No window, no terminal, no metrics — system should be a no-op.
        app.update();
    }
}
```

- [ ] **Step 2: Also register `OzmaLastSize` in `OzmaModePlugin::build`**

In `crates/ozma_mode/src/lib.rs`, add `init_resource::<layout::OzmaLastSize>()` to the `build` chain:

```rust
impl Plugin for OzmaModePlugin {
    fn build(&self, app: &mut App) {
        app.init_state::<AppMode>()
            .insert_resource(OzmaModeConfig {
                shell: self.config_shell.clone(),
            })
            .init_resource::<layout::OzmaLastSize>()   // ← add this line
            .add_observer(exit::on_child_exit)
            .add_systems(OnEnter(AppMode::Ozma), spawn::spawn_terminal)
            .add_systems(
                Update,
                layout::resize_to_window
                    .run_if(in_state(AppMode::Ozma))
                    .run_if(
                        resource_exists_and_changed::<layout::OzmaLastSize>
                            .or(resource_exists_and_changed::<ozma_tty_renderer::TerminalCellMetricsResource>)
                            .or(on_event::<bevy::window::WindowResized>),
                    ),
            )
            .add_systems(OnExit(AppMode::Ozma), spawn::despawn_terminal);
    }
}
```

- [ ] **Step 3: Run all tests**

```bash
cargo test -p ozma_mode 2>&1 | tail -8
```

Expected: all tests pass. If the smoke test panics because `TerminalCellMetricsResource` is absent and `Res<>` panics on missing resource, wrap with `Option<Res<TerminalCellMetricsResource>>` and return early:

```rust
pub(crate) fn resize_to_window(
    mut commands: Commands,
    mut last_size: ResMut<OzmaLastSize>,
    mut terminal_q: Query<
        (Entity, &mut TerminalHandle, &mut PtyHandle, &mut Coalescer),
        With<OzmaTerminal>,
    >,
    metrics: Option<Res<TerminalCellMetricsResource>>,
    window_q: Query<&Window, With<PrimaryWindow>>,
) {
    let Some(metrics) = metrics else { return; };
    // ... rest unchanged
```

- [ ] **Step 4: Run the full workspace test suite**

```bash
cargo test 2>&1 | tail -10
```

Expected: all crates pass. Fix any issues before committing.

- [ ] **Step 5: Lint**

```bash
make fix-lint
```

- [ ] **Step 6: Commit**

```bash
git add crates/ozma_mode/src/layout.rs crates/ozma_mode/src/lib.rs
git commit -m "feat(ozma_mode): implement window-fill resize system"
```

---

## Self-Review Checklist

| Spec requirement | Covered by |
|---|---|
| `AppMode::Ozma` / `AppMode::Ozmux` enum, `Default = Ozma` | Task 2 `lib.rs` |
| `AppMode` re-exported from `ozma_mode` crate | Task 2 `lib.rs` (`pub enum AppMode`) |
| `[ozma] shell` config with `$SHELL` fallback | Task 1 `ozma.rs`, Task 3 `spawn.rs` |
| `OzmaConfig` / `OzmaPatch` in `ozmux_configs` | Task 1 |
| `RawConfigs.ozma` + `apply_to` arm | Task 1 `raw.rs` |
| `OnEnter(AppMode::Ozma)` spawns `TerminalBundle` + `TerminalRenderBundle` | Task 3 `spawn.rs` |
| `OzmaTerminal` marker component | Task 3 `spawn.rs` |
| `TerminalChildExit` → `AppExit` via observer | Task 4 `exit.rs` |
| `resize_to_window` gated by `run_if`, not in-body early return (except single/not-found guards) | Task 5 `layout.rs` |
| `OzmaLastSize` resource guards redundant resizes | Task 5 |
| `despawn_terminal` on `OnExit(AppMode::Ozma)` | Task 3 stub, live from Task 2 |
| Shell resolution tests (config / env / fallback) | Task 3 |
| `OzmaConfig` tests (default / patch / TOML parse) | Task 1 |
| `child_exit_sends_app_exit` test | Task 4 |
| `plugin_registers_state` test | Task 2 |
| `main.rs` not modified | ✓ (no task touches `src/main.rs`) |
