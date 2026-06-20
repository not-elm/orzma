# Dynamic Window Title — Design

Date: 2026-06-20
Status: Draft (brainstormed; pending review)

## Problem

The OS window title is a static string `"ozmux"`, set once in `src/main.rs`
(the `WindowPlugin` primary window, currently around `src/main.rs:65`). It
never reflects what the user is doing, so the OS window switcher / taskbar /
window manager cannot distinguish one ozmux window or context from another.

We want the title to track the active context, with different content per
`AppMode`.

## Decisions

| Mode | Content | Example |
| --- | --- | --- |
| `Ozmux` | `session:active-window` + suffix | `main:vim — ozmux` |
| `Ozma` | focused terminal's OSC title + suffix | `~/src — ozmux` |
| Fallback (title unknown) | app name only | `ozmux` |

- The app-name suffix ` — ozmux` (space, em dash `—`, space, `ozmux`) is
  appended whenever there is leading context, mirroring the common
  "context — AppName" convention so the window stays identifiable in OS
  switchers.
- When there is no leading context, the title is just `ozmux` (no dangling
  separator).
- Formats are fixed, not user-configurable (no config template in this
  iteration — YAGNI).

## Relevant existing facts (verified against the codebase)

- `AppMode` is a `pub(crate)` `States` enum in `src/ozma.rs` with variants
  `Ozma` (default) and `Ozmux`. Reachable from a new sibling module in `src/`.
- Every Ozma terminal carries a `TerminalTitle(Option<String>)` component: it
  is a field of `TerminalBundle` (`crates/ozma_tty_engine/src/bundle.rs`),
  which `OzmaTerminalBundle` embeds (`crates/ozma_terminal/src/spawn.rs`). The
  engine updates it from OSC 0/2 sequences and also fires a
  `TerminalTitleChanged` event.
- The keyboard target in Ozma mode is the single entity marked
  `KeyboardFocused` + `OzmaTerminal` (`crates/ozma_terminal/src/input.rs`).
  Multiple Ozma terminals may exist; the title should follow the focused one.
- Ozmux mode projects a single `TmuxSession { id, name }`
  (`crates/tmux_session/src/components.rs`). `name` is empty until the first
  `%session-changed`.
- The active window is the single entity matching
  `(&TmuxWindow, With<ActiveWindow>)`; `TmuxWindow { id, index, name }`.
  `src/tmux/window_bar.rs` already reads session + windows this exact way
  (`session.iter().next()`, iterate `TmuxWindow`), which the new code mirrors.

## Approach (chosen)

Two mode-gated `Update` systems + a pure formatter + a conditional write.
Rejected alternatives: an intermediate `DesiredWindowTitle` resource +
applier (extra indirection the conditional write already provides — YAGNI),
and an event/observer-driven design (tmux name changes arrive as component
mutations, not events, and mode-transition + focus-change handling would add
wiring for a system that is already cheap).

### Module

New file `src/window_title.rs` exposing `pub(crate) WindowTitlePlugin` as its
only non-private item (formatter helpers, constants, and systems stay private
to the module). The file opens with a `//!` module doc (required for every
module file by the repo rules) and a `///` on `WindowTitlePlugin`; items are
ordered pub-first — `pub(crate) struct WindowTitlePlugin` and its `impl
Plugin` precede the private constants, systems, formatters, and the
`#[cfg(test)]` module. Registered in `src/main.rs` alongside the other ozmux
plugins. The static `title: "ozmux"` in `main.rs` stays as the boot value
(correct first-frame title before either system runs).

### Pure formatters (no ECS; unit-testable)

```rust
/// Returns the desired window title for Ozma mode.
fn format_ozma(title: Option<&str>) -> String;
//  Some(t) where t is non-empty  => "{t} — ozmux"
//  None or empty                 => "ozmux"

/// Returns the desired window title for Ozmux mode.
fn format_ozmux(session: &str, window: Option<&str>) -> String;
//  session non-empty + Some(w) non-empty => "{session}:{w} — ozmux"
//  session non-empty + None/empty        => "{session} — ozmux"
//  session empty                         => "ozmux"
```

`"ozmux"` and the suffix ` — ozmux` are defined once as a shared constant to
avoid drift.

### Apply helper (honest change detection)

```rust
/// Writes `window.title` only when it differs, so Bevy change detection on
/// `Window` fires solely on real changes.
fn apply_title(window: &mut Window, desired: String) {
    if window.title != desired {
        window.title = desired;
    }
}
```

This follows the repo's "let mutation drive change detection — mutate
conditionally" rule. Note the primary payoff is avoiding `Changed<Window>`
churn every frame: `bevy_winit`'s `changed_windows` system already diffs the
title against its `CachedWindow` before calling the OS `set_title`, so a
redundant write would not reach winit — but it *would* trip `Changed<Window>`
and force that whole-`Window` field diff to run each frame. The conditional
write prevents that and keeps the invariant honest for any future
`Changed<Window>` consumer.

