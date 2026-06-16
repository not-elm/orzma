# tmux inline-webview mouse routing & focus — design

Date: 2026-06-16
Branch: `webview-focus`
Status: design (approved through brainstorming; pending spec review)

## Problem

Under the tmux backend, the mouse wheel **unconditionally enters tmux copy
mode**, which makes an inline webview embedded in a tmux pane effectively
**uninteractable with the mouse** (it cannot be scrolled, clicked, or
hovered).

Root cause, verified against the code:

- The tmux wheel path `forward_wheel_to_tmux` (`src/tmux_input.rs`) is
  **position-blind**. It aggregates the frame's wheel first, then early-returns
  only when `focused_webview.0.is_some()` regardless of pointer position
  (`src/tmux_input.rs:415`). When `FocusedWebview` is `None`, a scroll-up runs
  the tmux `WheelUpPane` binding (`copy-mode -e`), which enters copy mode and
  inserts `CopyModeState` (`src/tmux_input.rs:451`, `:329`).
- `sync_focused_webview` (`src/webview_render.rs:93`) drives `FocusedWebview`
  from the **old multiplexer's** active surface every frame. The tmux backend
  does not populate the old multiplexer (production bootstrap no longer seeds an
  old workspace — `src/bootstrap.rs`), so per the comment at
  `src/tmux_input.rs:164-168`, `FocusedWebview` is "usually `None`" under tmux.
  A click-granted focus is clobbered back to `None` on the next frame's sync.
- Net effect under tmux: wheel-over-webview always falls through to copy-mode
  entry; an inline webview can never hold focus, so its CEF browser never
  receives mouse input.

### Decisive technical constraint (bevy_cef is focus-gated)

`bevy_cef`'s CEF input delivery is gated on a focused frame. All three input
calls funnel through `get_focused_browser`, which requires
`focused_frame().is_some()`:

```rust
// bevy_cef/crates/bevy_cef_core/src/browser_process/browsers.rs
pub fn send_mouse_wheel(&self, webview: &Entity, position: Vec2, delta: Vec2) {
    if let Some(browser) = self.get_focused_browser(webview) { ... }   // :265
}
pub fn send_mouse_move<'a>(&self, webview, buttons, position, mouse_leave) {
    if let Some(browser) = self.get_focused_browser(webview) { ... }   // :213
}
pub fn send_mouse_click(&self, ...) {
    if let Some(browser) = self.get_focused_browser(webview) { ... }   // :232
    // NOTE: the internal `browser.host.set_focus(true)` runs INSIDE this gate,
    // so it CANNOT bootstrap focus on a not-yet-focused browser.
}
fn get_focused_browser(&self, webview) -> Option<&WebviewBrowser> {
    self.browsers.get(webview)
        .and_then(|b| b.client.focused_frame().is_some().then_some(b))  // :641
}
pub fn set_focus(&self, webview: &Entity, focused: bool) { ... }        // :289
// ^ the ONLY ungated path (a direct lookup, NOT get_focused_browser); the only
//   call that can GRANT focus to a browser with no focused frame yet.
```

Consequence 1: **a pure hover model (deliver wheel to an unfocused webview) is
impossible without modifying `bevy_cef`.** The interaction model must be
focus-first. This is also why the existing native path
(`resolve_inline_wheel_target` in `src/input/mouse_wheel.rs`) is gated on focus
**and** pointer position.

Consequence 2 (critical): focus must be granted by the **ungated**
`Browsers::set_focus(&child, true)` (`browsers.rs:289`) BEFORE the first click is
forwarded. `send_mouse_click` alone CANNOT establish focus — its body, including
its own internal `set_focus(true)`, sits behind the `get_focused_browser` gate,
so the very first click on a never-focused inline webview is silently dropped and
focus never bootstraps (the exact failure this design must fix). The native path
already uses the two-call idiom (`browsers.set_focus(&hit.child, true)` then
`send_mouse_click`, `src/input/mouse_buttons.rs:781-783`, rationale at
`:719-723`); the tmux path MUST replicate it. See the lifecycle table below.

## Decisions (locked during brainstorming)

1. **Backend scope:** tmux backend only. The native path
   (`src/input/mouse_wheel.rs`, `src/input/mouse_buttons.rs`) is left unchanged;
   it continues to serve old-multiplexer surfaces.
