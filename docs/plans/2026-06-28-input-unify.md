# Input Unification — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move all input *gathering, deciding, and event-triggering* out of the library crates (`ozma_terminal`, and the `sync_focused_webview` system in `ozma_webview`) into the host binary under `src/input/`, leaving the libraries to declare only event types + apply observers.

**Architecture:** Behavior-preserving relocation. The library's terminal input dispatch (`dispatch_input`, `dispatch_mouse_buttons`, `dispatch_mouse_wheel`) moves to mode-neutral host modules `src/input/{keyboard,mouse}.rs`, marker-gated exactly as today (`KeyboardFocused + !KeyboardDisabled` / `!MouseDisabled`). The library keeps the apply observers (`on_paste`, `on_terminal_mouse_effects`) and the events they observe; `ozma_tty_engine`'s `TerminalKeyInput` + `on_terminal_key_input` are untouched. The mouse dispatch stays mode-neutral because normal tmux panes rely on it (`ozma_terminal::dispatch_mouse_wheel` is the `CededToOzma` owner; mouse reports route out via `TerminalForwardInput` → tmux `send-keys`).

**Tech Stack:** Rust (edition 2024, toolchain 1.95), Bevy 0.18 ECS. Crates touched: `ozma_terminal` (loses dispatch, keeps observers), `ozma_webview` (loses `sync_focused_webview`), binary `src/` (gains `src/input/{keyboard,mouse,gesture,bindings,focus}.rs`). `ozma_tty_engine` is untouched.

## Global Constraints

- Edition 2024, toolchain pinned to 1.95. Workspace builds with `cargo build`.
- **No behavior change.** Same routing, gating, ordering, shortcuts. The acceptance gate for every task is: `cargo build` green + `cargo test` green (the relocated unit tests are the safety net) + `just fix-lint` clean.
- **Comments:** only `// TODO:` / `// NOTE:` / `// SAFETY:`; `// NOTE:` for critical caveats only. English only. No block comments, no commented-out code, no narrative comments.
- **Doc comments:** every externally-`pub` item gets a `///` one-line summary. New `pub` items (`TerminalMouseEffects::new`, the `pub` on `MouseEffect`/`TerminalMouseEffects`) need docs.
- **Visibility:** narrowest that compiles. Items relocated into the binary become `pub(crate)` (or private if single-module). Component/Resource types moved to the host are `pub(crate)`. Struct fields stay private; expose constructors.
- **No `mod.rs`.** A new `src/input/tmux/` subtree needs `src/input/tmux.rs` as its declaring file. Imports at top of file, single contiguous block, no inline fully-qualified paths.
- **Bevy idioms:** `Plugin::build` is one method chain; register systems/observers in the plugin defined in the same file; whole-system change guards use `run_if`; no manual `set_changed()`. `Query` params use descriptive nouns (no `_q`). Mutable params before immutable.
- **Marker invariant (critical):** the moved mouse dispatch MUST keep `Without<MouseDisabled>` gating and run in BOTH modes (NOT `in_state(AppMode::Default)`). The moved keyboard dispatch keeps `KeyboardFocused + Without<KeyboardDisabled>` gating (naturally Default-only — tmux marks every pane `KeyboardDisabled`). Changing either gate silently breaks tmux normal-pane mouse / keyboard routing.

Reference spec: `docs/specs/2026-06-28-input-unify-design.md`.

### Two cross-cutting rules (read before starting)

**Temporary duplication.** Pure functions (`current_terminal_modifiers`, `chord_matches`, `bevy_key_to_terminal_key`, `accumulate_notches`, geometry helpers) and dispatch-local plain types (`ClickTracker`, `WheelAccumulator`, drag structs) may exist in BOTH the library and the host during the migration — that is intentional and compiles. They are deduped when the library dispatch that used the original is deleted (Tasks 4–5). **Components and Resources cannot be duplicated** (two definitions = two distinct types): `MouseDisabled` / `OzmaMouseConfig` move *atomically* with the mouse dispatch (Task 4); `KeyboardFocused` / `KeyboardDisabled` / `TerminalInputBindings` move *atomically* with the keyboard dispatch (Task 5).