### Systems (both take `Query<&mut Window, With<PrimaryWindow>>`; mutable params first)

- `update_ozma_window_title.run_if(in_state(AppMode::Ozma))`
  - Query `(&TerminalTitle, (With<OzmaTerminal>, With<KeyboardFocused>))`.
  - When `.single()` resolves a unique focused terminal, apply
    `format_ozma(title.0.as_deref())` — a `None`/empty OSC title yields the
    `ozmux` fallback (the "title unknown" row of the Decisions table).
  - When `.single()` returns zero/many (no *unique* focused terminal — a
    transient during spawn or a focus handoff between terminals), leave the
    title unchanged that frame. This is deliberately distinct from the `ozmux`
    fallback: re-applying `ozmux` on a one-frame focus gap would flicker the
    title, so the last good title is held instead.
- `update_ozmux_window_title.run_if(in_state(AppMode::Ozmux))`
  - Resolve `Query<&TmuxSession>` and
    `Query<&TmuxWindow, With<ActiveWindow>>` with `.iter().next()` (or
    `.single().ok()`), matching `window_bar.rs`. Do *not* use
    `Query<Option<&TmuxSession>>` — `Option<&T>` matches entities that lack
    the component, which is the wrong shape for an optional singleton.
  - `apply_title(window, format_ozmux(session_name, window_name))`.

Both run every frame while in their mode; the work is one small query plus a
string compare, and the conditional write keeps change detection honest.
Whole-system gating is the `in_state` run condition; no in-body early-return
change guard is used. The two systems share a write-write data access on
`Window`, so Bevy's executor serializes them (never concurrent) in an
unspecified order; since `in_state` guarantees at most one fires per frame,
the order is irrelevant and no explicit `.before`/`.after`/`.chain` is needed.
Collapsing both into one system that `match`es on `Res<State<AppMode>>` is a
viable alternative but widens the per-frame data-access footprint; the
two-system `in_state` split is kept for clarity.

Future optimization (deferred — not in this iteration): if the per-frame
`&mut Window` scheduling cost is ever measured to matter, gate each system
with dirty predicates mirroring `window_bar_dirty` (`Changed<TerminalTitle>`
plus `Added`/`RemovedComponents<KeyboardFocused>` for Ozma;
`Changed<TmuxSession>` / `Changed<TmuxWindow>` plus
`Added`/`RemovedComponents<ActiveWindow>` for Ozmux) and add an
`OnEnter(mode)` refresh so mode switches still update. Both reviewers judged
the every-frame form acceptable for v1.

### Data flow

- OSC title change → engine updates `TerminalTitle` → Ozma system reflects it
  on the next frame.
- tmux `%session-changed` / `%window-pane-changed` → `TmuxSession` /
  `ActiveWindow` update → Ozmux system reflects it.
- Mode switch: after the `StateTransition`, the new mode's `in_state` system
  runs the next frame and corrects the title. No `OnEnter` hook is required;
  at most one frame of staleness, which is imperceptible.

### Edge cases

- Ozmux mode before a connection exists (picker open / connecting, no
  `TmuxSession`) → `ozmux`.
- `TmuxSession.name` empty (pre `%session-changed`) → `ozmux`.
- Active window not yet known → `{session} — ozmux`.
- OSC title `None` or empty in Ozma → `ozmux`.
- Focus moves between Ozma terminals → title follows `KeyboardFocused`.

## Testing

- Unit tests on `format_ozma` / `format_ozmux` covering every branch:
  - Ozma: `Some("vim")` → `vim — ozmux`; `Some("")` → `ozmux`; `None` →
    `ozmux`.
  - Ozmux: `("main", Some("vim"))` → `main:vim — ozmux`;
    `("main", None)` → `main — ozmux`; `("", _)` → `ozmux`.
- Integration tests, one per system, using a minimal `App` that inserts the
  `AppMode` state and spawns a `(Window, PrimaryWindow)` entity — the systems
  query `&mut Window, With<PrimaryWindow>` and `MinimalPlugins` creates no
  window, so follow the `src/font.rs` test pattern that spawns
  `(Window, PrimaryWindow)`. Then spawn the mode components (`TmuxSession` /
  `TmuxWindow` + `ActiveWindow`, or an `OzmaTerminal` / `KeyboardFocused`
  carrying a `TerminalTitle`), run the schedule, and assert the primary
  window's `.title`. The `App`-construct + component-spawn + schedule-run
  structure mirrors `src/tmux/window_bar.rs`, but adds the `PrimaryWindow`
  those UI tests don't need.

## Out of scope

- Configurable title templates (tmux `set-titles-string`-style format).
- Reflecting transient UI states (copy-mode, dialogs, picker) in the title.
- Per-pane / per-window OS-level subtitles.
