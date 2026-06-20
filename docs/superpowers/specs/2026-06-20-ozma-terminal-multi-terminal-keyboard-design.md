# ozma_terminal Multi-Terminal Keyboard Input — Design

**Date:** 2026-06-20
**Status:** Design (pre-plan)
**Crate:** `ozma_terminal` (+ host `src/ozma.rs`, `src/input/ime.rs`)

## Goal

Make the self-contained `ozma_terminal` crate's **keyboard** handling correct
when more than one `OzmaTerminal` entity is live at once, mirroring what the
multi-terminal **mouse** effort (#164,
`2026-06-20-ozma-terminal-multi-terminal-mouse-design.md`) did for mouse.

Because keyboard has no cursor, routing is by **sticky focus** rather than
hit-testing: exactly one terminal carries a `KeyboardFocused` marker and
receives all keyboard input — raw keys (`dispatch_input`) and IME (commit
routing + candidate-window anchoring) — until the host moves the marker.

## Scope

**In scope:**

- `dispatch_input` (`crates/ozma_terminal/src/input.rs`): route to the
  `KeyboardFocused` terminal instead of `terminal.single()` over
  `Without<KeyboardDisabled>`.
- New `KeyboardFocused` marker component in the crate, exported from `lib.rs`.
- Host IME (`src/input/ime.rs`): `read_ime_events` commit routing **and**
  `ime_policy_system` enable/anchor — both follow `KeyboardFocused`.
- Host: insert `KeyboardFocused` on the spawned terminal (`src/ozma.rs`).
  `maintain_input_gates` (`src/ozma_input.rs`) keeps its responsibility (toggle
  the modal `KeyboardDisabled` / `MouseDisabled` markers on all terminals).

**Out of scope (flagged dependencies, not changed here):**

- **Focus *policy*.** What actually *moves* focus between terminals
  (click-to-focus, a cycle keybind, layout/z-order) is a separate effort. The
  crate stays mechanism-only ("host-driven marker only"), and the host still
  spawns exactly one terminal — so the marker never moves yet. This change is
  forward-looking capability, exactly as the mouse PR shipped topmost routing
  before any multi-terminal host existed.
- **Layout / multi-terminal spawning.** `LayoutPlugin::resize_to_window`
  (`layout.rs`) and `src/ozma.rs spawn_terminal` remain single-terminal. A real
  multi-terminal host owns per-terminal sizing/positioning and PTY resize.
- **Exit.** `exit.rs` multi-terminal exit policy is unaddressed (as in the mouse
  effort).
- **Single-window assumptions retained.** One window, many terminals.

## Background — why it breaks today

The keyboard path assumes exactly one interactive terminal:

- `dispatch_input` (`input.rs:88`) calls `terminal.single()` over
  `Query<Entity, (With<OzmaTerminal>, Without<KeyboardDisabled>)>`. The moment a
  second un-disabled `OzmaTerminal` exists, `.single()` returns `Err` and **all**
  keyboard input is dropped (`events.clear()`).
- The host IME path has the same shape in two places, both in `src/input/ime.rs`:
  - `read_ime_events` (`:357`, Ozma arm) routes the IME commit text via
    `ozma_terminal.single()` → `TerminalKeyInput`. With 2+ terminals the commit
    is dropped.
  - `ime_policy_system` (`:226`, Ozma arm) toggles `ime_enabled =
    ozma_terminal.single().is_ok()`; with 2+ terminals `.single()` → `Err` →
    `is_ok()` is false → IME is silently disabled entirely. (This arm also never
    anchors `ime_position` in Ozma mode today — it returns right after the toggle
    — so Ozma-mode candidate-window anchoring is a pre-existing gap.)

### Precedent: the multi-terminal mouse effort

#164 made the crate's **mouse** path multi-terminal capable: topmost-under-cursor
routing, a per-entity `MouseDisabled` marker, and entity-locked gestures. It
explicitly punted keyboard:

> **Keyboard routing.** `dispatch_input` keeps `terminal.single()` over
> `Without<KeyboardDisabled>`. With multiple terminals the host is responsible
> for keeping exactly one terminal un-`KeyboardDisabled` (the keyboard-focused
> one). A true multi-terminal keyboard-focus mechanism is a separate effort.

This design is that separate effort. It replaces the fragile "keep exactly one
un-`KeyboardDisabled`" contract with an explicit positive `KeyboardFocused`
marker, so modal gating (`KeyboardDisabled`) and focus (`KeyboardFocused`) are
independent concerns.

## Design decisions (from brainstorming)

1. **Routing model:** sticky focus — exactly one `KeyboardFocused` terminal
   receives keyboard input. (Mouse routes by cursor; keyboard has no cursor.)
2. **Focus is host-driven, marker only:** the crate exposes the `KeyboardFocused`
   marker and routes to it; it makes **no** focus decisions. The host owns all
   focus policy. Purest mirror of the mouse PR's "crate = mechanism, host =
   policy" split.