2. **Routing model:** **Focus-based + pointer-gated wheel.** Click grants
   keyboard/CEF focus; once focused, wheel/move route to the webview **only when
   the pointer is over its rect**. Pure hover-wheel (wheel without focus) is
   deferred to a future enhancement that would require patching `bevy_cef`.
3. **Focus grant:** a left press inside an interactive inline rect focuses the
   webview and forwards that same click to CEF (so links/buttons fire on one
   click).
4. **Focus release:** a left press on the terminal region (outside every inline
   rect) clears focus and proceeds as a normal tmux gesture; **plus** the
   existing `Ctrl+Shift+Esc` keyboard release.
5. **In-webview drag & hover are in scope:** text selection / slider drag inside
   the webview, and hover styling, are forwarded to CEF via `send_mouse_move`.

## Goals / non-goals

Goals:

- Make `FocusedWebview` tmux-aware so an inline webview in a tmux pane can hold
  focus.
- Route wheel, click (press/release), and move (hover/drag) to a focused inline
  webview when the pointer is over its rect, without breaking tmux copy-mode /
  drag-select on the terminal regions.

Non-goals:

- Pure hover-wheel (wheel to an unfocused webview). Deferred; needs a `bevy_cef`
  change.
- Touching the native (old-multiplexer) input path.
- Extracting a shared backend-agnostic inline-routing module (considered and
  rejected as over-scoped; see "Alternatives").

## Architecture

Reuse the already-verified geometry helpers — they run for **any**
`TerminalGrid`, and tmux panes carry `TerminalRenderBundle` (= `TerminalGrid`)
with inline webviews mounted as `ChildOf(TmuxPane entity)`
(`src/tmux_render.rs:86`). No helper changes are needed:

- `inline_hit_at`, `inline_local_dip`, `focused_inline_of`, and the
  `TerminalOverlays` projection (`src/inline_webview.rs`).
- CEF forwarding through `bevy_cef_core::Browsers` (`send_mouse_wheel`,
  `send_mouse_click`, `send_mouse_move`) — the same API the native path uses.

The work is three pieces, all tmux-local:

```
 MouseButtonInput ─► tmux_mouse.rs::arbiter
                       (new) inline left-click pre-step (mirrors native route_inline_left_click):
                        press in rect  → FocusedWebview = child; ungated set_focus(child);
                                         select-pane(host); send_mouse_click(down);
                                         record inline_press = Some(child);
                                         consume (do NOT arm divider/drag-select)
                        press off rect → clear FocusedWebview (if it held an inline child);
                                         fall through to existing divider/select-pane/drag
                        release        → if inline_press == Some(child): send_mouse_click(up)
                                         at the (drift-tolerant) release DIP; clear; consume
                       (modal guard: keep picker/copy-prompt drain+reset; the
                        focused_webview blanket-drain is REMOVED — inline routing owns it)

 (CursorMoved)    ─► tmux_mouse.rs::forward_tmux_inline_mouse_moves  (new)
                        on the latest CursorMoved, if the pointer is over an interactive
                        inline rect of the pane under it → send_mouse_move(child,
                        mouse_buttons.get_pressed(), dip). Stateless, event-driven, forwards
                        held buttons (so a press-drag selects). ONE system for hover AND
                        drag, exactly like native forward_inline_mouse_moves; bevy_cef gates
                        delivery on focus internally; modal: skip.

 MouseWheel       ─► tmux_input.rs::forward_wheel_to_tmux
                        (new) resolve_tmux_inline_wheel_target():
                          child = focused_inline_of(FocusedWebview, pane_under_pointer)
                          require pointer in child's rect (inline_hit_at)
                        if target: send_mouse_wheel(child, dip, raw_delta) per event;
                                   reset residual; return (tmux untouched)
                        else:      existing tmux path (copy-mode scroll / WheelUpPane)
                        # the blanket `focused_webview.is_some()` early-return is removed

 KeyboardInput    ─► tmux_input.rs::forward_keys_to_tmux  (unchanged)
                        focused_webview.is_some() → webview owns keys; Ctrl+Shift+Esc
                        releases. Already correct once FocusedWebview is tmux-aware.

 (per frame)      ─► webview_render.rs focus validator  (modified / tmux-aware)
                        keep FocusedWebview while it points at an inline child of a
                        live TmuxPane; GC to None when that child despawns / pane gone.
                        Does NOT drive focus from the active pane (focus is click-driven).
```

Files touched:

