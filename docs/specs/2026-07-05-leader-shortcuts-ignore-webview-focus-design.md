# Leader shortcuts fire regardless of webview keyboard focus

Date: 2026-07-05
Status: design approved, pending spec review

## Goal

Make `<Leader>`-scoped shortcuts (the prefix table) reachable while a CEF
webview owns the keyboard, exactly as they are when a terminal owns it. Today,
once a webview is focused, the keyboard decider short-circuits and the leader is
dead: `<Leader>n` (new window), `<Leader>c` (copy-mode), etc. never fire. After
this change the leader behaves like tmux's prefix — a global escape hatch that
works no matter what currently holds keyboard focus.

The leader's claimed keystrokes must NOT also leak into the focused web page.
`bevy_cef` delivers every keystroke to the focused webview, so firing a leader
action without suppression would both run the action and type its key into the
web content (`<Leader>n` → new window AND a stray `n` in a web input). The design
therefore has two coupled parts: **(1)** let the leader run during webview focus,
and **(2)** withhold the leader-claimed keys from CEF via `CefKeyboardFilter`.

## Background

### Where the leader dies today

Keyboard dispatch is a single mode-agnostic pipeline (PR #239):
`resolve_shortcuts` (`src/input/dispatch.rs`) reads `KeyboardInput`, calls the
pure decider `classify_key_batch` (`src/input/resolve.rs`), and emits a
`ShortcutBatch` the per-mode appliers (`apply_default_shortcuts`,
`apply_tmux_shortcuts`) consume. The leader state machine (`LeaderPhase`,
`step_leader`) lives in `src/input/shortcuts.rs`; the default leader is a Cmd
tap detected by `detect_modifier_tap`, focus-independently, in `LeaderGate::Detect`.

`classify_key_batch` has a `ctx.webview_focused` branch that short-circuits every
pressed key (`src/input/resolve.rs:92`):

```rust
if ctx.webview_focused {
    *leader_phase = LeaderPhase::Idle;            // reset the leader every key
    if shortcuts.is_release_webview_focus(..) { ReleaseWebviewFocus }
    else if forward_chord_matches(..)         { WebviewForward }
    continue;                                     // everything else swallowed
}
```

So while a webview is focused the leader is forced to `Idle` on every keystroke;
only the release chord and the webview's declared `forward_keys` chords are
handled. The prefix table is never consulted.

`KeyboardInput` events DO reach the decider during webview focus — that is how
the release and forward branches above match at all. So making the leader fire is
purely a decider question; no new event plumbing is needed.

### Why suppression is required (`bevy_cef` delivers to the focused webview)

`bevy_cef` 0.11's `send_key_event` (in its `KeyboardDeliverSet`) delivers every
`KeyboardInput` to the entity in `FocusedWebview`, and ozmux does not currently
populate `bevy_cef`'s opt-out filter. bevy_cef exposes exactly the hook we need:

```rust
// bevy_cef::prelude
pub struct CefKeyboardFilter { /* (Entity, KeyCode, ModifiersState) triples */ }
impl CefKeyboardFilter {
    pub fn set(&mut self, entries: impl IntoIterator<Item=(Entity, KeyCode, ModifiersState)>);
    pub fn contains(&self, webview: Entity, code: KeyCode, mods: ModifiersState) -> bool;
}
pub struct KeyboardDeliverSet;   // order filter population `.before(KeyboardDeliverSet)`
pub struct ModifiersState { pub alt: bool, pub ctrl: bool, pub shift: bool, pub logo: bool }
```

`send_key_event` skips any key whose `(webview, code, mods)` triple is in the
filter. An embedder fills the filter each frame `.before(KeyboardDeliverSet)`;
`set()` replaces the whole set, so it self-clears when nothing is claimed.

## Non-goals

- **Direct GUI chords are out of scope.** Only the leader/prefix table gains the
  focus-independent behavior. Cmd+Q (Quit), Cmd+S (copy-mode), Cmd+V (paste) and
  every other direct chord keep their current webview-focus behavior (they remain
  webview-owned while a webview is focused).
- **No change to `ReleaseWebviewFocus` or `forward_keys` semantics**, nor to the
  tmux `WebviewForward` → `send-keys` behavior. Those keys keep reaching CEF as
  they do today; only leader-claimed keys are newly suppressed. One narrow
  overlap: if a chord is BOTH the configured leader and a webview `forward_keys`
  chord, the leader wins (its `Swallow` arm precedes the forward arm) and that
  chord is suppressed from CEF rather than forwarded.
- **The webview is never blurred by the leader.** The leader borrows keystrokes
  transiently; keyboard focus stays on the webview. Only the explicit
  `ReleaseWebviewFocus` chord blurs it.
- No config surface changes. The existing leader (Cmd tap by default, or a
  configured chord/tap) is what activates during webview focus.

## Design

Approach: extend the pure decider to compute both the effects and the
leader-claimed keys in one pass, and let `resolve_shortcuts` forward the claims
to `CefKeyboardFilter`. Single source of truth (the decider), no logic
duplication, and guard-consistency is structural (claims are computed in the same
body, after the same coarse guards, as the effects). Alternatives that keep the
decider signature unchanged were considered and rejected (see below).

### Part 1 — let the leader run during webview focus (`src/input/resolve.rs`)

Rewrite the `ctx.webview_focused` branch of `classify_key_batch` to step the
leader machine instead of resetting it, then fall back to webview handling only
for keys the leader does not claim:

```rust
if ctx.webview_focused {
    match step_with_repeat(leader_phase, shortcuts, ev, ctx.mods, ctx.now) {
        LeaderStep::Swallow => {
            // the leader itself, or an abandoned second key — claimed, no effect
            webview_suppressed.push(ev.key_code);
            continue;
        }
        LeaderStep::RunAction(action) => {
            webview_suppressed.push(ev.key_code);
            effects.push(KeyEffect::Action { action, via_leader: true });
            continue;
        }
        LeaderStep::Passthrough => {
            // not leader-related — webview owns this key (unchanged behavior)
            if shortcuts.is_release_webview_focus(ev.key_code, ctx.mods) {
                effects.push(KeyEffect::ReleaseWebviewFocus);
            } else if ctx.forward_chords.iter()
                .any(|chord| chord_matches(chord, ev.key_code, ctx.mods))
            {
                effects.push(KeyEffect::WebviewForward {
                    logical: ev.logical_key.clone(),
                    key_code: ev.key_code,
                });
            }
            continue;
        }
    }
}
```

Consequences of using `step_with_repeat` (the same function the terminal path
uses) instead of the unconditional `Idle` reset:

- The Cmd-tap leader already engages via `detect_modifier_tap` during webview
  focus (it is focus-independent). On the tap-release frame `LeaderPhase` becomes
  `Pending`; the second-key frame resolves it here against the prefix table.
- A configured **chord leader** (e.g. `Ctrl+B`) now engages here too: pressing it
  during webview focus returns `Swallow` and sets `Pending` (and is suppressed
  from CEF — Part 2), rather than reaching the web page.
- Normal typing into a webview is unchanged: `Idle` + a non-leader key returns
  `Passthrough`, is not suppressed, and (as today) is swallowed by ozmux while
  bevy_cef delivers it to the page.
- The old NOTE about resetting to prevent a "stale leader firing when focus
  returns" is obsolete: the machine is now stepped, and the existing
  `reset_leader_phase` (gated on `FocusedWebview` change, `src/input/shortcuts.rs`)
  still clears any pending leader on click-in / click-out focus transitions.

Return type: `classify_key_batch` currently returns `Vec<KeyEffect>`. It returns
a small struct carrying both outputs:

```rust
pub(crate) struct ClassifiedKeys {
    pub effects: Vec<KeyEffect>,
    /// Physical keys the leader claimed while a webview was focused, to withhold
    /// from CEF. The frame's modifier snapshot (`ctx.mods`, constant across the
    /// batch) is applied by `resolve_shortcuts`. Empty on the non-webview path.
    pub webview_suppressed: Vec<KeyCode>,
}
```

`webview_suppressed` is populated only inside the `ctx.webview_focused` branch
(the `Swallow` / `RunAction` arms). `KeyEffect` itself is unchanged. The sole
production caller is `resolve_shortcuts`; the resolve.rs test helper `run` and
the tmux/default appliers are unaffected (they consume `ShortcutBatch`, not the
decider's return).

### Part 2 — withhold the claimed keys from CEF (`src/input/dispatch.rs`)

`resolve_shortcuts` maps `webview_suppressed` onto `CefKeyboardFilter`:

- Add `mut cef_filter: ResMut<CefKeyboardFilter>` to the system.
- Snapshot the focused webview entity into a local **before** the effect loop
  that handles `ReleaseWebviewFocus` (which sets `focused_webview.0 = None`,
  `src/input/dispatch.rs:175`). Building the filter from `focused_webview.0` after
  the loop would read `None` on a frame carrying both a leader claim and a release
  chord, silently dropping the suppression.
- After classifying, when a webview is focused, `filter.set(...)` with
  `(webview_entity, key_code, ms)` for each claimed `key_code`, where `ms` is the
  frame's `Modifiers { ctrl, shift, alt, meta }` mapped once to
  `ModifiersState { alt, ctrl, shift, logo: meta }`. When `webview_suppressed` is
  empty (non-webview path, or webview focused but nothing claimed), `set([])`
  clears it.
- On the coarse-guard early return (tmux modal prompt / IME composing / unfocused
  window — `src/input/dispatch.rs:137`), also `cef_filter.set([])` before
  returning, so a stale claim never lingers on a guarded frame.

The `ModifiersState` computed here matches what `send_key_event` derives at
delivery: both read the live `ButtonInput<KeyCode>` in the same frame.

### Ordering and registration (`src/input/dispatch.rs`)

`resolve_shortcuts` must write the filter before bevy_cef reads it. The claims are
computed in the same `classify_key_batch` pass that decides the effects — each
key's suppression is recorded at the exact phase state that decides its effect —
so no separate `LeaderPhase` snapshot is needed (`send_key_event` never reads
`LeaderPhase`; it reads the filter plus the live `ButtonInput`). This holds
because `resolve_shortcuts` IS the sole phase-advancing system and already owns
the classification:

- Add `.before(KeyboardDeliverSet)` to `resolve_shortcuts` in `DispatchPlugin`
  (import `KeyboardDeliverSet` from `bevy_cef::prelude`). This is the only new
  ordering edge; bevy_cef's delivery systems have no dependency on ozmux input
  sets, so no cycle is introduced.
- Existing set membership (`InputPhase::FocusedKey`, `ShortcutSet::Resolve`,
  `LeaderGate::Advance`, `run_if(on_message::<KeyboardInput>)`) is unchanged.

`CefKeyboardFilter` is `init_resource`d by bevy_cef's `KeyboardPlugin` (wired via
`OzmaWebviewPlugin` / `cef_plugin`), so it exists at runtime; tests init it
explicitly.

## Components and data flow

```
KeyboardInput (reaches Bevy even while a webview is focused)
   │
   ├─ detect_modifier_tap  (LeaderGate::Detect)  → LeaderPhase::Pending on Cmd tap
   │
   ▼
resolve_shortcuts  (InputPhase::FocusedKey, ShortcutSet::Resolve,
   │                LeaderGate::Advance, .before(KeyboardDeliverSet))
   │   classify_key_batch(...) → ClassifiedKeys { effects, webview_suppressed }
   │        webview_focused branch:
   │            Swallow / RunAction → claim key + (RunAction) push Action{via_leader}
   │            Passthrough         → release / forward / swallow (unchanged)
   │   ├─ ShortcutBatch{effects, focused, ..}  → per-mode appliers (unchanged)
   │   └─ CefKeyboardFilter.set(webview_suppressed)  ─┐
   ▼                                                  │
bevy_cef KeyboardDeliverSet::send_key_event  ◄────────┘  skips filtered keys
        delivers non-filtered keys to FocusedWebview
```

- Leader actions target `batch.focused`, the current `KeyboardFocused` surface —
  always the active pane, resolved independently of `FocusedWebview`
  (`resolve_shortcuts`, `Query<Entity, With<KeyboardFocused>>`). It is valid
  during webview focus: a webview-surface pane carries `KeyboardFocused` itself,
  and a freshly-focused inline child's parent pane is the active
  (`KeyboardFocused`) pane (`sync_focused_webview`, `src/input/focus.rs`). Note
  that inline webview focus is *preserved* for a child of any live surface —
  active or not — so `FocusedWebview` and `KeyboardFocused` can diverge once the
  leader is allowed to run under webview focus (see Risks). Paste targets the
  terminal, not the page (intended).
- The appliers already handle `KeyEffect::Action { via_leader: true }` for every
  leader action and are not gated on webview focus; no applier change is needed.

## Testing

Pure decider tests (`src/input/resolve.rs`, no `App`):

- webview focused + `Pending` + bound second key → one `Action{via_leader:true}`;
  `webview_suppressed` contains that key.
- webview focused + `Idle` + configured chord-leader press → `Swallow`, phase
  `Pending`, `webview_suppressed` contains the leader chord.
- webview focused + `Pending` + unbound second key → `Swallow`, no `Action`,
  key still in `webview_suppressed`.
- webview focused + `Idle` + plain non-leader key → no effect (or `WebviewForward`
  when a forward chord), phase `Idle`, `webview_suppressed` empty (Passthrough is
  not suppressed).
- Existing forward-chord and release-chord webview tests still pass; update
  `webview_focus_clears_leader_and_forwards` for the new stepped behavior.

Applier / ordering tests (`src/input/dispatch.rs`, `App`):

- After a leader claim during webview focus, `CefKeyboardFilter.contains(webview,
  code, ms)` is true for the claimed key.
- A guarded frame (unfocused window) clears the filter and emits no batch.
- A frame with nothing claimed (no webview, or plain webview typing) leaves the
  filter empty.
- `resolve_shortcuts` is ordered `.before(KeyboardDeliverSet)` (registration
  mirrors production), and the input `App` builds with that edge added without a
  schedule-graph cycle / ambiguity panic.

## Behaviour-preservation checklist

- Non-webview keyboard dispatch (terminal / tmux pane) is byte-for-byte
  unchanged: the new struct field is empty and the filter is cleared.
- Webview typing with no leader engaged is unchanged: keys pass through to CEF;
  ozmux emits nothing.
- `ReleaseWebviewFocus` and `forward_keys` continue to reach CEF and behave as
  before.
- Direct GUI chords keep their current webview-focus behavior (webview-owned).

## Risks and staging

- **Cmd-tap over a web app.** A bare Cmd tap now arms the leader even while a
  webview is focused, so the next keystroke is consumed as the leader's second
  key. This is the existing tap semantics extended to webview focus, and matches
  the "leader works regardless of focus" goal; a bare Cmd is harmless to CEF
  (no character, not an edit command). The same holds for a chord leader: the
  chord's bare modifier(s) (e.g. `Ctrl` in `Ctrl+B`) are not claimed by
  `step_leader` and still reach CEF — harmless for the same reason; only the
  chord's main key is suppressed. Acceptable.
- **Key releases are not suppressed.** The decider claims only pressed keys, but
  bevy_cef's `send_key_event` delivers key-up events too, so a leader-claimed
  key's release still reaches the page (the release usually lands a frame later,
  when `LeaderPhase` is already `Idle`). A key-up inserts no text — text is
  committed on keydown / `beforeinput` — so this is visually harmless for web
  inputs; a page with a `keyup` listener would observe a lone key-up. Suppressing
  releases would require cross-frame state (remember each claimed key-code until
  its release); deferred as a known limitation.