**Ordering anchors.** The host gates (`maintain_input_gates`, `maintain_tmux_input_gates`) order `.before(OzmaTerminalInputSet).before(OzmaTerminalMouseSet)` — the "gates before all terminal dispatch" invariant (gates write `MouseDisabled`/`KeyboardDisabled`, dispatch reads them). Keep both **library `SystemSet`s alive as ordering anchors** until Task 6. Relocated host dispatch joins those sets (`.in_set(OzmaTerminalMouseSet)` for mouse, `.in_set(OzmaTerminalInputSet)` for keyboard) so the gates' `.before(...)` keeps working with no change to the gates until Task 6 re-anchors everything to the host `InputPhase`.

---

### Task 1: Decouple `ozma_webview` from `KeyboardFocused` (spec Phase 1)

Move `sync_focused_webview` from the library into the host so `ozma_webview` stops reading `KeyboardFocused` / `OzmaTerminalInputSet`.

**Files:**
- Create: `src/input/focus.rs` (host home for the system; the marker types join it in Tasks 4–5)
- Modify: `src/input.rs` (declare `pub(crate) mod focus;`)
- Modify: `src/main.rs` (register `FocusSyncPlugin`)
- Modify: `crates/ozma_webview/src/webview/render.rs` (delete the system + its `#[cfg(test)]` cases; drop `OzmaTerminalInputSet`/`KeyboardFocused` from the `use` on line 11; drop the `.add_systems` on line 90)
- Modify: `crates/ozma_webview/src/control_plane.rs:1910-1976` (the `sync_preserves_app_declared_focus_from_control_plane` test imports `crate::webview::render::sync_focused_webview` — move it to `src/input/focus.rs`)

**Interfaces:**
- Consumes: `bevy_cef::prelude::{FocusedWebview, WebviewSource}` — `WebviewSource` is a **bevy_cef** type (host already imports it at `src/input/hyperlink.rs:23`), NOT an `ozma_webview` export; `ozma_webview::{Webview, NonInteractive}`; `ozma_terminal::{OzmaTerminal, KeyboardFocused}`; `crate::input::InputPhase`.
- Produces: `crate::input::focus::FocusSyncPlugin`, `pub(crate) fn sync_focused_webview(...)`.

- [ ] **Step 1: Create `src/input/focus.rs`**

Paste the verbatim body of `sync_focused_webview` (`crates/ozma_webview/src/webview/render.rs:113-139`, keep both `// NOTE:` comments) into a new module, as `pub(crate) fn`, with a plugin:

```rust
//! Host-owned webview focus sync: keeps bevy_cef's `FocusedWebview` in step with
//! the `KeyboardFocused` terminal surface. Moved out of `ozma_webview` so the
//! library no longer reads `KeyboardFocused`. The marker components
//! (`KeyboardFocused`/`KeyboardDisabled`/`MouseDisabled`) move into this module
//! in later tasks.

use crate::input::InputPhase;
use bevy::prelude::*;
use bevy_cef::prelude::{FocusedWebview, WebviewSource};
use ozma_terminal::{KeyboardFocused, OzmaTerminal};
use ozma_webview::{NonInteractive, Webview};

/// Registers the webview focus-sync system.
pub(crate) struct FocusSyncPlugin;

impl Plugin for FocusSyncPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, sync_focused_webview.after(InputPhase::FocusedKey));
    }
}

// pub(crate) fn sync_focused_webview(...) { /* verbatim body from render.rs:113-139 */ }
```

`.after(InputPhase::FocusedKey)` preserves the original "focus is resolved this frame" precondition (`KeyboardFocused` writers run in/before `InputPhase::Dispatch`; `FocusedKey` is the last input sub-phase).

- [ ] **Step 2: Move the tests**

Move the `sync_focused_webview` `#[cfg(test)]` cases from `render.rs` (~lines 320, 376, 403, 435) and `sync_preserves_app_declared_focus_from_control_plane` (`control_plane.rs:1910`) into a `#[cfg(test)] mod tests` in `src/input/focus.rs`. They compile in the binary (it depends on `ozma_terminal` + `ozma_webview` + `bevy_cef`).

- [ ] **Step 3: Delete from `ozma_webview`**

In `render.rs`: delete `sync_focused_webview` + its moved test cases; remove the `.add_systems(Update, sync_focused_webview.after(OzmaTerminalInputSet))` (line 90); change `use ozma_terminal::{KeyboardFocused, OzmaTerminal, OzmaTerminalInputSet};` (line 11) → `use ozma_terminal::OzmaTerminal;`. Delete the moved test in `control_plane.rs`.

