# Phase 4 — Session UX + keybind mirror (tmux migration)

Design spec — 2026-06-15
Parent spec: `docs/superpowers/specs/2026-06-14-tmux-multiplexer-migration-design.md`
Phases 0–3 landed in commit `398239e` (#126).

## Goal

Deliver the "Session UX" slice of the tmux migration on top of the
already-projecting tmux backend (`crates/tmux_session`, `ozmux_tmux`):

1. A **session chooser popup** (choose-tree-style) that lets the user switch
   the attached tmux session while keeping the live control connection.
2. **Connection recovery**: a detached/error overlay plus reconnect, with the
   chooser doubling as the reconnect entry point.
3. A **keybind mirror**: parse tmux's `list-keys` into an in-memory model for
   later display (cosmetic; off the critical path).

The app stays runnable at the phase boundary, alongside the old multiplexer,
exactly as the parent spec sequences it.

## Foundational decisions (settled during brainstorming)

These override the one-paragraph Phase 4 summary in the parent spec where they
differ; they are fixed for this slice:

1. **Session switching uses `switch-client`, not detach+reattach.** The control
   connection (`TmuxConnection`'s live `TmuxClient`) stays alive across a
   switch; tmux re-emits the new session's state and the projection rebuilds
   from it. This is faster and lower-risk than tearing down and re-spawning the
   `tmux -CC` process.
2. **No standalone detach command.** The parent spec's "detach shortcut" is
   dropped. Session management is centered on the chooser. Detach as a concept
   only surfaces as *unexpected* connection loss (`%exit` / transport `Closed`),
   which is already handled by the existing `TmuxConnectionReset` teardown.
3. **The chooser is rendered by ozmux, not tmux.** tmux control mode does **not**
   stream window-mode rendering (choose-tree / copy-mode) to a control client —
   see "Verified constraint" below. So ozmux draws its own choose-tree-style
   popup in Bevy UI and drives the switch via `switch-client`.
4. **The chooser is opened by a dedicated GUI chord, not by intercepting tmux's
   prefix.** The existing `Cmd+Shift+P` chord (already wired to open the picker)
   opens the chooser. ozmux does **not** track tmux's prefix state or intercept
   `prefix + s`. Consequently the keybind mirror stays **off the critical path**
   (cosmetic / display-only), as the parent spec intended.
5. **Only choose-tree (session switching) is reimplemented this phase.** Other
   window modes (copy-mode, etc.) remain deferred exactly as the parent spec
   lists them; entering one still produces a blank/limited pane (the known
   control-mode limitation), to be addressed in a later phase.
6. **The keybind mirror prefers `list-keys -F` (tmux ≥ 3.7) and falls back to
   parsing the human-readable `bind-key` output on older tmux**, because `-F`
   was only added to `list-keys` in 3.7 (see "Verified constraints"). It does
   not block any other Phase 4 feature.

## Verified constraints (checked against a live tmux 3.6b)

These were confirmed empirically during brainstorming and are the basis for
decisions 3 and 6. They must be re-confirmed by the gated integration tests.

- **Window modes do not render through `%output` in control mode.** Sending
  `choose-tree` to a `tmux -CC` client (pty-backed) produced only:
  `%pane-mode-changed %0`, a single minimal `%output` (a status redraw, not the
  tree), and `%window-renamed @0 [tmux]`. The session tree itself was never
  streamed, even when the client was held attached for several seconds. This is
  consistent with how iTerm2's tmux integration implements its own mode UIs, and
  with the parent spec already deferring copy-mode as "runs as normal pane
  output / first cut." → ozmux must render the chooser itself (decision 3).
- **`list-keys -F` is version-dependent.** On tmux 3.6b, `tmux list-keys -F
  '<fmt>'` fails with `unknown flag -F`; the custom `-F` format (and `-O`
  sorting) were added to `list-keys` in tmux 3.7 (CHANGES 3.6b→3.7, issue 4845).
  So on tmux ≥ 3.7 the mirror SHOULD use `list-keys -F` with a tab-separated
  format (matching `LIST_WINDOWS_FORMAT` discipline); on < 3.7 it parses the
  default `bind-key [-r] [-N ...] [-n] -T <table> <key> <command...>` line
  output. → the mirror prefers `-F` when available, line-parses otherwise
  (decision 6).
- **`list-windows -a -F` enumerates windows across all sessions** with
  `#{session_id}` / `#{session_name}` available, so the chooser's tree can be
  built from one `list-sessions` + one `list-windows -a` round-trip.
- **`switch-client` exists** and is the command to repoint the control client at
  another session.

## Architecture

All tmux-facing logic stays inside `crates/tmux_session` (`ozmux_tmux`); the
binary's `src/` modules consume its resources/events/components and own the
Bevy UI. No new dependency on tmux internals leaks past `ozmux_tmux`.

### A. Session chooser (choose-tree-style popup)

**Trigger & state.** The existing `Cmd+Shift+P` GUI chord (handled in
`src/tmux_input.rs::gui_chord`) sets `picker.open = true`. The current flat
`SessionPicker` (`src/tmux_picker.rs`) is generalized into a session→window
**tree** chooser.

**Data load (on open).** When the chooser transitions closed→open, ozmux issues
two commands through the live client (or, while disconnected, through a one-shot
`TmuxServer` query as today):

- `list-sessions -F <fmt>` → existing `SessionInfo { id, name, windows,
  attached, created }`.
- `list-windows -a -F <fmt>` → a new per-row parser yielding
  `WindowEntry { session_id, session_name, window_id, window_index,
  window_active, window_name }`. The format is tab-separated with the free-text
  `window_name` LAST (same discipline as `LIST_WINDOWS_FORMAT`), double-quoted so
  the embedded tabs survive tmux's control-mode tokenizer.

These extend the existing `SessionPicker` resource (rather than adding a second
resource): it already owns `selected` / `open` and a working flatten+clamp
navigation, so it gains the per-session window list (e.g. a single
`rows: Vec<Row>` where `Row` is `Session | Window | NewSession`). Keeping one
resource preserves the single `resource_exists_and_changed::<SessionPicker>`
refresh gate. Rendering shows all sessions expanded (choose-tree's default), one
session-header row plus one indented row per window.

**Navigation.** The tree is flattened to an ordered list of *selectable rows*
(every session header is selectable; every window row is selectable). `ArrowUp`
/ `ArrowDown` move the selection with clamping (reuse `step_selection`). `Enter`
activates. A trailing "+ New session" row is kept (as today).

**Selection semantics.**

| Connection state | Row type | Action |
|---|---|---|
| `Attached` | session header | `switch-client -t <name>` |
| `Attached` | window row | `switch-client -t <name>` then `select-window -t @<id>` |
| `Attached` | "+ New session" | create a session and switch to it (see below) |
| not attached (`Idle`/`Detached`/`Error`) | session header / window | attach to that session via the existing `attach_or_create` boot path (= reconnect) |
| not attached | "+ New session" | `attach_or_create(CreateNew)` (existing) |

"New session while attached" is created and switched in one step using a
correlated reply: `new-session -d -P -F '#{session_name}'` returns the new
session's name, which is then fed to `switch-client -t <name>`. (Exact form
verified by integration test; if `-P -F` proves unreliable in control mode, fall
back to `new-session` + the `%sessions-changed` it triggers.)

**New command builders** (in `crates/tmux_session/src/enumerate.rs`, the
escaping-disciplined home for high-risk commands per the parent spec):

- `switch_client_command(name: &str) -> String` → `switch-client -t <quoted>`.
- `new_session_command() -> String` → `new-session -d -P -F '#{session_name}'`.
- `list_windows_all_command() -> String` → `list-windows -a -F "<fmt>"`.
- `select_window_command` already exists and is reused.

### Session-change reconciliation (the key reducer addition)

`switch-client` makes tmux emit `%session-changed $id <name>`. Today
`on_session_changed` (`observers.rs`) only updates the session entity's id/name;
it does **not** tear down the old session's windows/panes or re-enumerate. For a
real switch the projection must rebuild against the new session.

**Which notification fires.** For a `-CC` *control* client, a `switch-client`
may surface as `%client-session-changed <client> $id <name>` rather than (or in
addition to) `%session-changed`. The parser already models both
(`ControlEvent::ClientSessionChanged`), but
`event_pump.rs::trigger_notification` currently handles only `SessionChanged`
and drops `ClientSessionChanged`. The reducer MUST react to
`ClientSessionChanged` for the attached client; the integration test (below)
asserts which notification a real `tmux -CC` emits on `switch-client` and the
handler keys off that.

Behavior: when a `%session-changed` reports a session id **different** from the
currently-projected one, the reducer:

1. Despawns all projected `TmuxWindow`s (cascading to their `TmuxPane`s) and
   clears the window/pane indexes — the same teardown `on_connection_reset`
   performs, but **without** taking the connection or clearing the session
   entity.
2. Updates the session entity to the new id/name.
3. Re-runs the on-attach enumeration for the new session: `list-windows`
   (seed windows + layouts), `display-message` active-pane query. This reuses
   the exact enumeration path `drain_tmux_events` runs on the `Attached`
   transition.

Where this lives: `on_session_changed` reads the currently-projected session id
(via `TmuxProjection.session` → `TmuxSession.id`) and compares it to the event's
id, so the id-change decision stays in one observer. The windows/panes teardown
reuses existing machinery rather than a new code path: triggering
`TmuxWindowsRetained { windows: vec![] }` despawns every window (cascading to
panes) while leaving the session entity intact — exactly the "reset
windows/panes only" behavior, with no second teardown loop to drift from
`on_connection_reset`. Re-sending the enumeration commands still needs the live
client (`NonSend`), so that part runs in `event_pump.rs` on a detected switch.
The first `%session-changed`/`%client-session-changed` after attach (id matches
the freshly-attached session, or none projected yet) must **not** trigger a
teardown — only an actual id change does.

### B. Connection recovery

**No detach chord.** Removed from scope (decision 2).

**Detached overlay (extend `tmux_dialog.rs`, no new plugin).** Rather than a
second near-duplicate overlay plugin, extend the existing
`src/ui/tmux_dialog.rs` — which already owns a full-screen backdrop,
`GlobalZIndex`, the `Display::None`/`Flex` toggle, and the
`resource_exists_and_changed::<ConnectionState>` gate — to render state-specific
text: `Detached` → "Disconnected — press ⌘⇧P to choose a session";
`Error { reason }` → "tmux unavailable\n{reason}" (unchanged). Because `Detached`
and `Error` are mutually-exclusive `ConnectionState` variants, only one is ever
active, so a single overlay with one z-index suffices — no separate plugin and
**no "ever-attached" marker**. The existing `reason` strings already distinguish
boot failure ("tmux unavailable: …") from a connect failure ("tmux connect
failed: …"), set in `tmux_picker.rs`.

**Reconnect = open the chooser while disconnected.** No separate reconnect
chord. Opening the chooser (`Cmd+Shift+P`) while `Idle`/`Detached`/`Error`
already runs the `attach_or_create` boot path on the selected session, which is
exactly a reconnect. The overlay's hint points the user at that chord.

**Unexpected loss.** Teardown is currently driven **only** by
`TransportEvent::Closed` (`next_state` → `Detached`, then `drain_tmux_events` →
`connection.take()` + `TmuxConnectionReset`). The control `%exit` notification
(`ControlEvent::Exit`) is parsed but **ignored** by `trigger_notification`
today, so teardown relies on the pty EOF/`Closed` that follows `%exit`. Phase 4
adds the overlay that surfaces `Detached`; if the integration tests show `%exit`
can arrive without a timely `Closed`, add an explicit `%exit` → teardown path
then. Do not claim `%exit` already drives teardown — it does not.

### C. Keybind mirror (parse + resource only)

Cosmetic, off the critical path (decision 4). No UI in this phase.

- **Command**: `list_keys_command() -> String` → `list-keys` (no `-F`; tmux
  rejects it). Sent once on the `Attached` transition, alongside the existing
  `list-windows` / client-name / active-pane queries in `drain_tmux_events`.
- **Parser**: a pure `parse_key_bindings(lines: &[String]) -> Vec<KeyBinding>`
  in a new `crates/tmux_session/src/keybinds.rs`. Each line is
  `bind-key [-r] [-N ...] [-n] -T <table> <key> <command...>`. The parser skips
  the leading `bind-key`, collects/ignores option flags, reads `-T <table>`
  (treating `-n` as `table = "root"`), takes the next token as the `key` chord
  (verbatim, including tmux's own escaping like `\;` `\"` `\#`), and keeps the
  remainder of the line (trimmed) as the `command` string. Fixtures come from
  real `list-keys` output (copy-mode, prefix, root tables).
- **Model**: `KeyBinding { table: String, key: String, command: String }` and a
  `TmuxKeyBindings(Vec<KeyBinding>)` resource. The reply is correlated by
  `CommandId` via a new field in `EnumerationState`
  (`list_keys_pending: Option<CommandId>`), parsed in `event_pump.rs`, and
  stored into the resource. `KeyBinding` / `TmuxKeyBindings` stay `pub(crate)`
  (no `lib.rs` re-export) until a display UI actually consumes them — the mirror
  has no consumer outside the crate this phase.
- **Re-sync**: on attach only. Re-sync on `%config-error` / manual reload is a
  follow-up (the parser does not yet surface `%config-error`).

## State & data-flow summary

```
Cmd+Shift+P ─▶ picker.open = true
  └▶ on open: list-sessions + list-windows -a ─▶ SessionPicker rows ─▶ tree UI

Enter on row:
  Attached      ─▶ switch-client (+ select-window) ─▶ %(client-)session-changed(new id)
                     └▶ teardown windows/panes + re-enumerate ─▶ projection rebuilt
  Disconnected  ─▶ attach_or_create ─▶ Connecting ─▶ Attached (existing flow)

%exit / Closed  ─▶ Detached + TmuxConnectionReset ─▶ tmux_dialog overlay (Detached text)
Attached (once) ─▶ send list-keys ─▶ parse_key_bindings ─▶ TmuxKeyBindings (no UI)
```

`ConnectionState` continues to gate overlays via
`resource_exists_and_changed::<ConnectionState>` run conditions (no in-body
change checks), per the repo's system-optimization rule.

## Affected files

**`crates/tmux_session/src/`**
- `enumerate.rs` — new builders (`switch_client_command`, `new_session_command`,
  `list_windows_all_command`, `list_keys_command`), the `WindowEntry` row parser
  for `list-windows -a`, and a `list_keys_pending` field on `EnumerationState`.
- `observers.rs` — a "reset windows/panes only" path; `on_session_changed`
  gains the id-changed teardown trigger.
- `event_pump.rs` — detect a real session switch (new id ≠ projected id), drive
  teardown + re-enumeration; correlate and parse the `list-keys` reply.
- `plugin.rs` — send `list-keys` on the `Attached` transition.
- `keybinds.rs` (new) — `KeyBinding`, `TmuxKeyBindings`, `parse_key_bindings`,
  all `pub(crate)` (no `lib.rs` re-export this phase).
- `lib.rs` — re-export only the new public command builders the binary calls
  (`switch_client_command`, `new_session_command`, `list_windows_all_command`);
  the keybind-mirror types stay crate-private.

**`src/`**
- `tmux_picker.rs` — tree chooser: refresh on the closed→open transition
  (`list-sessions` + `list-windows -a`), extended `SessionPicker` rows model,
  tree rendering + flattened-row navigation, `switch-client` / `select-window`
  on select while attached, unchanged boot/reconnect path while disconnected.
- `ui/tmux_dialog.rs` — extend to render state-specific text: `Detached` →
  reconnect hint, `Error { reason }` → existing message (single overlay, no new
  plugin, no "ever-attached" marker).
- `tmux_input.rs` — unchanged trigger (`Cmd+Shift+P` already opens the picker);
  touched only if the chooser needs a richer open signal.

## Testing strategy

Mirrors the parent spec's strategy (pure reducer/parser bulk + a few gated
real-tmux integration tests).

- **Pure (no tmux):**
  - Command builders: `switch_client_command` (quoting), `new_session_command`,
    `list_windows_all_command`, `list_keys_command`.
  - `list-windows -a` row parser: session/window fields, name-with-tabs-last,
    malformed-row error.
  - `parse_key_bindings`: copy-mode / prefix / root fixtures, `-n` → root, `-r`
    flag ignored, command-with-spaces/braces kept verbatim, escaped keys.
  - Session-change reducer: synthetic `%session-changed` / `%client-session-changed`
    with a **new** id tears down old windows/panes and re-enumerates; one with
    the **same** id does not tear down.
  - MRU selection (`select_attach_target`) already covered; extend if reconnect
    selection differs.
- **Bevy systems (headless `App`):** chooser open triggers a refresh; selecting a
  row while `Attached` emits the `switch-client` command; the `tmux_dialog`
  overlay renders the reconnect hint on `Detached` and the error message on
  `Error`.
- **Integration (real `tmux -CC`, gated like `tests/real_tmux_*.rs`):**
  - `switch-client` between two sessions → projection rebuilds (old panes gone,
    new panes spawned, active pane set).
  - `list-windows -a` tree enumeration parses across sessions.
  - `list-keys` reply parses into `TmuxKeyBindings`.
  - Killing the server / `%exit` → `Detached` + projection torn down + overlay.
  - "New session while attached" creates and switches (validates the
    `new-session -d -P -F` + `switch-client` form).

## Deferred (unchanged from parent spec, restated for this slice)

- Copy-mode and all non-choose-tree window modes (decision 5).
- Keybind-mirror **display UI** (status hints / menus) and re-sync on
  `%config-error` / reload.
- Prefix-key interception / full tmux-keybinding inheritance for GUI actions.
- Tree collapse/expand, mouse interaction in the chooser, and a multi-window/tab
  strip UI.
- The parent spec's other deferred items (inline webviews under tmux, mouse
  wheel/drag-resize, IME revalidation, hyperlink hover).
