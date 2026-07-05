# Unify input handling under `src/*`: the library declares only apply observers

## Problem

Input handling is split across three layers with no single owner of the
gather → decide → trigger pipeline:

- **`crates/orzma_terminal`** owns BOTH gather/decide AND apply for **Default**
  mode: `dispatch_input` (`src/input.rs:96`), `dispatch_mouse_buttons`
  (`src/mouse.rs:430`), `dispatch_mouse_wheel` (`src/mouse.rs:613`) read raw
  Bevy input, run the pure deciders `decide_button` (`src/mouse.rs:292`) /
  `decide_wheel` (`src/mouse.rs:405`), and `commands.trigger(...)` the apply
  events. The library also owns the input policy types (`TerminalInputBindings`
  `src/input.rs:52`, `OrzmaMouseConfig` `src/mouse.rs:47`, `FineModifier`
  `src/mouse.rs:32`) and the focus/gate marker components (`KeyboardFocused`,
  `KeyboardDisabled`, `MouseDisabled`).
- **`src/input/`** holds cross-mode input infrastructure (`hyperlink.rs`,
  `ime.rs`, `option_as_alt.rs`, `shortcuts.rs`) plus the `InputPhase` ordering
  (`src/input.rs:19`).
- **`src/mode/{default,tmux}/`** holds host dispatch. `src/mode/tmux/` *fully
  re-implements* its own keyboard (`forward_keys_to_tmux`), mouse
  (`tmux_gesture` `src/mode/tmux/mouse.rs:187`), and wheel dispatch, and does
  NOT use the library's dispatch — tmux panes are `OrzmaTerminal` entities that
  the host marks `KeyboardDisabled` / `MouseDisabled` (`src/mode/tmux/gate.rs`)
  to suppress the library systems.

The result is **two parallel dispatch implementations** (library = Default,
host = Tmux) gated by `AppMode` + marker components, with duplicated low-level
logic. `ClickTracker` is defined twice with an identical signature
(`crates/orzma_terminal/src/mouse.rs:143` and `src/mode/tmux/mouse/decide.rs:76`);
`cell_at_local` exists twice (`crates/orzma_terminal/src/mouse.rs:165` and
`src/surface_geom.rs:31`). The decision of "what does this key / click / wheel
mean" is scattered between the library and the host.

## Goal

**The library declares only event types and their apply observers. All input
gathering, deciding, and event-triggering moves into `src/*`, organized by
function under `src/input/`.**

- `orzma_terminal` (and `orzma_tty_engine`) keep the apply observers + the event
  vocabulary they observe; they read no raw input and call no `commands.trigger`.
- `src/*` owns gather → decide → trigger for both Default and Tmux.
- Low-level primitives (multi-click tracking, cell geometry, wheel
  accumulation, modifier/chord helpers) live once under `src/input/` /
  `src/surface_geom.rs` and are shared by both mode dispatchers.

## Non-goals

- **No single unified dispatcher.** Default and Tmux keep separate dispatch
  systems (gated by `AppMode`); they share *primitives*, not one monolithic
  system. (Brainstorming chose "shared primitives + mode dispatchers" over a
  single backend-branching dispatcher to avoid folding the large tmux state
  machine into the simple Default path.)
- **No rewrite of the tmux mouse/keyboard state machines.** Tmux's
  `decide_press` / `decide_release` / `decide_continuation` and prefix /
  copy-mode logic are RELOCATED and have their low-level `ClickTracker` /
  geometry swapped for the shared ones — not redesigned.
- **No behavior change.** Same routing, gating, ordering, and shortcuts.
  Verified by the relocated unit tests staying green.
- **No change to `orzma_tty_engine`.** `TerminalKeyInput` + `on_terminal_key_input`
  stay as-is; the host triggers `TerminalKeyInput` for PTY-backed surfaces.

## Decisions made during brainstorming

1. **Structure** = shared primitives + mode-specific dispatchers (not a single
   unified dispatcher, not a library-relocate-only move).
