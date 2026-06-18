# Configurable terminal shortcuts (ozmux-owned keys)

Date: 2026-06-18
Branch: `terminal-shortcuts`
Status: design approved, pending spec review

## Problem

ozmux already ships a complete shortcut-config schema
(`crates/configs/src/shortcuts.rs`): a `Bindings` table, a `"Cmd+V"`-shape
chord parser, and cross-action conflict validation. But that schema is **not
wired into the runtime**. The actual GUI-chord dispatch in
`src/tmux_input.rs::gui_chord()` is hardcoded:

- `Cmd+Shift+P` → open session picker
- `Cmd+Q` → quit
- `Cmd+V` → paste
- (`Ctrl+Shift+Escape` → release inline-webview focus, hardcoded separately at
  the `focused_webview` branch around `tmux_input.rs:191`)

Because the hardcoded defaults coincide with the config defaults, the
disconnect has gone unnoticed. Editing `~/.config/ozmux/config.toml` has no
effect on these keys today.

Furthermore, `open-picker` and `quit` are not even present in the config
schema — only `paste` and `release-inline-focus` are active fields (the
pane/window/surface entries are deprecated and ignored because tmux owns
them).

## Goal

Make the four ozmux-GUI-owned shortcuts editable from
`~/.config/ozmux/config.toml`:

- `paste`
- `release-inline-focus`
- `open-picker` (new schema field)
- `quit` (new schema field)

tmux-side key bindings continue to be read from tmux via `list-keys`
(`crates/tmux_session/src/keybindings.rs`) — **unchanged**. This change
concerns only the chords ozmux intercepts before forwarding to tmux.

## Non-goals

- Hot reload of shortcuts. Load-at-startup only (restart to apply), consistent
  with every other ozmux config section.
- Conflict detection between an ozmux shortcut and a tmux binding. ozmux
  intercepts first and wins; documented, not enforced.
- Adding new *actions* (pane/window/surface operations stay owned by tmux).
- Changing the layout-stable (physical-key) matching semantics ozmux uses
  today.

## Central design decision: bridging logical config keys to physical runtime keys

The config `KeyChord` is **logical** (`Key::Char('v')` + `Modifiers`). The
runtime `KeyboardInput` event carries a **physical** `KeyCode` (`KeyCode::KeyV`)
plus modifier state. Today's `gui_chord()` matches on the physical `KeyCode`
deliberately — its doc comment calls this "layout-stable".

Chosen approach (**A**): resolve each configured logical `Key` to a physical
`KeyCode` **once at startup**, then match on exact `(KeyCode, Modifiers)`
equality at runtime. This preserves the existing layout-stable behavior, keeps
the `ozmux_configs` crate free of any `bevy` dependency (the translation lives
in the binary), and is cheap (a linear scan over at most four entries per key
event).

Rejected alternatives:

- **B — match the event's `logical_key`.** Avoids a `Key → KeyCode` table but
  flips behavior to layout-dependent, and the logical key shifts under
  Shift/Alt, making the modifier match awkward.
- **C — resolve per frame inside the dispatch system.** No extra resource, but
  re-resolves every frame (wasteful) and further bloats the already-large
  `forward_keys_to_tmux` system; violates the repo's "compute once" preference.

## Design

### 1. Config schema — `crates/configs/src/shortcuts.rs`

Add two active fields to `Bindings`:

```toml
[shortcuts.bindings]
paste                = "Cmd+V"              # existing
release-inline-focus = "Ctrl+Shift+Escape"  # existing
open-picker          = "Cmd+Shift+P"        # new
quit                 = "Cmd+Q"              # new
```

- New fields: `open_picker: Option<KeyChord>` (default `Cmd+Shift+P`),
  `quit: Option<KeyChord>` (default `Cmd+Q`). Both use the existing
  `#[serde(deserialize_with = "deser_chord_or_unbind")]` attribute so `""`
  means unbind.
- New `ShortcutAction` variants: `OpenPicker`, `Quit`.
- `Bindings::iter()` yields all four `(label, &Option<KeyChord>, ShortcutAction)`
  tuples.