3. **Representation:** a `KeyboardFocused` marker **component**, consistent with
   the crate's existing per-entity marker gating (`KeyboardDisabled`,
   `MouseDisabled`). The "exactly one focused" invariant is the host's
   responsibility; two markers → `.single()` `Err` → keys dropped (same failure
   shape as today).
4. **IME follows focus:** both host IME systems target `KeyboardFocused`, so raw
   keys, IME commits, and the candidate-window anchor all follow one focus.
5. **Marker ownership:** the **host** inserts `KeyboardFocused` (spawn
   `(bundle, KeyboardFocused)`); the crate's `OzmaTerminalBundle` does **not**
   default to focused (see Alternatives).

## Architecture

The pure keyboard logic — `current_terminal_modifiers`, `chord_matches`,
`bevy_key_to_terminal_key` — is **unchanged**. Only the routing query, one new
marker, and the host integration change. The gather → decide → apply seam is
intact: `dispatch_input` still gathers events and `commands.trigger`s
`PasteAction` / `TerminalKeyInput` at `ev.entity`; the apply observers
(`on_terminal_key_input`, `on_paste`) target `ev.entity` and need no change.

### 1. `KeyboardFocused` marker

New marker beside `KeyboardDisabled` in `input.rs`, re-exported from `lib.rs`:

```rust
/// When present on an `OzmaTerminal` entity, that terminal is the keyboard
/// focus: the crate's keyboard dispatcher routes raw keys to it (and the host
/// routes IME commits / anchors the candidate window to it). The host
/// maintains the "exactly one focused" invariant; a terminal with no
/// `KeyboardFocused` receives no keyboard input.
#[derive(Component)]
pub struct KeyboardFocused;
```

- `KeyboardFocused` lives in `input.rs` (alongside `KeyboardDisabled`), exported
  from `lib.rs` so the gating + focus API is colocated at the export site.
- It is distinct from the modal `KeyboardDisabled` (see §2). The crate makes no
  focus decisions; the host is the sole writer.

### 2. `dispatch_input` routing

The query gains the positive focus filter:

```rust
terminal: Query<Entity, (With<OzmaTerminal>, With<KeyboardFocused>, Without<KeyboardDisabled>)>,
```

- Still `.single()`; still `events.clear()` + `return` on `Err`. The body is
  otherwise unchanged.
- The existing `// NOTE:` at the `.single()` site is rewritten: the host MUST
  keep exactly one terminal `KeyboardFocused`-and-not-`KeyboardDisabled`, or
  `.single()` returns `Err` and every keypress is silently dropped.

### 3. `KeyboardFocused` vs `KeyboardDisabled` — orthogonal

| Marker | Meaning | Lifetime | Writer |
| --- | --- | --- | --- |
| `KeyboardFocused` | "this terminal is the keyboard target" | sticky (set on spawn; moved by future focus policy) | host (focus policy) |
| `KeyboardDisabled` | modal suppression (picker / IME-composing / webview-focused / window-unfocused) | transient | host (`maintain_input_gates`) |

- During a modal, `maintain_input_gates` marks **all** terminals
  `KeyboardDisabled` (unchanged). The focused terminal is then *also*
  `KeyboardDisabled` → the dispatch set is empty → keys drop. Outside a modal,
  the focused terminal is the sole un-disabled focused one → routes.
- This reproduces today's single-terminal behavior exactly, with **no new global
  resource**. The host does **not** remove `KeyboardFocused` during modals —
  only `KeyboardDisabled` toggles.

### 4. IME follows focus (host, `src/input/ime.rs`)

Both Ozma-arm `.single()` sites retarget to the `KeyboardFocused` terminal.

- **`read_ime_events`** (Ozma arm): replace `ozma_terminal.single()` with a
  focus query `Query<Entity, (With<OzmaTerminal>, With<KeyboardFocused>)>`.
  **Critical (`// NOTE:`):** this path does **not** gate on `KeyboardDisabled`.
  IME composition itself is one of the modal triggers that sets
  `KeyboardDisabled` (suppressing raw keys via `dispatch_input`), but the IME
  *commit* must still land on the focused terminal. Gating the commit on
  `KeyboardDisabled` would drop every committed composition.
- **`ime_policy_system`** (Ozma arm): today, with no tmux `ActivePane`, it only
  sets `ime_enabled = ozma_terminal.single().is_ok()` and returns without
  anchoring. Change it to treat the `KeyboardFocused` terminal as the surface:
  - `ime_enabled` ← a `KeyboardFocused` terminal exists;
  - anchor `ime_position` at that terminal's cursor by reusing the **existing**
    tmux anchoring math — `anchors.get(focused)` over
    `(&ComputedNode, &UiGlobalTransform, &TerminalGrid)`, which `OzmaTerminal`
    already carries (the mouse hit-test reads the same components). This fixes
    the multi-terminal break **and** closes the pre-existing Ozma-mode
    no-anchor gap, so the candidate window appears at the focused terminal's
    cursor (one row below, per the existing macOS anchoring `// NOTE:`).
  - Copy-mode handling stays as in the tmux path where applicable; the Ozma
    terminal has no tmux `CopyModeState`, so the `desired = !in_copy_mode` guard
    naturally resolves to "enabled" for it.

