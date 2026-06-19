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
input.rs      (new) OzmaInputPlugin, InputDisabled, TerminalInputBindings, OzmaTerminalInputSet, key dispatcher
action.rs     (new) OzmaActionPlugin, PasteAction (EntityEvent) + observer
clipboard.rs  (new, moved from src/) Clipboard resource + build_paste_bytes
```

### Three readers, cleanly partitioned

While a terminal is active (no `InputDisabled`), two systems read
`KeyboardInput`: the crate's dispatcher and the binary's app-shortcut handler.
They never conflict because the partition is total and **order-independent**:

- The crate **skips** any press in `bindings.reserved` (the host's chords).
- The crate **owns** its built-in action chord (`bindings.paste`, default
  `Cmd+V` → Paste); this chord is **excluded** from `bindings.reserved`.
- The crate **forwards** every other press to the PTY as `TerminalKeyInput`,
  **except** unhandled `meta`/Cmd-modified chords, which it drops (preserving
  today's behavior where `Cmd+<key>` never reaches the PTY).
- The binary's app-shortcut handler acts **only** on its reserved chords
  (`Quit`, `OpenPicker`, `DetachSession`, `ReleaseInlineFocus`) and no longer
  handles Paste.

No `.before()/.after()` ordering is required **between the two readers** — their
partition is total, so either order yields the same result. One ordering edge
*is* required, though: the binary's `InputDisabled` maintainer must run
**before** the crate dispatcher within the same frame, or a focus / IME / picker
transition is observed one frame late. The crate therefore exposes a public
`OzmaTerminalInputSet` containing the dispatcher, and the host schedules its
maintainer `.before(OzmaTerminalInputSet)`.

## Component Specification

### `crates/ozma_terminal/src/clipboard.rs` (moved from `src/clipboard.rs`)

The `Clipboard` resource (an `arboard` wrapper) and the pure `build_paste_bytes`
helper move into the crate verbatim, with their existing tests. `Paste`'s
observer needs clipboard read access, and the clipboard is terminal
infrastructure, so it belongs here.

- `Clipboard` becomes `pub` (consumed by the crate's `PasteAction` observer and,
  externally, by the binary's tmux copy-mode code).
- `build_paste_bytes` becomes `pub` (not `pub(crate)`): besides the crate's
  `PasteAction` observer, the binary's tmux paste calls it at
  `src/tmux/input.rs:301`, so keeping it crate-private would break that path.
- The crate gains an `arboard` dependency.
- Headless behavior is unchanged: `arboard` init failure leaves the inner
  `Option` `None`, and reads/writes no-op.

**Consumer fan-out:** every binary reference to `crate::clipboard::*` re-points
to `ozma_terminal::*`. Consumers (from grep): `src/ozma_input.rs`,
`src/tmux/mouse.rs`, `src/tmux/input.rs` (uses `build_paste_bytes`),
`src/tmux/copy_mode.rs`, `src/ui/copy_mode.rs`, and the `mod clipboard`
declaration in `src/main.rs`. This is mechanical. `src/clipboard.rs` is deleted.

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

/// Host-supplied input policy: the chord that triggers the built-in Paste
/// action, plus the chords the dispatcher must skip (the host handles those).
/// Both are populated together from one source, so the "paste ∉ reserved"
/// invariant lives in one place rather than split across two resources.
///
/// `Default` is `Cmd+V` paste + empty reserved, so a spawn-and-go consumer
/// that never overrides it still gets working paste and forwards everything
/// else.
#[derive(Resource)]
pub struct TerminalInputBindings {
    pub paste: ReservedChord,
    pub reserved: Vec<ReservedChord>,
}

/// Registers the default terminal keyboard dispatcher.
pub struct OzmaInputPlugin;
```

`OzmaInputPlugin` registers `TerminalInputBindings` (the `Cmd+V`-paste default)
and the dispatcher system. The dispatcher:

- Query: `Query<Entity, (With<OzmaTerminal>, Without<InputDisabled>)>` — only
  active terminals. Registered in the `OzmaTerminalInputSet` (see below) with
  `run_if(on_message::<KeyboardInput>)`; an empty query (no active terminal)
  makes the body a no-op, so no extra guard is needed.
- Computes the current modifier set from `Res<ButtonInput<KeyCode>>` via a
  crate-local helper returning a **crate-local** modifier type (four bools). It
  cannot reuse the binary's `current_modifiers`, which returns
  `ozmux_configs::shortcuts::Modifiers` — the crate must not depend on
  `ozmux_configs`.