- [ ] **Step 4: Wire the host plugin**

`src/input.rs`: add `pub(crate) mod focus;`. `src/main.rs`: add `crate::input::focus::FocusSyncPlugin` to the input plugin group (beside `HyperlinkInputPlugin`).

- [ ] **Step 5: Build + test**

Run: `cargo build` → green.
Run: `cargo test -p ozma_webview && cargo test --bin ozmux focus` → green.

- [ ] **Step 6: Lint + commit**

```bash
just fix-lint
git add -A && git commit -m "refactor(input): move sync_focused_webview to host, decouple ozma_webview from KeyboardFocused"
```

---

### Task 2: Publish the mouse apply API (spec Phase 2)

Make `TerminalMouseEffects` constructible from the host without exposing fields.

**Files:**
- Modify: `crates/ozma_terminal/src/mouse.rs` (`MouseEffect:242`, `TerminalMouseEffects:280`)

**Interfaces:**
- Produces (used by Task 4): `pub enum MouseEffect`, `pub struct TerminalMouseEffects`, `pub fn TerminalMouseEffects::new(entity: Entity, effects: Vec<MouseEffect>) -> Self`.

- [ ] **Step 1: Widen visibility + add constructor**

`pub(crate) enum MouseEffect` → `pub enum MouseEffect`; `pub(crate) struct TerminalMouseEffects` → `pub struct TerminalMouseEffects` with **private** fields (drop any `pub(crate)` on `entity`/`effects`). Add `///` docs to both. Add:

```rust
impl TerminalMouseEffects {
    /// Builds a mouse-effects event targeting `entity` with `effects` applied in order.
    pub fn new(entity: Entity, effects: Vec<MouseEffect>) -> Self {
        Self { entity, effects }
    }
}
```

Internal struct-literal sites in `mouse.rs` (`:564,:601,:713`, test builders) still compile (same module); they move in Task 4.

- [ ] **Step 2: Build + test**

Run: `cargo test -p ozma_terminal` → green (additive).

- [ ] **Step 3: Lint + commit**

```bash
just fix-lint
git add -A && git commit -m "refactor(ozma_terminal): make TerminalMouseEffects/MouseEffect pub with a constructor"
```

---

### Task 3: Extract the shared host primitives (spec Phase 0)

Create the host modules the relocated mouse dispatch (Task 4) needs before the keyboard dispatch moves. **Copies** — the library keeps its originals until Tasks 4–5.

**Files:**
- Create: `src/input/keyboard.rs` (the modifier reader; gains the keyboard dispatch in Task 5)
- Create: `src/input/gesture.rs` (click / wheel / drag state)
- Modify: `src/input.rs` (declare both)

**Interfaces:**
- Produces (used by Task 4):
  - `crate::input::keyboard::current_terminal_modifiers(keys: &ButtonInput<KeyCode>) -> TerminalModifiers`
  - `crate::input::gesture::{ClickTracker, WheelAccumulator, accumulate_notches, wheel_delta_cells, OzmaMouseGesture, DragGesture, DragPhase, HeldPointer}` (names preserved from the library)

- [ ] **Step 1: `src/input/keyboard.rs`**

Copy `current_terminal_modifiers` verbatim from `crates/ozma_terminal/src/input.rs:147-154` (returns `ozma_tty_engine::TerminalModifiers`):

```rust
//! Host keyboard primitives shared by the terminal keyboard dispatch and the
//! mouse dispatch (modifier reading). Gains the relocated `dispatch_input` in Task 5.

use bevy::prelude::*;
use ozma_tty_engine::TerminalModifiers;

/// Returns the terminal modifier state from the `ButtonInput<KeyCode>` resource.
pub(crate) fn current_terminal_modifiers(keys: &ButtonInput<KeyCode>) -> TerminalModifiers {
    // verbatim body from ozma_terminal/src/input.rs:148-153
}
```

- [ ] **Step 2: `src/input/gesture.rs`**

