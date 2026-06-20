# ozbrowser keyboard link hints (Vimium-style)

Status: approved design (2026-06-20)

## 1. Goal

Let an ozbrowser user follow in-page links and operate basic interactive
elements with the keyboard alone, the way Vimium's `f` hint mode works.
Pressing `f` in Normal mode overlays short labels on the page's clickable
targets; typing a label activates that target. No mouse, no tab cycling.

In scope as targets: `a[href]`, `button`, `[role=button]`, `[onclick]`, and
form fields (`input`, `textarea`, `select`). Activation is element-aware:

| Target | Activation | Resulting TUI mode |
| --- | --- | --- |
| Link (`a[href]`) | synthetic click → webview navigates | Normal |
| Button / `[role=button]` / `[onclick]` | synthetic click, stay on page | Normal |
| Form field | `focus()` the element | Insert |

## 2. Architecture context

ozbrowser is a ratatui TUI (`apps/ozbrowser`) that drives a CEF webview through
the `ratatui-ozma` SDK. It owns a mode state machine (Normal / Insert / Address
/ Help) and a keymap, and it actuates the page by **emitting events** to the
webview (e.g. `view.emit("scroll", …)`).

ozbrowser loads **arbitrary remote URLs**, so it cannot bundle its own page
script the way `apps/ozmd` (which serves its own asset dir) can. The only way to
run JS in a remote page is the host's **preload-script injection**. The host
already injects `src/webview_render/ozma_bridge.js` (the `window.ozma`
back-channel) into every bridged webview, and that bridge carries a built-in
default `scroll` handler. The hint engine follows the same precedent: a new
host-owned preload script injected alongside the bridge.

This gives the standard two-layer split, identical to the existing `scroll`
feature:

- **TUI layer** owns mode/keymap state and forwards intent as emitted events.
- **Page layer** (preload JS) manipulates the DOM and reports outcomes back
  through the `window.ozma` channel.

### Decisions taken during brainstorming

- **Target scope**: links + buttons + form fields (not links-only, not the full
  Vimium element set).
- **Form field activation**: `focus()` the field and auto-switch the TUI to
  Insert mode.
- **Interaction model — Approach 1 (TUI-driven)**: the TUI captures hint-label
  keystrokes in a dedicated mode and forwards each to the page; the page never
  takes keyboard focus. Chosen over a page-driven focus-handoff model because it
  keeps the authoritative mode machine in the TUI and avoids the focus-on-mount
  races the `WebviewWidget` focus docs warn about. The per-keystroke local
  socket round-trip is negligible.
- **JS placement**: a dedicated preload file `ozma_hints.js`, injected into **all
  URL webviews** (after the bridge), positioned as a host-provided default
  capability for URL webviews — not an ozbrowser-private script.

## 3. Control & data flow

```
Normal + 'f'
  └─ TUI: mode = Hint; emit("hints:show", {})
        └─ Page: enumerate targets, assign labels, draw overlay
Hint mode keystrokes
  ├─ printable char → TUI: emit("hints:key", { key })
  │     └─ Page: extend prefix, dim non-matching;
  │              on unique full match → activate by element type,
  │              clear overlay, ozma.call("hintResult", { kind })
  ├─ Backspace → TUI: emit("hints:key", { backspace: true })
  └─ Esc → TUI: emit("hints:hide", {}); mode = Normal
Page → TUI: hintResult { kind }
  └─ TUI drains channel: focusedInput → Insert; else → Normal
```

`kind` is one of: `navigated`, `clicked`, `focusedInput`, `empty`. The page
sends `empty` immediately from `hints:show` when no targets exist so the TUI
leaves Hint mode without the user typing.

### Why this is race-free

In Hint mode the webview stays **unfocused** (exactly like Normal mode), so every
keystroke arrives at the TUI via `crossterm::event::read`. There is no focus
handoff and the page registers no `keydown` listener. The single place focus
moves is the form-field case, where the page calls `element.focus()` and the TUI
transitions to Insert only after receiving `hintResult { kind: "focusedInput" }`.

## 4. TUI changes (`apps/ozbrowser`)

### `keymap.rs`

- Add `Mode::Hint`.
- Add actions: `Action::HintKey(char)`, `Action::HintBackspace`. Reuse
  `Action::Escape`.
- `map_normal`: bind `KeyCode::Char('f')` → a new `Action::EnterHint`. `f` and
  `F` are currently unbound in Normal mode (only `Ctrl-f` is used), so no
  conflict.
- Add a `Mode::Hint` arm to `map`: `Esc` → `Escape`; `Backspace` →
  `HintBackspace`; printable `Char(c)` → `HintKey(c)`; `Ctrl-c` → `Quit`;
  everything else → `Ignore`. (Hint mode forwards letters to the page rather
  than interpreting them as Normal-mode commands.)

### `app.rs`

- Add `Cmd::HintShow`, `Cmd::HintKey(char)`, `Cmd::HintBackspace`, and
  `Cmd::HintHide`.
- `Action::EnterHint` → set `mode = Hint`, return `vec![Cmd::HintShow]`.
- `Action::HintKey(c)` → `vec![Cmd::HintKey(c)]` (mode unchanged).
- `Action::HintBackspace` → `vec![Cmd::HintBackspace]`.
- `Action::Escape` in Hint mode → set `mode = Normal`, return
  `vec![Cmd::HintHide]`.
- New method `App::on_hint_result(kind)` that sets `mode = Insert` for
  `focusedInput` and `mode = Normal` otherwise. Called by the event loop when it
  drains the result channel.

### `main.rs` (event loop)

- A second `crossbeam_channel` carries hint results from the `on("hintResult", …)`
  RPC handler into the loop, drained at the top alongside `url_rx` (same pattern
  as `url_tx`/`url_rx` for `urlChanged`). Draining calls `app.on_hint_result`.