- **`batch.focused` can drift from the pane behind the webview.** Because the
  leader may now run while a webview stays focused (and is never blurred by it),
  a user can `<Leader>`-navigate panes while `FocusedWebview` is preserved,
  moving `KeyboardFocused` (= `batch.focused`) to a pane other than the one
  hosting the focused webview. This is intended and consistent with the
  non-webview case: leader pane/split/window actions always target the active
  (`KeyboardFocused`) pane. Key-leak suppression is unaffected — it targets the
  snapshotted `focused_webview.0`, not `batch.focused`.
- **Ordering assumption.** Correctness of same-frame suppression depends on
  `resolve_shortcuts` running before `KeyboardDeliverSet`. Enforced by the
  explicit `.before` edge and covered by a registration test.
- Staging: Part 1 and Part 2 land together — Part 1 without Part 2 would leak the
  claimed key into the page.

## Alternatives considered

- **Blur the webview when the leader engages.** Clearing `FocusedWebview` on
  engage would route keys to the terminal path and need no CEF filter, but it
  destroys the web page's focus/caret on every leader use and complicates the
  first-key timing. Rejected — too invasive to focus state.
- **Separate suppression system, decider signature unchanged.** A pre-delivery
  system could populate the filter without touching `classify_key_batch`'s return
  type, but the `Swallow` vs `Passthrough`-swallow ambiguity means it must
  reproduce the leader claim decision — either by re-running `step_leader` on a
  cloned phase (precise, but the machine runs twice and the coarse guards must be
  mirrored to stay consistent) or by a phase→suppress heuristic (imprecise at the
  Repeat-window and multi-key-per-frame edges, and duplicative). The chosen
  approach computes claims in the same pass as effects, keeping one source of
  truth and structural guard-consistency.

## Out of scope

- Direct GUI chords bypassing webview focus (a separate product decision).
- Any change to `forward_keys` / `ReleaseWebviewFocus` routing or the tmux
  `WebviewForward` → `send-keys` mapping.
- Config surface for enabling/disabling per-webview leader behavior.