### 5. Host integration & marker ownership (`src/ozma.rs`, `src/ozma_input.rs`)

- `maintain_input_gates` (`src/ozma_input.rs`) is **unchanged** in
  responsibility: it toggles `KeyboardDisabled` / `MouseDisabled` on every
  terminal from the global `disable` bool. It does **not** touch
  `KeyboardFocused`.
- `spawn_terminal` (`src/ozma.rs`) spawns `(bundle, KeyboardFocused)` so the one
  host terminal is the keyboard target for its lifetime. A future multi-terminal
  host moves the marker via its focus policy.

## Alternatives considered

- **Resource `KeyboardFocus(Option<Entity>)`** instead of a marker component.
  Single source of truth (structurally impossible to have "two focused"), and
  `dispatch_input` would read + validate the entity. **Rejected:** it introduces
  a different vocabulary than the crate's existing per-entity markers
  (`KeyboardDisabled`, `MouseDisabled`); the marker keeps the gating + focus API
  uniform and the failure mode (`.single()` `Err`) identical to today.
- **Crate's `OzmaTerminalBundle` defaults to `KeyboardFocused`** (friendlier
  spawn-and-go). **Rejected:** spawning two terminals → both focused →
  `.single()` `Err`; the host would then have to remove the marker from
  all-but-one. That contradicts "host-driven marker only" and adds a
  multi-terminal footgun. The host inserts the marker explicitly instead.
- **Reuse `KeyboardDisabled` alone** (host keeps exactly one un-disabled, the
  original mouse-PR note's contract). **Rejected:** it conflates the transient
  modal gate with sticky focus — a modal would have to remember and restore
  which terminal was focused. The positive `KeyboardFocused` marker separates the
  two concerns cleanly.

## Change surface

**Crate `ozma_terminal`:**

| File | Change |
| --- | --- |
| `input.rs:14-18` | add `pub struct KeyboardFocused;` + doc, beside `KeyboardDisabled` |
| `input.rs:88-101` | `dispatch_input` query adds `With<KeyboardFocused>`; rewrite the `.single()` `// NOTE:` |
| `input.rs` tests | spawn `(OzmaTerminal, KeyboardFocused)`; add multi-terminal / focused-but-disabled / no-focus cases (see Testing) |
| `lib.rs:21` | export `KeyboardFocused` |
| `spawn.rs` | `OzmaTerminal` doc: note keyboard focus is host-driven via `KeyboardFocused` (mirror the mouse-doc multi-terminal premise fix) |

**Host:**

| File | Change |
| --- | --- |
| `src/ozma.rs` (`spawn_terminal`) | spawn `(bundle, KeyboardFocused)`; import `KeyboardFocused` |
| `src/input/ime.rs` (`read_ime_events`, Ozma arm) | query `KeyboardFocused` terminal; `// NOTE:` it must not gate on `KeyboardDisabled` |
| `src/input/ime.rs` (`ime_policy_system`, Ozma arm) | enable + anchor `ime_position` at the `KeyboardFocused` terminal's cursor (reuse tmux math) |

## Testing

- **Routing — focus required:** existing `dispatch_input` tests spawn
  `(OzmaTerminal, KeyboardFocused)` (now required to receive keys); plain-key,
  paste-chord, reserved-chord, meta-chord cases stay green.
- **Multi-terminal dispatch:** two live terminals, one `KeyboardFocused` → only
  the focused one receives keys (proves the old `.single()`-drops-everything
  failure is gone).
- **Gating wins over focus:** a `KeyboardFocused`-but-`KeyboardDisabled`
  terminal drops keys (modal still suppresses).
- **No focus:** zero `KeyboardFocused` terminals → keys dropped (`.single()`
  `Err` path).
- **Pure logic untouched:** `bevy_key_to_terminal_key` / `chord_matches` /
  `current_terminal_modifiers` tests stay green.
- **Host IME:** the commit-routing change is a query-filter swap; existing IME
  behavior holds once spawns carry `KeyboardFocused`. Add a focused-routing
  assertion where the `NonSend TmuxConnection` deps allow (the Ozma arm path
  triggers `TerminalKeyInput`, which is observable as in the existing crate
  tests).

## Constraints (repo coding rules)

- Rust 2024 / toolchain 1.95. No `mod.rs`. Comments only `// TODO:` / `// NOTE:`
  / `// SAFETY:`, English. Every `pub` item `///`-documented; module files `//!`.
- Imports in one top block, no inline fully-qualified paths.
- Bevy: mutable `SystemParam`s before immutable; whole-system change gates via
  `run_if` not in-body early return; `Plugin::build` one method chain; `Query`
  params descriptive nouns (no `_q`).
- Visibility minimized; private items last in a block; `#[expect(reason=…)]` over
  `#[allow]`.