- Register `.on("hintResult", …)` on the webview in `register_view`.
- Map the new `Cmd`s to emits: `HintShow` → `emit("hints:show", {})`, `HintKey`
  → `emit("hints:key", { key })`, `HintBackspace` →
  `emit("hints:key", { backspace: true })`, `HintHide` → `emit("hints:hide", {})`.
- Add the hint-label letters / `Esc` / `Backspace` to the registration
  `passthrough` set only if testing shows they are needed; by default Hint mode
  is unfocused so passthrough is not involved.

### `ui.rs`

- Add a `Hint` status label to `draw_status_bar` and a `Hint` arm to
  `mode_label`. The hint overlay itself is drawn by the page, not ratatui.
- Add a hint line to the help modal (`f  follow link / hint`).

## 5. Page changes (host preload)

### New file `src/webview_render/ozma_hints.js`

A self-contained IIFE that registers handlers on `window.ozma`:

- `ozma.on("hints:show", …)`: collect candidate targets, filter to those visible
  and within the viewport, assign labels via `generateLabels(n)`, render an
  absolutely-positioned overlay (one badge per target, pinned to each target's
  bounding rect), reset the typed prefix. If `n === 0`, call
  `ozma.call("hintResult", { kind: "empty" })` and render nothing.
- `ozma.on("hints:key", payload)`: `payload.backspace` trims the prefix;
  otherwise append `payload.key` (lower-cased). Re-filter: hide badges whose
  label does not start with the prefix. On a single remaining exact match,
  activate it and tear down.
- `ozma.on("hints:hide", …)`: tear down the overlay and reset state.

Activation by element type:

- Link → `el.click()` (CEF follows the navigation; the existing `urlChanged`
  handler updates the TUI URL). Report `{ kind: "navigated" }`.
- Button / `[role=button]` / `[onclick]` → `el.click()`. Report
  `{ kind: "clicked" }`.
- Form field → `el.focus()`. Report `{ kind: "focusedInput" }`.

### Label algorithm (the unit-tested core)

- `generateLabels(n)`: Vimium-style shortest-unique labels over a home-row
  alphabet (default `"sadfjklewcmpgh"`, tunable). One character while the
  alphabet covers the count; otherwise fixed-width two-character labels (longer
  if ever needed). Deterministic, order-stable with the target list.
- `filterByPrefix(labels, prefix)`: case-insensitive prefix match returning the
  surviving labels (and whether exactly one is a full match).

These two functions are pure and have no DOM dependency; they are the primary
unit-test surface (see §7).

### Injection wiring (`src/webview_render/preload.rs`, `src/inline_webview.rs`)

- Add `OZMA_HINTS_JS = include_str!("ozma_hints.js")`.
- URL-source bridged webviews receive `PreloadScripts::from([OZMA_BRIDGE_JS,
  OZMA_HINTS_JS])`; inline/dir bridged webviews keep bridge-only. **Order is an
  invariant**: the bridge defines `window.ozma`, which `ozma_hints.js` consumes,
  so the bridge must run first.
- Carry the URL-ness through `ResolvedWebviewMount` (it already matches on
  `DynSource` in `resolve_mount`) and branch the preload builder in
  `mount_inline` where `build_dynamic_preload()` is inserted today. Add a
  `build_url_preload()` (bridge + hints) next to the existing
  `build_dynamic_preload()` (bridge only).

## 6. Edge cases

- **No targets**: page reports `empty`; TUI returns to Normal (optional status
  flash). Covered above.
- **Esc anytime**: TUI handles `Esc` locally (mode → Normal) and emits
  `hints:hide`; never depends on a page reply to cancel.
- **Non-alphabet key**: page ignores characters not in the label alphabet; the
  prefix is unchanged. Cancel is `Esc` only.
- **Stale keys after activation**: once the overlay is torn down the page ignores
  further `hints:key` events; the TUI leaves Hint mode as soon as it drains
  `hintResult`. Keys typed in the gap are harmless no-ops.
- **Scroll / resize during Hint mode**: v1 computes the target set once at
  `hints:show`; it does not re-anchor on scroll or resize. Re-anchoring is a
  later enhancement.
- **overlapping / off-screen elements**: only elements that are visible
  (non-zero box, not `display:none`/`visibility:hidden`) and intersect the
  viewport get a badge.

## 7. Testing

- **Rust (`apps/ozbrowser`)**, mirroring the existing `app.rs` / `keymap.rs`
  test style:
  - `keymap`: `f` → `EnterHint`; Hint-mode `Char(c)` → `HintKey(c)`; `Esc` →
    `Escape`; `Backspace` → `HintBackspace`.
  - `app`: `EnterHint` sets Hint mode and emits `HintShow`; `Escape` in Hint
    mode returns to Normal and emits `HintHide`; `on_hint_result("focusedInput")`
    → Insert; `on_hint_result("navigated"|"clicked"|"empty")` → Normal.
  - preload builder: a URL webview's `PreloadScripts` contains the hints script
    after the bridge; an inline/dir webview does not.
- **JS**: vitest over `generateLabels` and `filterByPrefix` (uniqueness,
  shortest-length growth, prefix narrowing, case-insensitivity). DOM enumeration
  and overlay rendering are verified manually.

## 8. Out of scope (YAGNI)

New-tab / new-view hint variants (ozbrowser is single-view), `yf` URL-yank,
in-hint search, iframe / shadow-DOM traversal, re-anchoring hints on scroll, and
threading hint-followed navigations into the TUI `History` stack (hint link
clicks behave like today's in-page link clicks: `urlChanged` updates the URL bar
but does not push history — `H`/`L` do not include them).