2. **Layout** = by function: all input dispatch lives under `src/input/`
   (`default_mode.rs` + a `tmux/` subtree), not under `src/mode/`.
3. **Tmux boundary** = only the *pure dispatchers* move to `src/input/tmux/`
   (`input.rs`, `mouse.rs` + `mouse/`, `gate.rs`, `forward.rs`, `pane_hit.rs`,
   `window_bar_input.rs`). State/UI-coupled files (`copy_mode.rs`,
   `confirm_prompt.rs`, `rename_prompt.rs`, `pane_focus.rs`) stay in
   `src/mode/tmux/`.
4. **`orzma_webview`** = decoupled from focus: `sync_focused_webview` moves to
   the host so `orzma_webview` no longer reads `KeyboardFocused` /
   `OrzmaTerminalInputSet`, letting `KeyboardFocused` move fully to `src/*`.

## Current architecture (reference)

Apply layer (stays):

- `orzma_tty_engine`: `TerminalKeyInput` (EntityEvent) + `on_terminal_key_input`
  observer (encodes a key, writes to the PTY). Defined under
  `crates/orzma_tty_engine/src/` (`events.rs` / `plugin.rs`).
- `orzma_terminal::action` (`src/action.rs`): `PasteAction` (`:12`) +
  `on_paste` (`:27`), `OrzmaActionPlugin` (`:19`).
- `orzma_terminal::mouse` apply: `on_terminal_mouse_effects` (`src/mouse.rs:891`)
  applies `MouseEffect` (`:242`, currently `pub(crate)`) carried by
  `TerminalMouseEffects` (`:280`, `pub(crate)`); emits `TerminalForwardInput`
  (`:267`, `pub`) for PTY-less (tmux) surfaces.

Cross-crate consumers of soon-to-move items:

- `crates/orzma_webview/src/webview/render.rs` reads `KeyboardFocused`
  (`:115`) in `sync_focused_webview` (`:113`) and schedules it
  `.after(OrzmaTerminalInputSet)` (`:90`). `orzma_webview` depends on
  `orzma_terminal` (path dep). A binary cannot be a dependency, so anything
  `orzma_webview` consumes must otherwise live in a library crate — decision (4)
  removes this consumption instead.
- `KeyboardFocused` is WRITTEN only by the host today
  (`src/mode/default.rs:87`, `src/mode/tmux/pane_focus.rs:100`,
  `src/mode/tmux/adopt.rs`); the library and `orzma_webview` only READ it.
- `KeyboardDisabled` / `MouseDisabled` have no library-crate readers once the
  library dispatch is removed (host readers: `src/ui/copy_mode.rs`,
  `src/input/hyperlink.rs`, host gates).

## Target architecture

### `orzma_terminal` surface — after

Keeps (apply + spawn vocabulary):

- `PasteAction` + `on_paste`.
- `TerminalMouseEffects` (→ `pub`, constructed via `pub fn new(entity, effects)`
  — fields stay private per the visibility rule), `MouseEffect` (→ `pub`;
  variants inherit it), `TerminalForwardInput`, `on_terminal_mouse_effects`.
- `OrzmaTerminal` + spawn/config (`OrzmaTerminalBundle`, `OrzmaSpawnOptions`,
  `OrzmaTerminalConfig`, `cells_for`, `resolve_shell`), `Clipboard` /
  `build_paste_bytes`, `ExitPlugin`, `LayoutPlugin`, `on_add_inject_render`.
- `OrzmaTerminalPlugin` adds only: `ExitPlugin`, `LayoutPlugin`, the apply
  observers (`on_paste`, `on_terminal_mouse_effects`), `on_add_inject_render`.
  `OrzmaInputPlugin` is deleted; `OrzmaMousePlugin` is reduced to the apply
  observer registration.

Removed (→ host):

