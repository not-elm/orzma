# ozma_terminal: Internalized Input + Action Modules

**Date:** 2026-06-19
**Status:** Draft

## Overview

Make `crates/ozma_terminal` a self-contained, embeddable VT terminal: a
consumer spawns one bundle and gets a working terminal — keystrokes reach the
PTY, `Cmd+V` pastes — with **no input wiring of its own**. Two new modules move
the default keyboard handling and the PTY-level actions into the crate:

- `input` — the default `KeyboardInput` dispatcher. Reads key presses, decides
  per press whether to forward a raw key to the PTY, fire a crate action, or
  skip it for the host to handle.
- `action` — PTY-level operations modeled as `EntityEvent`s with observers,
  following the historical `src/action/` pattern (commit `461ca35`): one event
  struct + one observer (sub-plugin) per action, aggregated by a plugin. The
  first and only citizen this iteration is **Paste**.

A marker component (`InputDisabled`) lets the host switch the crate's default
input off per entity, so a host that routes input elsewhere (tmux, a focused
webview, an open picker, IME composition) can delegate without the crate
knowing about any application-level concept.

The spawn API becomes `commands.spawn(OzmaTerminalBundle::spawn(opts)?)` —
the bundle carries the engine terminal, the `OzmaTerminal` marker, and a
default full-screen `Node`; an `On<Add, OzmaTerminal>` observer injects the GPU
render bundle (allocating the material where it has `World` access).

## Motivation

Today the binary owns all of this:

- `src/ozma_input.rs` (`OzmaInputPlugin`) reads `KeyboardInput`, encodes raw
  keys into the engine's `TerminalKeyInput`, and handles GUI shortcuts
  (`Quit`, `Paste`, `OpenPicker`, `ReleaseInlineFocus`, `DetachSession`) inline,
  gated on `run_if(in_state(AppMode::Ozma))`.
- `src/ozma.rs` `spawn_terminal` hand-assembles `TerminalBundle::spawn` +
  `TerminalRenderBundle` + `OzmaTerminal` + `Node`, reading `Assets`, window,
  and metrics.

This means any consumer of `ozma_terminal` must re-implement keyboard handling
and bundle assembly. The crate should instead ship a usable default, exactly as
a reusable terminal component library would.

### Key insight: the engine already owns raw-key → PTY

`ozma_tty_engine` already defines `TerminalKeyInput` (an `EntityEvent`) and an
observer (`on_terminal_key_input`, registered by `TerminalHandlePlugin`) that
encodes the key against the entity's terminal mode and writes the VT bytes to
the PTY. Raw-key forwarding is therefore **already event-driven and already in a
crate** — the new modules do not re-implement it. This sharpens the layering:

| Tier | Responsibility | Home |
|---|---|---|
| **1** Raw key → PTY bytes | Encode `TerminalKey` → write to PTY | `ozma_tty_engine` (existing, unchanged) |
| **2** PTY-level operations | Paste; later scroll / copy / selection | **`ozma_terminal::action`** (new) |
| **3** Application operations | Quit, OpenPicker, DetachSession, ReleaseInlineFocus | **binary** (caller's responsibility) |

`input` is the **dispatcher** that, per key press, fires a Tier-1
`TerminalKeyInput`, fires a Tier-2 action event, or skips the press for the
host's Tier-3 layer.

## Goals

- `commands.spawn(OzmaTerminalBundle::spawn(opts)?)` yields a fully working
  terminal with no consumer-side input or render wiring.
- Default keyboard handling (raw-key forwarding + built-in Paste) lives inside
  the crate.
- PTY-level actions are `EntityEvent`s processed by observers in `action`.
- The crate has **no dependency** on `AppMode`, `SessionPicker`, `ImeState`,
  `FocusedWebview`, or `ozmux_configs`.
- The host can disable the crate's default input per entity via `InputDisabled`.
- Behavior in the running ozmux binary is preserved exactly (same chords, same
  PTY bytes, same guards).

## Non-Goals

- **Mouse input** (scroll, drag-selection) — deferred to a future `mouse`
  module. No mouse handling is added to the crate here.
- **Scroll / Copy / text-selection actions** — these are mouse-driven and land
  with the mouse module. `action` is structured to accept them but ships only
  `Paste`.
- **Application-level action handlers** — `Quit`, `OpenPicker`,
  `DetachSession`, `ReleaseInlineFocus` stay in the binary. The crate never
  learns what they mean.
- No change to the engine (`ozma_tty_engine`) or renderer
  (`ozma_tty_renderer`).

## Architecture

### Crate module map (`crates/ozma_terminal/src`)

```
lib.rs        OzmaTerminalPlugin — aggregates Exit + Layout + Input + Action plugins
spawn.rs      OzmaTerminal (marker), OzmaTerminalBundle::spawn(opts), On<Add> render injection,
              cells_for, resolve_shell, OzmaSpawnOptions
layout.rs     (existing) window-fill resize
exit.rs       (existing) child-process exit → AppExit
input.rs      (new) OzmaInputPlugin, InputDisabled, ReservedChords, key dispatcher
action.rs     (new) OzmaActionPlugin, PasteAction (EntityEvent) + observer
clipboard.rs  (new, moved from src/) Clipboard resource + build_paste_bytes
```

### Three readers, cleanly partitioned

While a terminal is active (no `InputDisabled`), two systems read
`KeyboardInput`: the crate's dispatcher and the binary's app-shortcut handler.
They never conflict because the partition is total and **order-independent**:

- The crate **skips** any press in `ReservedChords` (the host's chords).
- The crate **owns** its built-in action chord (`Cmd+V` → Paste); this chord is
  **excluded** from `ReservedChords`.
- The crate **forwards** every other press to the PTY as `TerminalKeyInput`.
- The binary's app-shortcut handler acts **only** on its reserved chords
  (`Quit`, `OpenPicker`, `DetachSession`, `ReleaseInlineFocus`) and no longer
  handles Paste.

No `.before()/.after()` ordering is required between the two readers.

## Component Specification

### `crates/ozma_terminal/src/clipboard.rs` (moved from `src/clipboard.rs`)

The `Clipboard` resource (an `arboard` wrapper) and the pure `build_paste_bytes`
helper move into the crate verbatim, with their existing tests. `Paste`'s
observer needs clipboard read access, and the clipboard is terminal
infrastructure, so it belongs here.

- `Clipboard` becomes `pub` (consumed by the crate's `PasteAction` observer and,
  externally, by the binary's tmux copy-mode code).
- `build_paste_bytes` stays `pub(crate)` (only the `PasteAction` observer calls
  it inside the crate).
- The crate gains an `arboard` dependency.
- Headless behavior is unchanged: `arboard` init failure leaves the inner
  `Option` `None`, and reads/writes no-op.

**Consumer fan-out:** every binary reference to `crate::clipboard::Clipboard`
(`src/ozma_input.rs`, `src/tmux/mouse.rs`, copy-mode code) re-points to
`ozma_terminal::Clipboard`. This is mechanical. `src/clipboard.rs` is deleted.

### `crates/ozma_terminal/src/action.rs` (new)

Mirrors the historical `src/action/` shape — one event + one observer
(sub-plugin) per action, aggregated by a plugin — sized to the single action
that exists this iteration.

```rust
/// Pastes the system clipboard into the target terminal entity's PTY.
#[derive(EntityEvent, Debug, Clone)]
pub struct PasteAction {
    #[event_target]
    pub entity: Entity,
}

/// Aggregates the crate's PTY-level action observers.
pub struct OzmaActionPlugin;

impl Plugin for OzmaActionPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_paste);
    }
}
```

`on_paste` reproduces the current `ShortcutAction::Paste` arm of
`forward_keys_to_ozma`:

1. `Clipboard::read()` → `None` or empty ⇒ no-op.
2. If the viewport is scrolled back, `scroll_to_bottom`.
3. `build_paste_bytes(&text, handle.bracketed_paste_enabled())`.
4. `TerminalHandle::write(&mut pty, &bytes)`; log a warning on error.

Observer params: `On<PasteAction>`, `ResMut<Clipboard>`, and a `Query<(&mut
TerminalHandle, &mut PtyHandle, &mut Coalescer)>` resolved by `ev.entity`.

**Extensibility:** future mouse-driven actions (`ScrollLinesAction`,
`CopySelectionAction`, …) are added as sibling event structs + observers
registered in `OzmaActionPlugin`. Not built now (YAGNI — no keyboard trigger
exists for them yet).

### `crates/ozma_terminal/src/input.rs` (new)

```rust
/// When present on an `OzmaTerminal` entity, the crate's default input
/// dispatcher skips it entirely — the host routes input elsewhere.
#[derive(Component)]
pub struct InputDisabled;

/// A keyboard chord the default dispatcher must NOT consume because the
/// host application reserves it (e.g. Quit, OpenPicker). Config-agnostic:
/// a physical `KeyCode` plus the four modifier bits.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ReservedChord {
    pub key_code: KeyCode,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub meta: bool,
}

/// Chords the host reserves for its own shortcuts. Empty by default — a
/// spawn-and-go consumer reserves nothing and the crate forwards everything.
#[derive(Resource, Default)]
pub struct ReservedChords(pub Vec<ReservedChord>);

/// Registers the default terminal keyboard dispatcher.
pub struct OzmaInputPlugin;
```

`OzmaInputPlugin` registers `ReservedChords` (default-empty) and the dispatcher
system. The dispatcher:

- Query: `Query<Entity, (With<OzmaTerminal>, Without<InputDisabled>)>` — only
  active terminals. Registered with `run_if(on_message::<KeyboardInput>)`; an
  empty query (no active terminal) makes the body a no-op, so no extra guard is
  needed.
- Computes the current modifier set from `Res<ButtonInput<KeyCode>>` (a
  crate-local copy of `current_modifiers`).
- For each `ButtonState::Pressed` event:
  1. If `(key_code, mods)` is in `ReservedChords` → **skip** (host handles it).
  2. Else if it matches the built-in Paste chord → `trigger(PasteAction { entity })`.
  3. Else map the logical key via `bevy_key_to_terminal_key`; on `Some(key)` →
     `trigger(TerminalKeyInput { entity, key, modifiers })`; on `None` → drop.

`bevy_key_to_terminal_key` moves here from `src/ozma_input.rs` unchanged (with
its tests).

**Built-in Paste chord.** To keep the crate config-free while still matching the
user's configured paste binding, the Paste chord is carried as a host-supplied
resource:

```rust
/// The chord that triggers the built-in Paste action. Host-populated;
/// defaults to `Cmd+V`.
#[derive(Resource)]
pub struct PasteChord(pub ReservedChord);
```

`OzmaInputPlugin` inserts the `Cmd+V` default; the host overrides it from
config (symmetric with `ReservedChords`). Step 2 above matches the press
against `PasteChord`, so paste stays data-driven rather than hardcoded and the
host's configurability is intact.

> Rationale for not hardcoding `Cmd+V`: ozmux lets users rebind paste. The crate
> stays ignorant of `ozmux_configs` but accepts the resolved chord as plain
> data, symmetric with `ReservedChords`.

### `crates/ozma_terminal/src/spawn.rs` (extended)

```rust
/// Options for spawning a standalone Ozma terminal.
#[derive(Default)]
pub struct OzmaSpawnOptions {
    /// Shell override; `None` falls back to `$SHELL` then `/bin/sh`.
    pub shell: Option<String>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
}

/// Self-contained spawn bundle for a standalone Ozma terminal.
#[derive(Bundle)]
pub struct OzmaTerminalBundle {
    terminal: TerminalBundle,   // engine PTY/VT bundle
    marker: OzmaTerminal,
    node: Node,                 // default full-screen
}

impl OzmaTerminalBundle {
    /// Spawns the PTY at a provisional size and returns the bundle.
    /// `layout.rs` fits it to the window on the first frame.
    pub fn spawn(opts: OzmaSpawnOptions) -> anyhow::Result<Self> { /* ... */ }
}
```

`spawn`:

1. `resolve_shell(opts.shell.as_deref(), $SHELL)`.
2. Build engine `SpawnOptions { cols: 80, rows: 24, shell, cwd, env,
   osc_webview_gate: Arc::new(AtomicBool::new(false)) }` — provisional size,
   OSC-webview gate disabled (preserving current Ozma behavior).
3. `TerminalBundle::spawn(opts)?`.
4. Wrap with `OzmaTerminal` and a default full-screen `Node`
   (`position: Absolute`, `0/0`, `100%/100%`).

The provisional `80×24` is corrected to window fill by the existing
`resize_to_window` system on the first frame (`reset_last_size` already fires on
`On<Add, OzmaTerminal>`). The shell has not produced output by then, so there is
no visible flash. A consumer wanting an exact initial size can resize after
spawn; no `cols/rows` field is added (YAGNI).

**Render injection** (new observer in `spawn.rs`):

```rust
fn on_add_inject_render(
    ev: On<Add, OzmaTerminal>,
    mut commands: Commands,
    mut materials: ResMut<Assets<TerminalUiMaterial>>,
) {
    let material = materials.add(TerminalUiMaterial::default());
    commands.entity(ev.entity).insert(TerminalRenderBundle::new(material));
}
```

This is why spawn can be one consumer call: the GPU material (which needs
`Assets`) is allocated in the observer, not by the consumer. The observer fires
during the spawn command flush, so render components are present before the
first `Update`.

### `crates/ozma_terminal/src/lib.rs`

`OzmaTerminalPlugin` aggregates the four sub-plugins as one method chain:

```rust
impl Plugin for OzmaTerminalPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((ExitPlugin, LayoutPlugin, OzmaInputPlugin, OzmaActionPlugin))
            .add_observer(on_add_inject_render);
    }
}
```

`OzmaTerminalConfig` (the crate's shell-override resource) and `config_shell`
are retained as-is. The binary's `spawn_terminal` reads `OzmaTerminalConfig` and
passes `shell` into `OzmaSpawnOptions`; `spawn` no longer reads any resource
itself (it is a plain constructor).

### Binary changes (`src/`)

**`src/ozma_input.rs` — gutted.** The raw-key forwarding and the Paste arm move
to the crate. What remains, split into two small systems:

1. **`InputDisabled` maintainer.** Computes `should_disable = picker.open ||
   ime.is_composing() || !window_focused || focused_webview.is_some()` and
   toggles `InputDisabled` on the `OzmaTerminal` entity. Per the
   change-detection rule, it inserts only when absent-and-should-disable and
   removes only when present-and-should-not — no unconditional rewrite.
   (`AppMode::Ozmux` needs no handling here: the `OzmaTerminal` entity does not
   exist in Ozmux mode, so the crate dispatcher's `With<OzmaTerminal>` query is
   already empty.)

2. **App-shortcut handler.** A faithful extraction of the GUI-action arm of the
   current `forward_keys_to_ozma`: reads `KeyboardInput`, and on a matched
   chord runs `Quit` / `OpenPicker` / `DetachSession` / `ReleaseInlineFocus`.
   The **Paste arm is removed** (the crate owns it). It keeps its existing
   guards (skip when picker open / unfocused; handle `ReleaseInlineFocus` while
   a webview holds focus). It runs independently of the terminal's
   `InputDisabled` so `ReleaseInlineFocus` still works while a webview is
   focused.

**`ReservedChords` population.** Add `ResolvedShortcuts::reserved_chords(&self)
-> Vec<ReservedChord>` mapping every resolved shortcut whose action is **not
Paste** into a crate `ReservedChord`. A `Startup` system (ordered after
`build_resolved_shortcuts`) writes them into the crate's `ReservedChords`
resource, and supplies the resolved Paste chord to the crate's `PasteChord`.

**`src/ozma.rs` `spawn_terminal` — collapses to:**

```rust
match OzmaTerminalBundle::spawn(OzmaSpawnOptions { shell: config.shell.clone(), ..default() }) {
    Ok(bundle) => { commands.spawn(bundle); }
    Err(e) => { tracing::error!(?e, "failed to spawn ozma terminal"); exit.write(AppExit::Success); }
}
```

The window/metrics size computation is removed (layout handles it).

**`src/clipboard.rs` — deleted**, imports re-pointed to `ozma_terminal::Clipboard`.

## Data Flow

```
Startup (binary):
  build_resolved_shortcuts → ResolvedShortcuts
  populate_reserved_chords  → ozma_terminal::ReservedChords  { Cmd+Q, Cmd+Shift+P,
                                                               Ctrl+Shift+Escape, Ctrl+Shift+D }
                            → ozma_terminal::PasteChord       { Cmd+V }     (Paste excluded from reserved)

Per frame, terminal active (Without<InputDisabled>):
  KeyboardInput ─┬─▶ crate dispatcher (ozma_terminal::input)
                 │     reserved?  → skip
                 │     paste chord → trigger PasteAction ─▶ on_paste ─▶ Clipboard::read → PTY write
                 │     else        → trigger TerminalKeyInput ─▶ engine on_terminal_key_input ─▶ PTY write
                 │
                 └─▶ binary app-shortcut handler
                       match_gui_action → Quit / OpenPicker / DetachSession / ReleaseInlineFocus
                       (Paste arm removed)

Per frame, host guard active:
  binary InputDisabled maintainer sets/removes InputDisabled
    (picker open | IME composing | unfocused | webview focused)
  → crate dispatcher skips the entity; binary app-shortcut handler still runs
```

### Behavior parity table

| Input | Current | After |
|---|---|---|
| `a`, `Ctrl+C` | PTY | crate: not reserved → `TerminalKeyInput` → PTY |
| `Cmd+V` | binary pastes | crate: paste chord → `PasteAction` → PTY |
| `Cmd+Q` | binary quits | crate skips (reserved); binary quits |
| `Cmd+Shift+P` | binary opens picker | crate skips (reserved); binary opens picker |
| `Ctrl+Shift+D` (non-meta) | binary detach (no-op in Ozma; not sent to PTY) | crate skips (reserved); binary handles; never leaks to PTY |
| `Ctrl+Shift+Escape` while webview focused | binary releases focus | binary releases focus (runs despite `InputDisabled`) |

## Error Handling

- `OzmaTerminalBundle::spawn` returns `anyhow::Result`; PTY spawn failure
  propagates to the consumer, which decides the response (the binary logs and
  sends `AppExit::Success`, as today).
- `Clipboard` unavailable (headless) → `PasteAction` no-ops.
- Empty `ReservedChords` / unset host → the crate forwards everything and
  pastes on the default chord; a minimal consumer needs zero input code.
- A logical key with no `TerminalKey` mapping (function keys, etc.) is dropped,
  as today.

## Testing

Crate (`crates/ozma_terminal`), all with `MinimalPlugins` + injected
`KeyboardInput`:

- `input.rs`:
  - reserved chord → no `TerminalKeyInput` / `PasteAction` fired.
  - paste chord → `PasteAction` fired for the entity.
  - plain/forwardable key → `TerminalKeyInput` fired with correct `TerminalKey`.
  - `InputDisabled` present → dispatcher fires nothing.
  - `bevy_key_to_terminal_key` unit tests (moved verbatim).
- `action.rs`: `PasteAction` observer writes bracketed vs unbracketed bytes
  (drive `Clipboard` via a seeded value; assert PTY write path), and no-ops on
  empty/unavailable clipboard.
- `clipboard.rs`: existing `build_paste_bytes` / `Clipboard` tests move intact.
- `spawn.rs`: `OzmaTerminalBundle::spawn` composes the expected components;
  `On<Add, OzmaTerminal>` injects `TerminalRenderBundle` (extend the existing
  `reset_last_size` test style).

Binary (`src/`):

- `ResolvedShortcuts::reserved_chords` is a pure function: default bindings
  produce exactly `{Quit, OpenPicker, ReleaseInlineFocus, DetachSession}` chords
  and exclude Paste.
- `InputDisabled` maintainer toggles correctly across the guard permutations and
  obeys conditional change detection (no rewrite when state is unchanged).

## Migration Steps

1. Move `src/clipboard.rs` → `crates/ozma_terminal/src/clipboard.rs`; add
   `arboard` to the crate; make `Clipboard` `pub`; re-point binary imports
   (`ozma_input`, `tmux/mouse`, copy-mode); delete `src/clipboard.rs`.
2. Add `crates/ozma_terminal/src/action.rs` with `PasteAction` + `on_paste` +
   `OzmaActionPlugin`.
3. Add `crates/ozma_terminal/src/input.rs` with `InputDisabled`,
   `ReservedChord`/`ReservedChords`, `PasteChord`, the dispatcher, and the
   moved `bevy_key_to_terminal_key`.
4. Extend `spawn.rs`: `OzmaSpawnOptions`, `OzmaTerminalBundle::spawn`, and the
   `On<Add, OzmaTerminal>` render-injection observer.
5. Wire `OzmaInputPlugin` + `OzmaActionPlugin` + the render observer into
   `OzmaTerminalPlugin::build`.
6. Binary: split `ozma_input.rs` into the `InputDisabled` maintainer + the
   app-shortcut handler (drop raw-key forwarding and the Paste arm); add
   `reserved_chords` + the population `Startup` system; collapse
   `src/ozma.rs` `spawn_terminal` to the bundle call.
7. `cargo test`, `cargo clippy --workspace`, `cargo fmt`. Manual smoke:
   type/paste/quit/open-picker/detach in Ozma mode; confirm Ozmux mode
   (tmux) input is unaffected.

## Decisions & Rationale

- **ReservedChords over modifier-partition or leftover-event.** Default
  bindings include non-`meta` app shortcuts (`Ctrl+Shift+D` detach,
  `Ctrl+Shift+Escape` release-inline-focus), so a "meta = app, non-meta =
  terminal" split leaks those to the PTY. A leftover-event model would still
  need a reserved set to classify them. `ReservedChords` is the minimal
  mechanism that handles arbitrary host bindings, keeps the proven
  `match_gui_action` flow in a normal system, and exposes one small, testable
  seam.
- **Clipboard moves into the crate.** `Paste`'s observer needs clipboard read;
  the clipboard is terminal infrastructure shared with tmux copy-mode. Moving
  it (vs. injecting a trait) is less code and a natural home; `arboard` is
  headless-safe.
- **`InputDisabled` as the single coarse gate.** It absorbs every host-specific
  guard (picker, IME, focus, webview, mode) so the crate stays free of
  `AppMode` / `SessionPicker` / `ImeState` / `FocusedWebview`. The host owns
  the policy; the crate sees one marker.
- **Provisional spawn size + layout correction.** Lets the consumer call one
  spawn function without computing cols/rows; the existing resize system
  already fits to the window before any output is drawn.

## Open Questions

None.