- `Bindings::default()` seeds the two new fields with their default chords.
- `validate_no_conflicts()` already detects duplicate chords across all fields
  reported by `iter()`; with four active actions it now guards all four. The
  default set (`Cmd+V`, `Cmd+Shift+P`, `Cmd+Q`, `Ctrl+Shift+Escape`) is
  conflict-free. Conflicts remain fatal via the existing `exit(2)` path in
  `src/configs.rs`.

Unbinding semantics: a `None` field contributes no entry to the resolved table,
so e.g. `quit = ""` disables the GUI quit chord entirely (the user closes the
window through the OS).

### 2. Resolution layer — new `src/input/shortcuts.rs`

`ozmux_configs` must not depend on `bevy`, so the logical→physical translation
and the resolved table live in the binary. Declared from `src/input.rs` as
`pub(crate) mod shortcuts;` (no `mod.rs`, per repo rules).

Contents:

- `fn key_to_keycode(key: &Key) -> Option<KeyCode>` — maps the config `Key`
  enum to a physical `bevy::KeyCode`:
  - `Char('a'..='z')` → `KeyA..KeyZ`
  - `Char('0'..='9')` → `Digit0..Digit9`
  - `Escape/Space/Enter/Tab/Backspace/ArrowUp/Down/Left/Right` → the matching
    `KeyCode`
  - `Plus`: no dedicated physical `KeyCode` exists (`+` is Shift+Equal on US
    layouts); it has no clean layout-stable mapping and is not used by any
    default shortcut, so it resolves to `None` (skipped) in v1
  - anything else (e.g. `Key::Other`, unmapped punctuation) → `None`
- `ResolvedShortcuts(Vec<ResolvedShortcut>)` Bevy `Resource`, where
  `ResolvedShortcut { keycode: KeyCode, modifiers: Modifiers, action: ShortcutAction }`.
- `fn build_resolved_shortcuts(...)` — a `Startup` system that reads
  `Res<OzmuxConfigsResource>`, walks `bindings.iter()`, resolves each bound
  chord's `Key` to a `KeyCode` (logging and skipping any chord whose key has no
  `KeyCode` mapping), and inserts `ResolvedShortcuts`.
- Two query helpers on `ResolvedShortcuts`:
  - `fn match_gui_action(&self, keycode: KeyCode, mods: Modifiers) -> Option<ShortcutAction>`
    — exact `(keycode, modifiers)` match, **excluding** `ReleaseInlineFocus`
    (that action is meaningful only while a webview holds focus).
  - `fn is_release_inline_focus(&self, keycode: KeyCode, mods: Modifiers) -> bool`
    — exact match against the `ReleaseInlineFocus` entry only.