- `src/tmux_mouse.rs` — inline click pre-step in the arbiter (mirrors native
  `route_inline_left_click`); a new `inline_press: Option<Entity>` field on
  `TmuxMouseGesture` (mirrors native `MouseSelectionState.inline_press`) to
  route the release click-up to the same child; the modal guard keeps the
  picker/copy-prompt drain+reset but DROPS the `focused_webview` blanket-drain
  (inline routing now owns focus). New `CursorMoved`-driven
  `forward_tmux_inline_mouse_moves` system (hover + drag, stateless). Reuse the
  existing `pub(crate)` `pane_under_cursor` and `phys_to_terminal_local`.
- `src/tmux_input.rs` — `forward_wheel_to_tmux`: pointer-gated inline wheel
  target + per-event CEF forwarding (mirrors native `aggregate_wheel_delta`).
  `forward_keys_to_tmux` unchanged.
- `src/webview_render.rs` — make the focus source tmux-aware (preserve inline
  focus for tmux-pane children; GC on despawn). The native old-multiplexer
  preserve arm stays.
- Shared helpers in `src/inline_webview.rs`: **no change**, reused.

## State & focus lifecycle

The single source of focus truth remains the existing `FocusedWebview`
(bevy_cef resource) — "single focus source" (spec §7) extended to tmux. New
persistent state is minimal: one added `inline_press: Option<Entity>` field on
the existing `TmuxMouseGesture` resource (mirrors native
`MouseSelectionState.inline_press`), holding the in-flight press so the release
click-up routes to the same child even if the pointer drifted off the rect.
There is no per-frame drag-capture state — move forwarding is stateless (see
"Move").

| Trigger | Action |
| --- | --- |
| press inside an interactive inline rect | `FocusedWebview = Some(child)`; **ungated `browsers.set_focus(&child, true)` (`browsers.rs:289`)** to grant CEF focus; `select-pane` the host pane; `send_mouse_click(down)`; record `inline_press = Some(child)`. The explicit `set_focus` is mandatory — `send_mouse_click` cannot bootstrap focus on its own (see "Decisive technical constraint", Consequence 2) |
| press on terminal region (outside every rect) | `FocusedWebview = None` (→ CEF blur); proceed with the normal tmux gesture |
| `Ctrl+Shift+Esc` | `FocusedWebview = None` (existing keyboard release) |
| release while `inline_press == Some(child)` | `send_mouse_click(up)` at the drift-tolerant release DIP; clear `inline_press` |
| focused inline child despawns / host pane gone | focus validator GCs `FocusedWebview` to `None` |

CEF-focus consistency (critical invariant): setting `FocusedWebview = Some(child)`
alone does not establish CEF focus, and `send_mouse_click` cannot establish it
either (its body sits behind the `get_focused_browser` gate). The focusing press
**must** call the ungated `browsers.set_focus(&child, true)` (`browsers.rs:289`)
itself — ozmux owns the grant rather than relying on bevy_cef's deferred,
unordered `apply_webview_focus` (a `Local`/`is_changed`-gated Update system).
Order within the press: set `FocusedWebview` → `browsers.set_focus(true)` →
`send_mouse_click(down)`.

Same-frame caveat: `set_focus(true)` sets browser-process focus, but
`focused_frame()` reflects renderer-process state updated via an async
browser↔renderer IPC round-trip, so it does not flip to `Some` synchronously.
The press bootstraps focus and it is reliably available on **subsequent** frames;
a wheel event arriving in the very same frame as the first-ever focus grant may
still find `focused_frame() == None` and be dropped. This fails **safe** (the
event is silently dropped, never misrouted) and is not observable in practice
(focus persists across frames; the press and the first wheel-over-rect are
essentially never the same frame).

## Data flow detail

### Click — `tmux_mouse.rs::arbiter`

For each **left** button event, before the existing divider/select-pane/
drag-select logic:

```
press @ cursor_phys:
  gesture.inline_press = None                          # a fresh press invalidates any stale marker
  pane = pane_under_cursor(cursor_phys)
  hit  = pane ? inline_hit_at(pane, local_phys)        # interactive slots only (excludes NonInteractive)
  if hit:
     FocusedWebview = Some(hit.child)
     browsers.set_focus(hit.child, true)               # UNGATED grant — mandatory, MUST precede the click
     send select-pane(host pane)
     browsers.send_mouse_click(hit.child, hit.local_dip, Left, mouse_up=false)
     gesture.inline_press = Some(hit.child)            # consumed; no divider/drag-select arm; no click-count bump
  else:
     if FocusedWebview holds an inline child: FocusedWebview = None   # off-rect click releases focus
     # fall through to the existing Pressed / Resizing / select-pane flow

release @ cursor_phys:
  if gesture.inline_press.take() == Some(child):
     dip = inline_release_dip(child, cursor_phys)       # inline_local_dip WITHOUT containment (drift-tolerant)
     browsers.send_mouse_click(child, dip, Left, mouse_up=true)
  else:
     # existing Released flow (copy-selection, divider-click select-pane, ...)
```