Copy verbatim from `crates/ozma_terminal/src/mouse.rs`: `ClickTracker` (`:143-161`, incl. `register`), `WheelAccumulator` (`:354-370`), `accumulate_notches` (`:386-401`), `wheel_delta_cells` (`:374-379`), `OzmaMouseGesture` (`:127-139`), `DragGesture`/`DragPhase` (`:92-111`), `HeldPointer` (`:118-123`). Bring their `#[cfg(test)]` unit tests (`accumulate_notches`, `ClickTracker::register`). Types `pub(crate)`; keep fields private, exposing only what Task 4's `mouse.rs` needs via `pub(crate)` accessors.

- [ ] **Step 3: Declare modules**

`src/input.rs`: add `pub(crate) mod keyboard;` and `pub(crate) mod gesture;`.

- [ ] **Step 4: Build + test**

Run: `cargo build && cargo test --bin ozmux` → green. If clippy flags `dead_code` on a not-yet-wired item, add `#[expect(dead_code, reason = "wired in Task 4")]` (remove it in Task 4).

- [ ] **Step 5: Lint + commit**

```bash
just fix-lint
git add -A && git commit -m "refactor(input): extract shared keyboard/gesture primitives into src/input/"
```

---

### Task 4: Move the shared mouse dispatch to `src/input/mouse.rs` (spec Phase 3)

The atomic core: relocate the mouse dispatch + deciders + geometry + `MouseDisabled` + `OzmaMouseConfig`, delete the library mouse dispatch, repoint tmux's `ClickTracker`. Mode-neutral, marker-gated.