Modifier comparison reuses the existing `crate::input::current_modifiers()`,
which already converts `ButtonInput<KeyCode>` into an
`ozmux_configs::shortcuts::Modifiers` with `meta = super` (so `Cmd` ↔ the
config's `meta`). Matching is **exact modifier equality**, which preserves the
current behavior precisely — e.g. `Cmd+Q` requires Shift to be *up*, matching
the old `!mods.shift && super_` check.

`OzmuxShortcutPlugin` (`src/input.rs`) registers `build_resolved_shortcuts` in
`Startup`. Ordering is safe: `OzmuxConfigsPlugin` inserts `OzmuxConfigsResource`
in its `Plugin::build` (added before `OzmuxShortcutPlugin` in `main.rs`), so the
resource exists before any `Startup` system runs.

### 3. Runtime wiring — `src/tmux_input.rs`

- Delete `gui_chord()` and the `GuiChord` enum.
- Add `resolved: Res<ResolvedShortcuts>` to `forward_keys_to_tmux`'s params.
- Replace the per-event chord branch in the key loop with:

  ```text
  let mods = current_modifiers(&keys);            // Modifiers, meta = super
  if let Some(action) = resolved.match_gui_action(ev.key_code, mods) {
      *prefix_pending = false;                    // a GUI action abandons a pending prefix
      match action {
          OpenPicker        => picker.open = true,
          Quit              => exit.write(AppExit::Success),
          Paste             => { /* existing paste body verbatim */ }
          ReleaseInlineFocus => {}                // unreachable here (excluded from match_gui_action)
      }
      continue;
  }
  if mods.meta {                                  // Cmd held, no ozmux chord matched
      *prefix_pending = false;
      continue;                                   // swallow — tmux/PTY has no Super; never leak
  }
  if let Some(name) = bevy_key_to_tmux_name(...) { key_names.push(name); }
  ```

  The `mods.meta` swallow preserves the old `GuiChord::Other` safety net (any
  unhandled Cmd-modified key is dropped, never forwarded to tmux).

- Replace the hardcoded `Ctrl+Shift+Escape` test in the `focused_webview`
  branch (~`tmux_input.rs:191`) with
  `resolved.is_release_inline_focus(ev.key_code, mods)`. Behavior when no
  webview is focused is unchanged: the general loop excludes
  `ReleaseInlineFocus`, so the release chord (default `Ctrl+Shift+Escape`,
  no Cmd) falls through to `bevy_key_to_tmux_name` and forwards to tmux exactly
  as today.

### 4. Data flow

```
config.toml
  → OzmuxConfigs::load_blocking()            (startup, existing)
  → OzmuxConfigsResource                     (existing Resource)
  → [Startup] build_resolved_shortcuts       (logical Key → physical KeyCode, resolved once)
  → ResolvedShortcuts                         (new Resource)
  → forward_keys_to_tmux:
        match_gui_action → run action
        else Cmd held    → swallow (safety net)
        else             → forward to tmux (unchanged)
     focused_webview branch:
        is_release_inline_focus → release focus
```

## Error handling

- A bound chord whose `Key` has no `KeyCode` mapping is logged
  (`tracing::warn!`) and skipped during resolution — it behaves as unbound
  rather than crashing. In practice the config `Key` set is already constrained
  to the mappable letters/digits/named keys, so this is a defensive guard for
  forward-compat `Key::Other`.
- Duplicate-chord conflicts across the four actions remain fatal (`exit(2)`),
  surfaced by the existing `OzmuxConfigsPlugin` path.
- Parse/IO errors fall back to defaults with a warning, as today.

## Testing

`crates/configs/src/shortcuts.rs`:

- `open-picker`/`quit` defaults are `Cmd+Shift+P` / `Cmd+Q`.
- `Bindings::iter()` yields four entries (update `iter_yields_2_entries`).
- Default-shortcuts JSON snapshot updated to include the two new fields.
- `validate_no_conflicts` detects a user-introduced conflict among the four
  actions; the default set stays conflict-free.
- `""` unbinds `quit` / `open-picker`.

`src/input/shortcuts.rs`:

- `key_to_keycode`: `Char('v')→KeyV`, `Char('1')→Digit1`, `Escape→Escape`,
  arrows; `Key::Plus` and `Key::Other(..)` → `None`.
- `match_gui_action`: exact modifier equality (e.g. `Cmd+Q` does not match when
  Shift is also held); `Cmd+V` resolves to `Paste`; `ReleaseInlineFocus` is
  never returned.
- `is_release_inline_focus`: true only for the configured release chord.
- `build_resolved_shortcuts`: from default config produces the four expected
  entries; a chord with an unmappable key is skipped.

`src/tmux_input.rs`:

- Migrate the existing `gui_chord` tests (`cmd_shift_p_opens_picker`,
  `cmd_q_quits`, `cmd_v_is_paste`, `other_super_chord_is_swallowed`,
  `non_super_key_is_not_a_chord`) to drive the new matcher against a
  default-built `ResolvedShortcuts`.

## Files affected

- `crates/configs/src/shortcuts.rs` — schema (fields, `ShortcutAction`,
  `iter`, `Default`) + tests.
- `src/input.rs` — declare `pub(crate) mod shortcuts;`, register the
  `Startup` system in `OzmuxShortcutPlugin`.
- `src/input/shortcuts.rs` — **new**: `key_to_keycode`, `ResolvedShortcuts`,
  `build_resolved_shortcuts`, matchers + tests.
- `src/tmux_input.rs` — remove `gui_chord`/`GuiChord`, consume
  `Res<ResolvedShortcuts>`, swap the two dispatch sites; migrate tests.
- `src/configs.rs` — unaffected (conflict-fatal path already covers four
  actions).

## Open questions

None blocking. (Documenting the four config keys in any user-facing config
reference is a minor follow-up if such a doc exists.)