The modal guard at `src/tmux_mouse.rs:240` currently drains ALL mouse events when
`picker.open || copy_prompt.open.is_some() || focused_webview.0.is_some()` AND
resets the gesture. **Keep** the `picker` / `copy_prompt` drain+reset unchanged (a
modal still owns input — see the edge-case table), but **drop** the
`focused_webview.0.is_some()` term entirely: the inline click pre-step above now
owns focus (in-rect press keeps it, off-rect press releases it and drives tmux),
so a focused webview must no longer blanket-drain mouse input.

### Move (hover + drag) — `tmux_mouse.rs::forward_tmux_inline_mouse_moves` (new)

A single stateless system, an exact mirror of the native `forward_inline_mouse_moves`
(`src/inline_webview.rs:565`) but resolving the pane via `pane_under_cursor`
(`TmuxPane` hosts) instead of the `SurfaceMarker + Slotted` host query. It is
`CursorMoved`-driven (at most one forward per frame, at the latest position — NOT
per-idle-frame), and forwards whatever buttons are held, so the same system serves
both hover and an in-rect drag (text selection / slider):

```
moved = CursorMoved.last(); if none: return
if modal open (picker / copy-search prompt): return
(terminal, local_phys) = pane_under_cursor(moved.position * scale)   # TmuxPane host
hit = inline_hit_at(terminal, local_phys)                            # interactive rects only
if hit:
   browsers.send_mouse_move(hit.child, mouse_buttons.get_pressed(), hit.local_dip, mouse_leave=false)
```

No drag-capture and no `mouse_leave` tracking — this matches native exactly: a drag
that leaves the rect simply stops extending (its release click-up is still routed to
the child by the arbiter's `inline_press` marker via `inline_release_dip`, so the
selection completes at the last in-rect position). `bevy_cef`'s `send_mouse_move` is
itself focus-gated, so hovers/drags over an unfocused browser are dropped
browser-side, not here.

### Wheel — `tmux_input.rs::forward_wheel_to_tmux`

```
target = resolve_tmux_inline_wheel_target():
    child = focused_inline_of(FocusedWebview, pane_under_pointer)   # focus required
    require pointer in child's rect (inline_hit_at)
for ev in wheel:
    if target: browsers.send_mouse_wheel(child, local_dip, raw_delta(ev))   # Line ×120, Pixel as-is, no sign flip
    else:      existing tmux aggregate (copy-mode scroll / WheelUpPane → copy-mode entry)
if target was Some: reset residual; return                          # tmux path untouched this frame
```

The blanket `focused_webview.is_some()` early-return is **removed**; gating is
now pointer-position-aware exactly like the native `resolve_inline_wheel_target`.
A focused webview no longer steals wheel events aimed at the terminal region.

### Keyboard — unchanged

`forward_keys_to_tmux`'s `focused_webview.0.is_some()` early-return is already
correct: the webview owns the keyboard, `Ctrl+Shift+Esc` releases. It begins to
function for real once `FocusedWebview` is tmux-aware.

### Focus validator — `webview_render.rs` (per frame)

Under tmux, do **not** drive focus from the active pane. Keep `FocusedWebview`
while it points at an inline child (`ChildOf`) of a live `TmuxPane`; GC to `None`
when that child despawns or its host pane is gone. The native old-multiplexer
preserve arm is retained for the native backend.

## Error handling & edge cases

