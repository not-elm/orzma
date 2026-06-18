# ozma_mode Crate Design

## Overview

Introduce `crates/ozma_mode` — a new Bevy plugin crate that implements the Ozma mode: a
single PTY terminal running directly through Alacritty's VT emulator, with no tmux
dependency. This crate also owns the `AppMode` Bevy `States` enum shared across future mode
crates (`ozmux_mode`).

The immediate deliverable is the crate itself, ready to be plugged into `main.rs` in a
later roadmap step (step 5/6). The binary (`src/main.rs`) is not modified in this task.

## Background

`docs/memo.md` defines two operating modes:

- **Ozma mode** — single terminal, direct PTY, no tmux
- **Ozmux mode** — tmux backend, multiplexer, current default

Roadmap step 1 is to produce a complete `ozma_mode` crate. Steps 5 and 6 wire up the
`AppMode` state and mode switching in the binary.

## Crate Structure

```
crates/ozma_mode/
  Cargo.toml
  src/
    lib.rs      — AppMode definition + re-export, OzmaModePlugin
    spawn.rs    — OnEnter(AppMode::Ozma): spawn TerminalBundle with resolved shell
    layout.rs   — Update: resize terminal to fill the primary window
    exit.rs     — Update: TerminalChildExit → AppExit
```

No `mod.rs` files — Rust 2018+ module layout per `.claude/rules/rust.md`.

### Dependencies (`Cargo.toml`)

```toml
[dependencies]
bevy              = { workspace = true }
ozma_tty_engine   = { path = "../ozma_tty_engine" }
ozma_tty_renderer = { path = "../ozma_tty_renderer" }
ozmux_configs     = { path = "../configs" }
tracing           = { workspace = true }
```

## `AppMode` State

Defined in `crates/ozma_mode/src/lib.rs` and re-exported as the crate's public API so that
the future `crates/ozmux_mode` can depend on `ozma_mode` for the shared state type.

```rust
#[derive(States, Debug, Clone, PartialEq, Eq, Hash, Default)]
pub enum AppMode {
    #[default]
    Ozma,
    Ozmux,
}
```

`AppMode::Ozma` is the `Default`, so `app.init_state::<AppMode>()` in the binary starts in
Ozma mode without any additional configuration.

## `OzmaModePlugin` Systems

| Schedule | Run condition | System | Responsibility |
|---|---|---|---|
| `OnEnter(AppMode::Ozma)` | — | `spawn_terminal` | Resolve shell, spawn `TerminalBundle` + `TerminalRenderBundle` |
| `Update` | `in_state(AppMode::Ozma)` + `resource_exists_and_changed::<TerminalCellMetricsResource>.or(on_event::<WindowResized>)` | `resize_to_window` | Track primary window size, resize terminal |
| observer | — | `on_child_exit` | `On<TerminalChildExit>`: send `AppExit` via `MessageWriter` |
| `OnExit(AppMode::Ozma)` | — | `despawn_terminal` | Despawn terminal entity (future mode switch) |

All `Update` systems use `.run_if(in_state(AppMode::Ozma))` at registration — no in-body
early returns. `on_child_exit` is registered as `app.add_observer(on_child_exit)` since
`TerminalChildExit` is an `EntityEvent` dispatched via `commands.trigger(...)`, not a
Bevy message.

### Shell Resolution (`spawn.rs`)

Resolution order:

1. `configs.ozma.shell` — value from `[ozma]` section of `config.toml`
2. `std::env::var("SHELL")` — current user's shell
3. `"/bin/sh"` — guaranteed fallback

Environment variable lookup happens in `spawn.rs`, not inside `ozmux_configs`, so the
`configs` crate remains free of environment variable dependencies.

### Terminal Marker Component

`spawn_terminal` inserts an `OzmaTerminal` marker component on the spawned entity so that
`resize_to_window`, `handle_child_exit`, and `despawn_terminal` can query it unambiguously.

```rust
#[derive(Component)]
struct OzmaTerminal;
```