**Files:**
- Create: `src/input/mouse.rs` (dispatch systems, deciders, engine-`Side` geometry, `MouseInputPlugin`)
- Create: `src/input/bindings.rs` (`OzmaMouseConfig`, `FineModifier`)
- Modify: `src/input/focus.rs` (add the `MouseDisabled` marker)
- Modify: `crates/ozma_terminal/src/mouse.rs` (delete dispatch + deciders + geometry + gesture state + config + `MouseDisabled`; KEEP `on_terminal_mouse_effects`, `apply_effect*`, `MouseEffect`, `TerminalMouseEffects`, `TerminalForwardInput`; KEEP `OzmaTerminalMouseSet` as the ordering anchor)
- Modify: `crates/ozma_terminal/src/lib.rs` (`OzmaMousePlugin` keeps only the observer; drop re-exports of moved items, keep `TerminalForwardInput` + `OzmaTerminalMouseSet`)
- Modify importers of `MouseDisabled`/`OzmaMouseConfig`/`FineModifier`: `src/input/shortcuts.rs:8`, `src/input/hyperlink.rs:24`, `src/mode/default/input.rs:21-24`, `src/mode/tmux/gate.rs:17`, `src/ui/copy_mode.rs:14`
- Modify: `src/mode/tmux/mouse/decide.rs:76` (delete tmux's `ClickTracker`; import `crate::input::gesture::ClickTracker`) and `src/mode/tmux/mouse.rs:30` (`use`)
- Modify: `src/main.rs` (register `MouseInputPlugin`)

**Interfaces:**
- Consumes: `crate::input::keyboard::current_terminal_modifiers`, `crate::input::gesture::*` (Task 3), `crate::webview_pointer::topmost_surface_at` (replaces `topmost_terminal_at`), `TerminalMouseEffects::new` (Task 2), engine routers `ButtonAction::route` / `WheelAction::{route, route_horizontal}`, `ozma_terminal::OzmaTerminalMouseSet` (ordering anchor).
- Produces: `crate::input::mouse::MouseInputPlugin`; `pub(crate)` `OzmaMouseConfig`/`FineModifier` (`bindings.rs`); `pub(crate) struct MouseDisabled` (`focus.rs`).

- [ ] **Step 1: `src/input/bindings.rs` with the mouse config**

Move `OzmaMouseConfig` (`crates/ozma_terminal/src/mouse.rs:47-75`, incl. `Default`) and `FineModifier` (`:32-42`) verbatim → `src/input/bindings.rs` as `pub(crate)`. Declare `pub(crate) mod bindings;` in `src/input.rs`.

- [ ] **Step 2: `MouseDisabled` → `src/input/focus.rs`**

Move `pub struct MouseDisabled;` (`mouse.rs:88`) → `src/input/focus.rs` as `pub(crate) struct MouseDisabled;` with its `///` doc.

- [ ] **Step 3: `src/input/mouse.rs` with the dispatch**

Move verbatim from `crates/ozma_terminal/src/mouse.rs` (fixing imports): `dispatch_mouse_buttons` (`:430`), `dispatch_mouse_wheel` (`:613`), `decide_button` (`:292`), `decide_wheel` (`:405`), `effects_from_wheel_action`, `resolve_button_event` (`:749`), `synthesize_drag` (`:802`), `update_selection` (`:1009`), `effective_drag_cursor`, `build_wheel_modifiers*`, `fine_held`, `map_button`, `protocol_mods` (`:228`), and the engine-`Side` geometry `cell_at_local`/`cell_at_cursor`/`to_viewport_point`/`CellContext` (`:165-225,:723-743`). Bring all their `#[cfg(test)]` tests. Then:
- Replace `current_terminal_modifiers` calls with `crate::input::keyboard::current_terminal_modifiers`.
- Replace `OzmaMouseGesture`/`ClickTracker`/`WheelAccumulator`/`accumulate_notches`/`wheel_delta_cells`/`DragGesture` refs with `crate::input::gesture::*`.
- Replace `topmost_terminal_at(...)` with `crate::webview_pointer::topmost_surface_at(...)` (identical signature — `src/webview_pointer.rs:207`). Do NOT relocate `topmost_terminal_at`.
- Build the event via `TerminalMouseEffects::new(target, decided)`, not a struct literal.
- Define `MouseInputPlugin` here (NOT `MousePlugin` — tmux already has a `MousePlugin`). Register `OzmaMouseConfig`/`OzmaMouseGesture`/`WheelAccumulator` resources + the two dispatch systems `.in_set(ozma_terminal::OzmaTerminalMouseSet)` (the surviving anchor), `.run_if(on_message::<MouseButtonInput>.or(on_message::<CursorMoved>).or(on_message::<MouseWheel>))`. **Keep `Without<MouseDisabled>` gating; do NOT add `in_state(AppMode::Default)`.**

Declare `pub(crate) mod mouse;` in `src/input.rs`.

- [ ] **Step 4: Delete the library mouse dispatch; keep the observer + anchor**

In `crates/ozma_terminal/src/mouse.rs`: delete everything moved in Steps 1–3. KEEP `on_terminal_mouse_effects`, `apply_effect`, `apply_effect_detached`, `MouseEffect`, `TerminalMouseEffects`, `TerminalForwardInput`, `OzmaTerminalMouseSet`, and their tests. In `OzmaMousePlugin::build`, drop the message registrations + the two dispatch systems; keep `.add_observer(on_terminal_mouse_effects)`. In `lib.rs`, drop `pub use mouse::{FineModifier, MouseDisabled, OzmaMouseConfig, ...}` for moved items; keep `TerminalForwardInput` and `OzmaTerminalMouseSet`.

- [ ] **Step 5: Repoint importers**

Update each moved-item import: `MouseDisabled` → `crate::input::focus::MouseDisabled`; `OzmaMouseConfig`/`FineModifier` → `crate::input::bindings::{...}`. Files: `src/input/shortcuts.rs`, `src/input/hyperlink.rs`, `src/mode/default/input.rs`, `src/mode/tmux/gate.rs`, `src/ui/copy_mode.rs`. In `src/mode/tmux/mouse/decide.rs`, delete the local `ClickTracker` (76-95) and import `crate::input::gesture::ClickTracker`; fix `src/mode/tmux/mouse.rs`'s `use` (line 30).

- [ ] **Step 6: Register the plugin**

`src/main.rs`: add `crate::input::mouse::MouseInputPlugin` to the input group. `OzmaTerminalPlugin` still adds the now-observer-only `OzmaMousePlugin` — leave it.

- [ ] **Step 7: Build + test (behavior-critical)**

Run: `cargo build` → green.
Run: `cargo test` → green. The moved `dispatch_mouse_*` tests, the `decide_*` tests, the tmux mouse tests (now on the shared `ClickTracker`), and `ozma_terminal`'s `on_terminal_mouse_effects` tests must all pass.

- [ ] **Step 8: Lint + commit**

```bash
just fix-lint
git add -A && git commit -m "refactor(input): move shared mouse dispatch to src/input/mouse.rs (mode-neutral), dedup ClickTracker"
```

---

### Task 5: Move the keyboard dispatch + focus markers to `src/input/keyboard.rs` (spec Phase 4)

Relocate `dispatch_input`, the keyboard primitives, `TerminalInputBindings`/`ReservedChord`, and the `KeyboardFocused`/`KeyboardDisabled` components; delete `ozma_terminal::input` (its `OzmaTerminalInputSet` is preserved by moving the definition to `lib.rs` as the surviving anchor).

**Files:**
- Modify: `src/input/keyboard.rs` (add `dispatch_input`, `bevy_key_to_terminal_key`, `chord_matches`, `KeyboardInputPlugin`)
- Modify: `src/input/bindings.rs` (add `TerminalInputBindings`, `ReservedChord`)
- Modify: `src/input/focus.rs` (add `KeyboardFocused`, `KeyboardDisabled`; the Task 1 import becomes local)
- Modify: `crates/ozma_terminal/src/lib.rs` (move the `OzmaTerminalInputSet` definition here from `input.rs`; drop `mod input;`, `OzmaInputPlugin`, the moved re-exports) — Delete: `crates/ozma_terminal/src/input.rs`
- Modify: ALL `KeyboardFocused` importers → `crate::input::focus::KeyboardFocused`: `src/window_title.rs:8`, `src/mode/default/copy_mode.rs:20`, `src/mode/default.rs:15`, `src/ui/ime_overlay.rs:32`, `src/mode/tmux/pane_focus.rs:12`, `src/mode/tmux/adopt.rs:20`, `src/input/ime.rs:24`, `src/mode/default/input.rs:22`, `src/input/focus.rs`. `KeyboardDisabled` importers: `src/ui/copy_mode.rs:14`, `src/mode/tmux/gate.rs:17`, `src/mode/default/input.rs:22`.

**Interfaces:**
- Consumes: `crate::input::bindings::{TerminalInputBindings, ReservedChord}`, `crate::input::focus::{KeyboardFocused, KeyboardDisabled}`, `crate::input::InputPhase`, `ozma_tty_engine::{TerminalKey, TerminalKeyInput, TerminalModifiers}`.
- Produces: `crate::input::keyboard::KeyboardInputPlugin`; `pub(crate)` `KeyboardFocused`/`KeyboardDisabled`/`TerminalInputBindings`/`ReservedChord`.

- [ ] **Step 1: Move bindings + markers**

Move `TerminalInputBindings` (`crates/ozma_terminal/src/input.rs:52-72`) and `ReservedChord` (`:31-42`) → `src/input/bindings.rs` (`pub(crate)`). Move `KeyboardFocused` (`:26`) and `KeyboardDisabled` (`:18`) → `src/input/focus.rs` (`pub(crate)`, with docs). In `focus.rs`, change Task 1's `use ozma_terminal::{KeyboardFocused, OzmaTerminal};` → `use ozma_terminal::OzmaTerminal;` (KeyboardFocused now local).

- [ ] **Step 2: Move the dispatch + preserve the set**

Move `dispatch_input` (`:96-145`), `bevy_key_to_terminal_key` (`:164-183`), `chord_matches` (`:156-162`) into `src/input/keyboard.rs`, repointing imports to `crate::input::{focus,bindings}`. Define `KeyboardInputPlugin` registering `TerminalInputBindings` + `dispatch_input` `.in_set(ozma_terminal::OzmaTerminalInputSet)` (the preserved anchor — keeps the gates ordered before it; Task 6 re-anchors it to `InputPhase::FocusedKey`), `.run_if(on_message::<KeyboardInput>)`, **keeping `KeyboardFocused + Without<KeyboardDisabled>` gating**. Bring the `dispatch_input` `#[cfg(test)]` cases. Move the `pub struct OzmaTerminalInputSet;` definition from `input.rs:78` to `crates/ozma_terminal/src/lib.rs` so it survives the file deletion (it stays the gates' anchor until Task 6).

- [ ] **Step 3: Delete `ozma_terminal::input`**

Delete `crates/ozma_terminal/src/input.rs`. In `lib.rs`: remove `mod input;`, the `OzmaInputPlugin` from `OzmaTerminalPlugin::build`, the `pub use input::{...}` re-exports; keep the `OzmaTerminalInputSet` definition (moved in Step 2) `pub`.

- [ ] **Step 4: Repoint importers**

Mechanically change every `use ozma_terminal::{... KeyboardFocused / KeyboardDisabled ...}` → `crate::input::focus::{...}` (splitting out items that stay in `ozma_terminal`, e.g. `OzmaTerminal`). Touch the files listed above.

- [ ] **Step 5: Register the plugin**

`src/main.rs`: add `crate::input::keyboard::KeyboardInputPlugin` to the input group.

- [ ] **Step 6: Build + test**

Run: `cargo build && cargo test` → green.
Run: `grep -rn "ozma_terminal::.*KeyboardFocused\|ozma_terminal::.*KeyboardDisabled" src crates` → no matches.

- [ ] **Step 7: Lint + commit**

```bash
just fix-lint
git add -A && git commit -m "refactor(input): move keyboard dispatch + focus/gate markers to src/input/, delete ozma_terminal::input"
```

---

### Task 6: Delete the library input system sets; re-anchor on `InputPhase` (spec Phase 5)

Remove `OzmaTerminalInputSet` / `OzmaTerminalMouseSet`; host dispatch + gates order via `InputPhase`.

**Files:**
- Modify: `crates/ozma_terminal/src/{lib.rs, mouse.rs}` (delete the two `SystemSet` definitions + re-exports)
- Modify: `src/input/mouse.rs` (dispatch `.in_set(OzmaTerminalMouseSet)` → `.in_set(InputPhase::Dispatch)`)
- Modify: `src/input/keyboard.rs` (dispatch `.in_set(OzmaTerminalInputSet)` → `.in_set(InputPhase::FocusedKey)`)
- Modify: `src/mode/default/input.rs:40-41`, `src/mode/tmux/gate.rs:31-32` (gates `.before(OzmaTerminalInputSet).before(OzmaTerminalMouseSet)` → `.before(InputPhase::Hover)`)

**Interfaces:**
- Produces: nothing new. Gates run `.before(InputPhase::Hover)`; mouse dispatch in `InputPhase::Dispatch`; keyboard dispatch in `InputPhase::FocusedKey` (Hover → Dispatch → FocusedKey, so gates precede both).

- [ ] **Step 1: Re-anchor the host dispatch**

In `src/input/mouse.rs`, change the dispatch systems from `.in_set(ozma_terminal::OzmaTerminalMouseSet)` to `.in_set(InputPhase::Dispatch)`; drop the import. In `src/input/keyboard.rs`, change `dispatch_input` from `.in_set(ozma_terminal::OzmaTerminalInputSet)` to `.in_set(InputPhase::FocusedKey)`; drop the import.

- [ ] **Step 2: Re-anchor the gates**

In `src/mode/default/input.rs` and `src/mode/tmux/gate.rs`, replace `.before(OzmaTerminalInputSet).before(OzmaTerminalMouseSet)` with `.before(InputPhase::Hover)` (the first input sub-phase — `src/input.rs:19`). Drop the `ozma_terminal::{OzmaTerminalInputSet, OzmaTerminalMouseSet}` imports.

- [ ] **Step 3: Delete the sets**

Remove `OzmaTerminalMouseSet` (`mouse.rs:80`) and `OzmaTerminalInputSet` (moved to `lib.rs` in Task 5) and any `pub use` re-exports. Confirm `OzmaTerminalPlugin::build` now adds only `ExitPlugin`, `LayoutPlugin`, `OzmaActionPlugin`, the observer-only `OzmaMousePlugin`, `on_add_inject_render`.

- [ ] **Step 4: Build + test**

Run: `cargo build && cargo test` → green. Watch for Bevy ambiguous-ordering warnings; if any, add explicit `.before/.after` against the relevant `InputPhase`.
Run: `grep -rn "OzmaTerminalInputSet\|OzmaTerminalMouseSet" src crates` → no matches.

- [ ] **Step 5: Lint + commit**

```bash
just fix-lint
git add -A && git commit -m "refactor(input): delete library input SystemSets, re-anchor host ordering on InputPhase"
```

---

### Task 7: Final relocation into `src/input/` (spec Phase 6)

Pure module move: `src/mode/default/input.rs` → `src/input/default_mode.rs`; the tmux dispatch files → `src/input/tmux/`.

**Files:**
- Move: `src/mode/default/input.rs` → `src/input/default_mode.rs`
- Move: `src/mode/tmux/{input.rs, mouse.rs, mouse/*, forward.rs, gate.rs, pane_hit.rs, window_bar_input.rs}` → `src/input/tmux/` (create `src/input/tmux.rs` as the declaring file)
- Modify: `src/input.rs`, `src/mode/default.rs`, `src/mode/tmux.rs` (`mod` declarations), every `use crate::mode::{default,tmux}::...` referencing the moved items, and `src/main.rs` (plugin import paths)

**Interfaces:**
- Produces: same plugins, new paths (`crate::input::default_mode::DefaultHostInputPlugin`, `crate::input::tmux::{InputPlugin, MousePlugin, ForwardPlugin, GatePlugin}`). No symbol renames.

- [ ] **Step 1: Move the default-host input**

`git mv src/mode/default/input.rs src/input/default_mode.rs`. `src/mode/default.rs`: remove `mod input;`. `src/input.rs`: add `pub(crate) mod default_mode;`. Fix references (`crate::mode::default::input::*` → `crate::input::default_mode::*`) and the now-sibling `crate::input::*` imports inside the moved file.

- [ ] **Step 2: Move the tmux dispatch files**

Create `src/input/tmux.rs` declaring the moved submodules (`pub(crate) mod input; mod mouse; mod forward; mod gate; mod pane_hit; mod window_bar_input;`). `git mv` `input.rs`, `mouse.rs`, `mouse/`, `forward.rs`, `gate.rs`, `pane_hit.rs`, `window_bar_input.rs` from `src/mode/tmux/` → `src/input/tmux/`. Leave the state/UI files (`copy_mode.rs`, `confirm_prompt.rs`, `rename_prompt.rs`, `pane_focus.rs`, `render.rs`, `window_bar.rs`, `adopt.rs`, `locale.rs`, `mode_ui.rs`, `webview_tokens.rs`, `paint_rescue.rs`) in `src/mode/tmux/`. Update `src/mode/tmux.rs` (drop the moved `mod`s; the `OzmuxTmuxPlugin` aggregator now composes `crate::input::tmux::{InputPlugin, MousePlugin, ForwardPlugin, GatePlugin}`). `src/input.rs`: add `pub(crate) mod tmux;`. Fix every cross-reference (`super::pane_hit` → `crate::input::tmux::pane_hit`, `super::gate` → `crate::input::tmux::gate`, references from the state/UI files that stayed behind, etc.).

- [ ] **Step 3: Build + test**

Run: `cargo build && cargo test` → green. Failures here are missing/renamed `mod` paths — fix and re-run.

- [ ] **Step 4: Lint + commit**

```bash
just fix-lint
git add -A && git commit -m "refactor(input): relocate default/tmux input dispatch under src/input/"
```

---

## Final verification

- [ ] `cargo build && cargo test` (full workspace) green.
- [ ] `just fix-lint` clean (clippy + fmt + biome).
- [ ] `grep -rn "dispatch_input\|dispatch_mouse_buttons\|dispatch_mouse_wheel" crates/` → no matches (library has no dispatch).
- [ ] `grep -rn "ozma_terminal::.*\(KeyboardFocused\|KeyboardDisabled\|MouseDisabled\|OzmaMouseConfig\|OzmaTerminalInputSet\|OzmaTerminalMouseSet\)" src crates` → no matches.
- [ ] `grep -rn "sync_focused_webview" crates/ozma_webview` → no matches.
- [ ] Manual smoke (if a display is available): `cargo run` — type in the Default shell, drag-select + wheel-scroll; adopt a `tmux -CC` session and confirm normal-pane mouse reporting + wheel still reach tmux, copy-mode selection works, and webview focus still follows the active pane.

## Note on the spec's geometry framing

The spec's layout says `src/surface_geom.rs` "absorbs `cell_at_cursor` / `to_viewport_point`". This plan instead **co-locates** the engine-`Side` geometry (`cell_at_local`/`cell_at_cursor`/`to_viewport_point`/`CellContext`) inside `src/input/mouse.rs` with its only caller, because `surface_geom`'s existing `cell_at_local` returns the host `surface_geom::Side` (a different enum) and merging the two `Side` types is out of scope for a no-behavior-change refactor (per the spec's own NOTE). `topmost_terminal_at` IS deduped — replaced by the existing `topmost_surface_at`.