- For each `ButtonState::Pressed` event, reading `Res<TerminalInputBindings>`:
  1. If `(key_code, mods)` is in `bindings.reserved` → **skip** (host handles it).
  2. Else if it matches `bindings.paste` → `trigger(PasteAction { entity })`.
  3. Else if `mods.meta` (an unhandled Cmd/Super chord) → **drop**. This
     preserves today's `if mods.meta { continue }` swallow (`src/ozma_input.rs:141`)
     so that `Cmd+<key>` never reaches the PTY as text. Without this step the
     three-way partition would forward an unreserved `Cmd+J` to the PTY — a
     behavior change.
  4. Else map the logical key via `bevy_key_to_terminal_key`; on `Some(key)` →
     `trigger(TerminalKeyInput { entity, key, modifiers })`; on `None` → drop.

`bevy_key_to_terminal_key` moves here from `src/ozma_input.rs` unchanged (with
its tests).

**Why the Paste chord lives in `TerminalInputBindings`, not hardcoded.** ozmux
lets users rebind paste, so step 2 matches against `bindings.paste` rather than
a literal `Cmd+V`. Folding it into the same resource as `reserved` (instead of a
separate `PasteChord`) keeps the two values — always written together from one
resolved-shortcut source — in one place, and makes the "paste must not also
appear in `reserved`" invariant local and testable. The crate stays ignorant of
`ozmux_configs`; the host supplies both as plain `ReservedChord` data.

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
`resize_to_window` system: `reset_last_size` fires on `On<Add, OzmaTerminal>`
(setting `OzmaLastSize` to `None`), so `resize_to_window` runs on the first
frame metrics exist. Caveat: no scheduling edge forces that resize to run before
the engine's bootstrap snapshot — `check_deadline_flush`
(`crates/ozma_tty_engine/src/plugin.rs`) can emit one frame at `80×24` before
the resize lands. This is cosmetic (a single frame, before any shell output). If
it proves visible, add an ordering edge so `resize_to_window` precedes the engine
emit, or resize synchronously inside the `On<Add>` observer from window +
metrics. A consumer wanting an exact initial size can resize after spawn; no
`cols/rows` field is added (YAGNI).

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
`Assets`) is allocated in the observer, not by the consumer. The `On<Add>`
observer runs when `OzmaTerminal` is inserted; the commands it queues are
applied during command-flush (Bevy 0.18 applies observer-issued commands at the
end of the triggering flush, not synchronously at the `commands.spawn(...)` call
site). The render components are therefore present before the first `Update`.

**Precondition:** the observer takes `ResMut<Assets<TerminalUiMaterial>>`, so the
consumer must add `TerminalRendererPlugin` (which registers that asset) before
any `OzmaTerminal` is spawned, or the observer panics. In ozmux this holds —
spawning happens on `OnEnter(AppMode::Ozma)`, long after plugin build.

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
   The **Paste arm is removed** (the crate owns it). It is **not** gated by the
   terminal's `InputDisabled` (it has its own guards), so it must reproduce the
   current guard structure exactly:
   - skip entirely when the picker is open or the window is unfocused;
   - **while a webview holds focus, process only `ReleaseInlineFocus`** and
     suppress `Quit` / `OpenPicker` / `DetachSession` — matching today's
     `focused_webview.is_some()` early branch (`src/ozma_input.rs:83-94`).

   Missing the webview guard is a regression: those actions would newly fire
   while a webview is focused. (The crate dispatcher is already off in that
   state because the maintainer sets `InputDisabled`.)

**`TerminalInputBindings` population.** Add
`ResolvedShortcuts::input_bindings(&self) -> TerminalInputBindings` that maps the
resolved Paste chord into `paste` and every resolved shortcut whose action is
**not Paste** into `reserved` (as crate `ReservedChord`s). A `Startup` system
(ordered after `build_resolved_shortcuts`) writes the whole resource. Building
both fields in one function keeps the "paste ∉ reserved" invariant in one place.

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
  populate_bindings → ozma_terminal::TerminalInputBindings {
      paste:    Cmd+V
      reserved: { Cmd+Q, Cmd+Shift+P, Ctrl+Shift+Escape, Ctrl+Shift+D }  (Paste excluded)
    }

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
| `Ctrl+Shift+Escape` while webview focused | binary releases focus | binary releases focus (app-shortcut handler runs despite terminal `InputDisabled`) |
| `Cmd+Q` / `Cmd+Shift+P` while webview focused | suppressed (only ReleaseInlineFocus fires) | suppressed — app-shortcut handler's webview guard skips them |
| unreserved `Cmd+J` | dropped (`if mods.meta` swallow) | dropped (dispatcher step 3: unhandled meta) |