- Systems: `dispatch_input`, `dispatch_mouse_buttons`, `dispatch_mouse_wheel`.
- Deciders: `decide_button`, `decide_wheel`, `update_selection`,
  `resolve_button_event`, `synthesize_drag`, and the wheel/modifier helpers
  (`protocol_mods`, `build_wheel_modifiers*`, `fine_held`, `map_button`,
  `wheel_delta_cells`).
- State: `OrzmaMouseGesture`, `DragGesture`, `HeldPointer`, `ClickTracker`,
  `WheelAccumulator`, `accumulate_notches`.
- Geometry: `cell_at_local`, `cell_at_cursor`, `to_viewport_point`,
  `CellContext` → `src/surface_geom.rs`. `topmost_terminal_at` is byte-identical
  to the host's existing `topmost_surface_at` (`src/webview_pointer.rs:207`) —
  delete it and reuse that, rather than relocating a duplicate.
- Policy/config: `OrzmaMouseConfig`, `FineModifier`, `TerminalInputBindings`,
  `ReservedChord`.
- Keyboard mapping: `bevy_key_to_terminal_key`, `chord_matches`,
  `current_terminal_modifiers`.
- Markers / sets: `KeyboardFocused`, `KeyboardDisabled`, `MouseDisabled`,
  `OrzmaTerminalInputSet`, `OrzmaTerminalMouseSet`.

`orzma_tty_engine` is unchanged.

### Host module layout — after

```
src/input/
  keyboard.rs      (NEW) keyboard primitives (modifiers, chord, Key → TerminalKey)
                         + relocated terminal keyboard dispatch (dispatch_input),
                         marker-gated (KeyboardFocused + !KeyboardDisabled) — naturally
                         Default-only because tmux marks every pane KeyboardDisabled
  mouse.rs         (NEW) relocated terminal MOUSE dispatch (dispatch_mouse_buttons /
                         dispatch_mouse_wheel) + deciders + engine-Side geometry.
                         MODE-NEUTRAL, marker-gated (!MouseDisabled): serves the Default
                         shell AND normal tmux panes (→ TerminalForwardInput). NOT
                         AppMode::Default-gated.
  gesture.rs       (NEW) the single ClickTracker, WheelAccumulator,
                         accumulate_notches, drag gesture types
  bindings.rs      (NEW) InputBindings (paste + reserved), MouseConfig,
                         FineModifier      [was TerminalInputBindings / OrzmaMouseConfig]
  focus.rs         (NEW) KeyboardFocused, KeyboardDisabled, MouseDisabled markers
                         + sync_focused_webview (moved from orzma_webview)
  default_mode.rs  (MOVED) was src/mode/default/input.rs — default-host glue only
                         (maintain_input_gates, app_shortcut_handler, IME-commit routing)
  tmux.rs + tmux/  (MOVED) was src/mode/tmux/{input, mouse, mouse/*, forward,
                         gate, pane_hit, window_bar_input}
  hyperlink.rs · ime.rs · option_as_alt.rs · shortcuts.rs   (existing)

src/surface_geom.rs   (EXPANDED) absorbs cell_at_cursor, to_viewport_point.
                      NOTE: cannot simply "join" the existing cell_at_local — the
                      library cell helpers return (CellCoord, orzma_tty_engine::Side),
                      surface_geom returns (u32, u32, surface_geom::Side), and tmux
                      copy-mode wants zero-based (u16, u16). Keep distinct adapters
                      per coordinate/Side convention; the Default decider must emit
                      orzma_tty_engine::Side (carried by MouseEffect::SelStart).

src/mode/{default,tmux}/   non-dispatch mode behavior only (render, copy_mode
                      state, pane_focus, prompts, window_bar render, adopt,
                      locale, webview_tokens, …)
```

`KeyboardFocused` / `KeyboardDisabled` / `MouseDisabled` move to
`src/input/focus.rs`; their import sites across `src/*` switch from
`orzma_terminal::…` to `crate::input::focus::…`. The host remains the sole
writer of `KeyboardFocused`.

### `orzma_webview` decoupling