| Case | Handling |
| --- | --- |
| focused inline child despawns / host pane gone | validator GCs `FocusedWebview = None`; CEF forwards no-op via `get_focused_browser` → `None` |
| `Browsers` (NonSend) absent (CEF-less tests) | `Option<NonSend<Browsers>>` like the native path; `None` → skip forward, run tmux path |
| `NonInteractive` inline (display-only) | excluded by `inline_hit_at`; never focused/clicked/wheeled — tmux path as before |
| multi-notch fling while focus changes | wheel resolves target once per frame → frame-consistent; residual reset on target switch |
| modal (picker / copy-search) opens while webview focused | modal wins; arbiter + forwarder no-op; focus preserved, restored when modal closes |
| off-rect press that releases focus | the same press both clears focus AND runs the normal tmux gesture (one click) |
| pane resize / layout change mid-press | the `inline_press` marker is keyed to the child entity, so the release click-up still routes to that child (drift-tolerant `inline_release_dip`) even if its rect moved |
| clicking an inline in a copy-mode pane | inline wins (focus + CEF); the pane's copy-mode state is independent (released via tmux as usual) |
| drag starts in rect, pointer leaves rect while held | moves stop extending once off-rect (matches native); the release click-up is still routed to the child via `inline_press` + `inline_release_dip`, completing the selection at the last in-rect position |

Invariant: every CEF forward is focus-gated, so any "`FocusedWebview` child ≠
CEF focused frame" desync fails safe (dropped, never misrouted). The focusing
press's explicit ungated `browsers.set_focus(&child, true)` (NOT
`send_mouse_click`'s internal, gated `set_focus`) is what establishes focus.

## Testing

Mirror the native tests (`src/input/mouse_wheel.rs` `resolve_inline_wheel_target`
suite: `target_resolves_when_focused_inline_under_pointer`,
`target_none_when_pointer_off_the_rect`, `target_none_when_inline_not_focused`)
for the tmux variant:

- **Pure-function unit tests** — `resolve_tmux_inline_wheel_target`:
  (a) focused inline under pointer → `Some`, (b) pointer off-rect → `None`,
  (c) unfocused → `None`, (d) `NonInteractive` → `None`.
- **Arbiter inline routing** (`RunSystemOnce` + `MinimalPlugins`, a
  `TmuxPane` + `TerminalOverlays` + `InlineWebview` child fixture modeled on the
  existing `make_wheel_app`): press in rect → `FocusedWebview` set to child;
  press off rect → `FocusedWebview` cleared and the gesture proceeds to the
  normal flow; a consumed press does not arm divider/drag-select.
- **Move forwarder** (`forward_tmux_inline_mouse_moves`) — a `CursorMoved` over an
  interactive inline rect resolves the child + DIP; over the terminal resolves
  nothing. (CEF calls are not exercised when `Browsers` is absent; the routing
  decision is asserted via the pure `inline_hit_at` resolution.) Mirror the native
  forwarder's test shape.
- **First-click focus bootstrap** — a press inside an inline rect on a
  never-focused webview must record the ungated `set_focus(child, true)` BEFORE
  the click (assert ordering and that the grant is not focus-gated), so the
  first click is not swallowed and focus actually bootstraps.
- **Focus validator** — focused child despawn → `FocusedWebview` GC'd to `None`.
- **Regression** — a tmux pane with no inline webview still scrolls into
  copy-mode / drag-selects exactly as before (the inline path yields no
  false-positive target).

CEF forwarding itself (`Browsers`) is not invoked in the `Option`-absent tests,
matching the native convention; whether a forward happens is asserted through
the pure target-resolution / gesture-state assertions.

## Alternatives considered

- **Generalize the native `mouse_buttons.rs` / `mouse_wheel.rs` to be
  backend-agnostic** (one host query covering both `SurfaceMarker + Slotted` and
  `TmuxPane`). Rejected: larger blast radius into the native path, which is tied
  to old-multiplexer active-pane focus (`try_click_to_focus`,
  `MultiplexerCommands`); against the "tmux priority, leave native alone" scope.
  (Note: a *narrow* extraction of just the host-agnostic
  `(terminal, local_phys) → inline_hit_at → send_mouse_*` tail — distinct from
  this larger refactor — is a reasonable optional dedup with no native blast
  radius; left to implementation discretion.)
- **Minimal: fix only the focus source + wheel, rely on bevy_cef's own
  `set_focus_on_press` for clicks.** Rejected: inline webviews render as overlay
  textures on the pane grid, not as discrete bevy_cef-hit-tested UI nodes, so
  bevy_cef would not see the rect and clicks would not focus/activate — likely
  insufficient.

## Future enhancement (out of scope)

Pure hover-wheel (scroll the webview under the pointer without a prior click)
requires extending `bevy_cef` so wheel can be delivered to a browser without a
focused frame (or via transient focus). `bevy_cef` is a path dependency under
the user's control, so this is feasible later, but it is deliberately excluded
here to keep the change tmux-local and avoid an upstream-crate change.