## Error Handling

- `OzmaTerminalBundle::spawn` returns `anyhow::Result`; PTY spawn failure
  propagates to the consumer, which decides the response (the binary logs and
  sends `AppExit::Success`, as today).
- `Clipboard` unavailable (headless) → `PasteAction` no-ops.
- Default `TerminalInputBindings` (empty `reserved`, `Cmd+V` paste) → the crate
  forwards everything and pastes on `Cmd+V`; a minimal consumer needs zero input
  code.
- A logical key with no `TerminalKey` mapping (function keys, etc.) is dropped,
  as today.

## Testing

Crate (`crates/ozma_terminal`), all with `MinimalPlugins` + injected
`KeyboardInput`:

- `input.rs`:
  - reserved chord → no `TerminalKeyInput` / `PasteAction` fired.
  - paste chord → `PasteAction` fired for the entity.
  - plain/forwardable key → `TerminalKeyInput` fired with correct `TerminalKey`.
  - unhandled meta chord (e.g. `Cmd+J`, not reserved, not paste) → **nothing
    fired** (dispatcher step 3 drop — guards the parity regression).
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

- `ResolvedShortcuts::input_bindings` is a pure function: default bindings
  produce `paste = Cmd+V` and `reserved = {Quit, OpenPicker, ReleaseInlineFocus,
  DetachSession}`, with Paste excluded from `reserved`.
- `InputDisabled` maintainer toggles correctly across the guard permutations and
  obeys conditional change detection (no rewrite when state is unchanged).
- App-shortcut handler: while a webview is focused, `Quit` / `OpenPicker` /
  `DetachSession` are suppressed and only `ReleaseInlineFocus` fires (the
  webview-guard parity check).

## Migration Steps

1. Move `src/clipboard.rs` → `crates/ozma_terminal/src/clipboard.rs`; add
   `arboard` to the crate; make `Clipboard` **and** `build_paste_bytes` `pub`;
   re-point binary imports (`ozma_input`, `tmux/mouse`, `tmux/input`,
   `tmux/copy_mode`, `ui/copy_mode`, and drop `mod clipboard` from `main.rs`);
   delete `src/clipboard.rs`.
2. Add `crates/ozma_terminal/src/action.rs` with `PasteAction` + `on_paste` +
   `OzmaActionPlugin`.
3. Add `crates/ozma_terminal/src/input.rs` with `InputDisabled`,
   `ReservedChord`, `TerminalInputBindings`, the public `OzmaTerminalInputSet`,
   the dispatcher (incl. the meta-drop step), and the moved
   `bevy_key_to_terminal_key`.
4. Extend `spawn.rs`: `OzmaSpawnOptions`, `OzmaTerminalBundle::spawn`, and the
   `On<Add, OzmaTerminal>` render-injection observer.
5. Wire `OzmaInputPlugin` + `OzmaActionPlugin` + the render observer into
   `OzmaTerminalPlugin::build`.
6. Binary: split `ozma_input.rs` into the `InputDisabled` maintainer
   (scheduled `.before(OzmaTerminalInputSet)`) + the app-shortcut handler (drop
   raw-key forwarding and the Paste arm; keep the picker/unfocused/webview
   guards); add `ResolvedShortcuts::input_bindings` + the population `Startup`
   system; collapse `src/ozma.rs` `spawn_terminal` to the bundle call.
7. `cargo test`, `cargo clippy --workspace`, `cargo fmt`. Manual smoke:
   type/paste/quit/open-picker/detach in Ozma mode; confirm Ozmux mode
   (tmux) input is unaffected.

## Decisions & Rationale

- **A host-supplied reserved set over modifier-partition or leftover-event.**
  Default bindings include non-`meta` app shortcuts (`Ctrl+Shift+D` detach,
  `Ctrl+Shift+Escape` release-inline-focus), so a "meta = app, non-meta =
  terminal" split leaks those to the PTY. A leftover-event model would still
  need a reserved set to classify them. `TerminalInputBindings.reserved` is the
  minimal mechanism that handles arbitrary host bindings, keeps the proven
  `match_gui_action` flow in a normal system, and exposes one small, testable
  seam. (Paste is carried in the same resource so the "paste ∉ reserved"
  invariant is local.)
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