Production usage is one system in `crates/orzma_webview/src/webview/render.rs`,
but a test in `control_plane.rs` also imports it, so the move touches two files:

- Move `sync_focused_webview` to `src/input/focus.rs` (co-located with the
  `KeyboardFocused` marker it queries); the host registers it
  `.after(InputPhase::FocusedKey)` (keyboard focus resolved) and writes
  `bevy_cef`'s `FocusedWebview` exactly as today.
- The moved system imports `WebviewSource` from `bevy_cef::prelude` — it is a
  `bevy_cef` type (the host already imports it at `src/input/hyperlink.rs:23`),
  NOT an `orzma_webview` type, so no `orzma_webview` export is needed.
  (`Webview` / `NonInteractive` are already `pub` and host-used.)
- Move/rewrite the `orzma_webview` tests that reference the system — the
  `render.rs` `#[cfg(test)]` cases and
  `sync_preserves_app_declared_focus_from_control_plane`
  (`crates/orzma_webview/src/control_plane.rs:1911`) — into binary-side tests (or
  replace the control-plane one with a crate-local `SetFocus` test), since a
  crate cannot import a binary module.
- `orzma_webview` drops the `orzma_terminal::{KeyboardFocused, OrzmaTerminalInputSet}`
  import; `RenderPlugin` no longer registers the system. The crate still depends
  on `orzma_terminal` for `OrzmaTerminal` (unchanged).

## Data flow (apply layer unchanged)

- **Default key**: `KeyboardInput` → `src/input/default_mode.rs` (modifiers +
  GUI-shortcut gate + `bevy_key_to_terminal_key`, from `src/input/keyboard.rs`)
  → `commands.trigger(TerminalKeyInput | PasteAction)` → library observer
  encodes/writes the PTY.
- **Mouse (shared, mode-neutral)**: `MouseButtonInput` / `CursorMoved` / `MouseWheel`
  → `src/input/mouse.rs` (hit-test, click count via the shared `ClickTracker`,
  `decide_button` / `decide_wheel` over the engine routers) →
  `commands.trigger(TerminalMouseEffects { effects })` → `on_terminal_mouse_effects`
  applies selection/scroll/copy, OR (PTY-less tmux pane) emits `TerminalForwardInput`.
  Marker-gated by `Without<MouseDisabled>`, so it drives BOTH the Default shell and
  normal tmux panes; gate.rs MouseDisables a pane only in copy-mode/modal/webview
  cases, where tmux's own gesture takes over. tmux's `forward_wheel_to_tmux` owns
  ONLY copy-mode + alt-screen wheel; every other wheel is ceded to this dispatch.
- **Tmux-specific** (copy-mode selection, divider resize, prefix/copy-mode keys):
  `src/input/tmux/` with its own deciders + `TmuxMouseEffects` / `send-keys`.

## Migration sequence (each phase compiles + tests green)

0. **Extract the shared seam first** — relocate the primitives every later phase
   leans on into the host shared layer: `current_terminal_modifiers` (+ the
   modifier/chord helpers) → `src/input/keyboard.rs`, the unified `ClickTracker` /
   wheel accumulation → `src/input/gesture.rs`, cell geometry → `src/surface_geom.rs`.
   Load-bearing: the mouse helpers `protocol_mods` / `build_wheel_modifiers` call
   `current_terminal_modifiers`, so the mouse move (Phase 3) cannot precede it —
   the original "mouse before keyboard" order would not compile.
1. **Decouple `orzma_webview` from focus** — move `sync_focused_webview` →
   `src/input/focus.rs`; import `WebviewSource` from `bevy_cef::prelude`; move the
   referencing `orzma_webview` tests (`render.rs` + `control_plane.rs:1911`) to the
   binary; drop the `KeyboardFocused` / `OrzmaTerminalInputSet` import.
   (`KeyboardFocused` is still defined in the library here; the host imports it.)