### Layout (`layout.rs`)

`resize_to_window` queries the `PrimaryWindow` for its physical size, reads cell metrics
from `Res<TerminalCellMetricsResource>` (inserted at `Startup` by `TerminalFontPlugin`),
computes `cols = floor(phys_w / advance_phys)` and `rows = floor(phys_h / line_height_phys)`
— the same formula as `sync_client_size` in `src/tmux/render.rs:534-541` — and calls the
resize API on the terminal. Cell counts are clamped to `1..=u16::MAX` before the call.
Gated with `run_if(resource_exists_and_changed::<TerminalCellMetricsResource>.or(on_event::<WindowResized>))`
so the system runs only on font or window size changes, not every frame.

### Exit (`exit.rs`)

`on_child_exit` is a Bevy observer (`fn on_child_exit(ev: On<TerminalChildExit>, mut exit: MessageWriter<AppExit>)`)
registered via `app.add_observer(on_child_exit)`. On receipt it sends `AppExit::Success` via
`MessageWriter<AppExit>`. No restart logic — app quits.

## Config Extension

A new `OzmaConfig` type is added to `crates/configs`:

**`crates/configs/src/ozma.rs`** (new file, mirrors `tmux.rs` pattern):

```rust
pub struct OzmaConfig {
    pub shell: Option<String>,
}

impl Default for OzmaConfig {
    fn default() -> Self {
        Self { shell: None }
    }
}
```

`OzmuxConfigs` gains a new field:

```rust
pub struct OzmuxConfigs {
    // … existing fields …
    pub ozma: OzmaConfig,
}
```

Config TOML:

```toml
[ozma]
shell = "/bin/zsh"   # optional; defaults to $SHELL → /bin/sh
```

`OzmaPatch` (deserialization struct) uses `#[serde(deny_unknown_fields)]` and an
`Option<String>` for `shell`, matching the established pattern in `tmux.rs`.

`crates/configs/src/raw.rs` also requires a new `pub(crate) ozma: Option<OzmaPatch>` field
on `RawConfigs` and a corresponding `apply_to` branch in `OzmuxConfigs`. Without this,
`[ozma]` in the user's config TOML would be rejected at parse time because `RawConfigs`
uses `#[serde(deny_unknown_fields)]`.

## Testing

### `crates/ozma_mode`

| Test | What it checks |
|---|---|
| `shell_resolution_uses_config` | When `OzmaConfig { shell: Some(...) }`, that value is used |
| `shell_resolution_falls_back_to_env` | `OzmaConfig { shell: None }` + `$SHELL` set → env value used |
| `shell_resolution_falls_back_to_sh` | `OzmaConfig { shell: None }` + `$SHELL` unset → `"/bin/sh"` |
| `plugin_registers_state` | `App::new().add_plugins(OzmaModePlugin)` does not panic; `AppMode::Ozma` is the initial state |
| `child_exit_sends_app_exit` | Writing `TerminalChildExit` message + `app.update()` triggers `AppExit` |

PTY spawn and GPU rendering are excluded from unit tests. `spawn_terminal` and
`despawn_terminal` are smoke-tested via `App::new()` without a real PTY.

### `crates/configs`

Following the pattern in `tmux.rs`:

| Test | What it checks |
|---|---|
| `default_ozma_shell_is_none` | `OzmaConfig::default().shell == None` |
| `patch_overrides_shell` | `OzmaPatch { shell: Some(...) }.apply_to(default)` sets the field |
| `empty_patch_keeps_base` | `OzmaPatch::default().apply_to(default)` is a no-op |

## Invariants

- `OzmaTerminal` is inserted by `spawn_terminal` and queried exclusively by ozma-mode
  systems; no other plugin may insert or remove it.
- `despawn_terminal` runs on `OnExit(AppMode::Ozma)` and is the only place the
  `OzmaTerminal` entity is despawned, ensuring a clean state before entering another mode.
- Shell resolution never panics; the `/bin/sh` fallback is always reachable.