2. **Publish the mouse apply API** — add `TerminalMouseEffects::new` and make it +
   `MouseEffect` `pub`. The library still owns dispatch, so this is an additive,
   green step that lets host code construct the event before the dispatch moves.
3. **Move the shared mouse dispatch to the host** — relocate `dispatch_mouse_buttons`
   / `dispatch_mouse_wheel` + deciders + engine-Side geometry into the MODE-NEUTRAL
   `src/input/mouse.rs`, marker-gated by `Without<MouseDisabled>` (NOT AppMode-gated —
   it serves normal tmux panes too, ceded to via `forward_wheel_to_tmux`'s
   `CededToOrzma` branch). Emit `orzma_tty_engine::Side` via the adapters; point the tmux
   mouse at the shared `ClickTracker` (delete its duplicate). THEN delete the library
   mouse dispatch systems/resources, leaving `on_terminal_mouse_effects`. Move
   `MouseDisabled` + the mouse config to `src/input/{focus,bindings}.rs` together with
   the host modules that import them (`shortcuts.rs`, `hyperlink.rs`, `gate.rs`,
   `ui/copy_mode.rs`).
4. **Move Default keyboard dispatch to the host** — relocate `dispatch_input`,
   `bindings` (keyboard half), and `KeyboardFocused` / `KeyboardDisabled` →
   `src/input/{keyboard,bindings,focus}.rs`; update all `KeyboardFocused` import
   sites; delete `orzma_terminal::input` / `OrzmaInputPlugin`.
5. **Delete the library input system sets** — remove `OrzmaTerminalInputSet` /
   `OrzmaTerminalMouseSet`; host ordering uses `InputPhase`; slim
   `OrzmaTerminalPlugin`.
6. **Final relocation** — move the dispatch files into `src/input/default_mode.rs`
   and `src/input/tmux/`; update `mod` declarations + imports. (Earlier phases may
   land code directly in the final paths to fold this in.)

## Testing

- Relocate the moved primitives' unit tests with them: `ClickTracker`,
  `decide_button` / `decide_wheel`, `accumulate_notches`, `cell_at_*` /
  `topmost_terminal_at`, the gesture truth-tables, and the `sync_focused_webview`
  tests. These run without a PTY/GPU/App and should pass unchanged.
- Keep the Default-dispatch behavior tests (paste chord, reserved chord,
  meta-drop, focus routing) — relocate from `crates/orzma_terminal/src/input.rs`
  to the host Default dispatcher module.
- No new behavior tests required for a no-behavior-change refactor; green suite
  is the acceptance gate.

## Risks

- **Wide `KeyboardFocused` import churn** — many `src/*` files plus removal from
  `orzma_webview`; mechanical but broad. Mitigated by doing it in one phase (4).
- **System-set reordering** — `OrzmaTerminalInputSet` (`.in_set` for library
  dispatch, `.before` for host gates, `.after` for the moved webview-focus
  system) collapses into the host `InputPhase` sets. Must preserve the
  Hover → Dispatch → FocusedKey order and the gates-before-dispatch invariant.
- **`orzma_webview` decouple equivalence** — `sync_focused_webview` has subtle
  "preserve inline webview focus" / GC-on-despawn behavior; move it verbatim and
  rely on its relocated tests.
- **`MouseEffect` / `TerminalMouseEffects` visibility widening** — expose
  `TerminalMouseEffects::new` (fields stay private) + `pub` on `MouseEffect`;
  both need doc comments (public-API rule). Grows the library's intended surface,
  acceptable since the host now constructs them.

## Open questions

- Whether `src/input/default_mode.rs` should itself become a `src/input/default/`
  subtree if it grows large after absorbing keyboard + mouse + wheel + gates +
  shortcuts (defer; split if the single file exceeds comfort).
- Config-type rename — RESOLVED (spec review): keep `TerminalInputBindings` /
  `OrzmaMouseConfig` names on relocation; a no-behavior-change refactor should
  minimize import churn. A rename can be a separate follow-up.
