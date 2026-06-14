# Alt-Screen Fixed-Anchor Inline Webviews (ozmux core)

**Status:** Design approved, pending spec review
**Date:** 2026-06-14
**Scope:** Subsystem A of the `ratatui-ozma` initiative. This spec covers the
ozmux *core* change that lets an inline webview be placed on the alternate
screen at a fixed viewport rectangle. It is a prerequisite for Subsystem B
(the `ratatui-ozma` SDK crate), which is specified separately.

## 1. Motivation

`ratatui-ozma` will expose ozmux inline webviews as a Ratatui `Widget`
(HTML rendering + Rust-side RPC handlers). A typical Ratatui application runs
**full-screen on the alternate screen**. The current inline-webview machinery
rejects mounting on the alternate screen, so the SDK cannot work in the
environment its users actually run in. This spec removes that limitation by
adding a second anchoring mode designed for the alternate screen, and folds in
a related simplification (`SurfaceKind` removal) that the change makes possible.

## 2. Current architecture (grounding)

- An inline webview is a **child entity of a Terminal surface**, carrying
  `InlineWebview { view_id, instance_id, slot }`
  (`src/inline_webview.rs:31-46`) and `InlinePlacement`
  (`src/inline_webview.rs:53-66`). The owning surface is the `ChildOf` parent.
- `InlinePlacement` today is
  `{ anchor_line: u64, anchor_col: u16, rows: u16, cols: u16, frame_seq: u32 }`,
  where `anchor_line` is an **absolute scrollback line**.
- **Mount path:** the OSC 5379 `mount-inline` sequence is parsed in
  `crates/ozma_tty_engine/src/osc_webview.rs:93-115`; the anchor is stamped at
  the OSC stop-point in `handle_webview_verb`
  (`crates/ozma_tty_engine/src/handle.rs:906-946`). Mounting is **rejected on
  the alternate screen** (`handle.rs:919-922`) and when scrollback is
  **saturated** (`handle.rs:924-926`). ozmux receives the request in
  `on_osc_webview_request` (`src/osc_webview.rs:51-110`), which calls
  `mount_inline` (`src/inline_webview.rs`).
- **CEF page load happens at mount, not register.** `register` over the
  control socket only populates `DynamicRegistry` (handle ↔ URL). `mount_inline`
  spawns the webview entity and inserts `WebviewSource` (the CEF page load)
  (`src/inline_webview.rs:242-255`).
- **Duplicate mount is rejected.** A second mount of the same
  `(view_id, instance_id)` is dropped (`src/inline_webview.rs:213-224`,
  test `duplicate_mount_same_view_is_rejected` at `:863-877`). `InlinePlacement`
  is immutable after mount; there is no move/resize path. Geometry only flows
  one way: `sync_inline_webview_size` (`src/inline_webview.rs:478-499`)
  recomputes `WebviewSize` from the static `InlinePlacement` each frame, writing
  only on change (a spurious write recreates the IOSurface).
- **Projection:** `project_inline_overlays` (`src/inline_webview.rs:579-637`)
  converts the absolute anchor line to a viewport row each frame via
  `viewport_row = anchor_line - (history_base + history_size - display_offset)`
  (`:611`), culling off-screen rects and **blanking all overlays while on the
  alternate screen** (`:605-607`).
- **`SurfaceKind`** (`crates/multiplexer/src/components.rs:143-151`) is a
  Surface component with variants `Terminal` and `Extension { entry }`. **Every
  production `add_surface` call uses `Terminal`;** `Extension` is constructed
  only in `#[cfg(test)]` code (`src/ui.rs:718`,
  `crates/multiplexer/src/commands.rs:1223`). `finish_extension_setup` exists
  only as a doc-comment reference (`src/ui.rs:72`); the Extension apparatus was
  removed in commit `04405cb`. All `Extension` match arms (tab label
  `WebTitle`, `SURFACE_EXTENSION` color, the non-terminal veil path,
  `ExtensionSurfaceMarker`) are therefore unreachable in production.

## 3. Goals / non-goals

**Goals**

- Allow an inline webview to be placed on the **alternate screen** at a fixed
  viewport cell rectangle, re-anchored every frame **without reloading the CEF
  page**.
- Make the placement model match established terminal-graphics practice
  (Kitty/iTerm2/ratatui-image): scroll-anchored on the main screen,
  re-placed-to-a-fixed-cell-rect on the alternate screen.
- Remove the now-vestigial `SurfaceKind` enum and its dead `Extension` branches.

**Non-goals (this spec)**

- The `ratatui-ozma` SDK crate (Subsystem B): the `Widget`, the Rust RPC
  handler API, the per-frame render loop, focus integration. Specified
  separately.
- Any change to the control-plane back-channel protocol
  (`hello`/`register`/`unregister`/`emit`/`call`/`reply`). The back-channel is
  unchanged; only placement/anchoring changes.
- Multiple overlapping fixed webviews as a first-class feature (see §4.7).
- Carrying a fixed-anchor webview from the alternate screen back to the main
  screen (explicitly auto-unmounted; see §4.5).

## 4. Design

### 4.1 `AnchorMode`

Replace `InlinePlacement.anchor_line`/`anchor_col` with a tagged anchor:

```rust
enum AnchorMode {
    /// Anchored to an absolute scrollback line; scrolls with the text.
    Scrollback { line: u64, col: u16 },
    /// Anchored to a viewport-relative cell; fixed on the visible screen.
    FixedScreen { row: u16, col: u16 },
}
```

`InlinePlacement` becomes `{ anchor: AnchorMode, rows: u16, cols: u16, frame_seq: u32 }`.

The cross-crate payload `ozma_tty_engine::InlineAnchor`
(`crates/ozma_tty_engine/src/vt/listener.rs:46-53`, today `{ line, col,
frame_seq }`) is the value `handle_webview_verb` stamps and `mount_inline`
consumes (`handle.rs:930-936` → `inline_webview.rs:262-268`). It must gain the
same mode discriminant. Define the mode-tagged anchor **once in
`ozma_tty_engine`** and reuse it in `src/inline_webview.rs` (already imports
`InlineAnchor` at `:24`) so the engine payload and the ozmux component do not
drift.

The name is `AnchorMode` (not `InlineAnchorMode`): with `SurfaceKind` gone
(§4.6), "inline" stops being a distinguishing surface kind, so the anchor mode
is the sole discriminator among webview placements and does not need the
`Inline` qualifier.

### 4.2 Mount-time stamping

In `handle_webview_verb` (`crates/ozma_tty_engine/src/handle.rs:919`), replace
the alternate-screen rejection with a branch on the terminal mode **observed at
the OSC stop-point**:

```text
on mount-inline at OSC stop-point:
    if term.mode().contains(TermMode::ALT_SCREEN):
        anchor = FixedScreen { row: cursor.row (viewport-relative), col: cursor.col }
    else:
        if saturated: reject            # saturation gate stays, scrollback-only
        anchor = Scrollback { line: history_base + history_size + cursor.row, col }
```

- The screen mode is determined solely by `TermMode::ALT_SCREEN`, which
  `alacritty_terminal` toggles when the application emits the alternate-screen
  private modes (1049 / 1047 / 47). No new state is introduced.
- The saturation gate (`handle.rs:924-926`) applies **only** to the
  `Scrollback` branch; a `FixedScreen` anchor does not depend on scrollback
  capacity.
- **Invariant (ordering):** the anchor is stamped using the cursor position and
  mode at the exact `advance_until_terminated` stop-point where the OSC
  terminates. The existing split-OSC / same-chunk-preceding-output tests
  (`handle.rs` around `:1937`) already pin this stop-point for the scrollback
  case; equivalent coverage is added for the alt-screen case, including an OSC
  that arrives in the same chunk immediately after `ESC[?1049h`.

`FixedScreen.row`/`col` are **viewport-relative** (0-based from the top-left of
the visible screen), matching where the cursor sits on the alternate screen.

### 4.3 Re-mount = in-place re-anchor (no reload)

Change the duplicate-mount semantics in `mount_inline`
(`src/inline_webview.rs:213-224`) from **reject** to **in-place update**, for
**both** anchor modes:

- If a live `(view_id, instance_id)` child already exists on the terminal,
  **do not despawn/respawn**. Take the fast path **before** `resolve_mount`,
  slot allocation, preload construction, or `WebviewSource` insertion (the
  live-children scan already runs first at `:213`): `set_if_neq` its
  `InlinePlacement` (`anchor`, `rows`, `cols`) on the existing entity and
  return. `InlinePlacement` already derives `PartialEq` (`:53`), so a no-op
  re-emit elides the change-detection write entirely. The CEF page is **not
  reloaded** (the `WebviewSource` entity is untouched), and the registry
  lookup / allocation cost is skipped on every re-emit frame.
- **The existing child's `InlineWebview.slot` is preserved** — the in-place
  path MUST NOT re-run slot allocation (`smallest_free_slot`). The overlay
  texture lands in `overlays.textures[slot]` (`:630`); re-allocating the slot
  mid-stream would churn the texture target and break the no-reload guarantee.
- **Anchor change (move):** flows only into `project_inline_overlays`; the
  overlay rect moves. Cost ≈ zero.
- **`rows`/`cols` change (resize):** flows through the existing
  `sync_inline_webview_size` into `WebviewSize`; `bevy_cef` resizes the CEF
  surface (recreating the IOSurface). The **DOM is preserved** — this is a
  browser window resize, not a navigation. `set_if_neq` already suppresses
  no-op writes, so a re-emit with unchanged `rows`/`cols` triggers no resize.

This is what makes the per-frame re-emit model (§4.4) cheap and reload-free, and
it generalizes a useful capability to the main screen too (a scrollback webview
can now be moved/resized in place rather than unmount→remount).

The test `duplicate_mount_same_view_is_rejected` is replaced by
`duplicate_mount_updates_placement_in_place` (asserts: still exactly one child;
`InlinePlacement` reflects the new anchor/rows/cols; the `WebviewSource` /
webview entity id is unchanged ⇒ no reload).

### 4.4 Per-frame re-emit model (the A/B contract)

The SDK (Subsystem B) re-emits `mount-inline` for its handle **every frame**,
positioning the terminal cursor at the widget's top-left first. ozmux treats
each re-emit as an idempotent re-anchor (§4.3). Consequences:

- Layout changes and terminal resizes are followed **for free**: the next
  frame's re-emit carries the new `row`/`col`/`rows`/`cols`.
- No page reload occurs on any frame; only a genuine `rows`/`cols` delta causes
  a CEF surface resize.

This contract is what Subsystem B's `Widget` will rely on; it is the reason the
re-anchor must be reload-free.

### 4.5 Projection

Split the per-frame projection in `project_inline_overlays`
(`src/inline_webview.rs:579-637`) by anchor mode:

```text
Scrollback { line, col }:   # unchanged
    viewport_row = line - (history_base + history_size - display_offset)
    cull off-screen / right-overflow; negative row → shader clips
FixedScreen { row, col }:   # new
    viewport_row = row      # already viewport-relative, no history math
```

Visibility by screen mode changes from "blank everything on alt-screen" to a
per-mode, per-anchor rule:

| Screen      | `Scrollback` webview      | `FixedScreen` webview          |
|-------------|---------------------------|--------------------------------|
| main        | shown (scroll-anchored)   | does not exist (auto-unmounted)|
| alt-screen  | **hidden** (as today)     | **shown**                      |

The `frame_seq` hold (defer first projection until the grid's `last_seq`
reaches the mount-stamped `frame_seq`) applies to **both** anchor modes.

### 4.6 Lifecycle: alt-screen exit ⇒ auto-unmount

A `FixedScreen` webview is torn down when its terminal leaves the alternate
screen:

- Detect the falling edge of `TermMode::ALT_SCREEN` (alt → main) via the
  existing `TerminalModeChanged` event, whose `removed` field carries the
  cleared modes (`crates/ozma_tty_engine/src/events.rs:35-43`). The engine
  triggers mode changes **before** the per-frame trigger, with that ordering
  documented (`handle.rs:804-830`), and inline projection runs in `PostUpdate`
  (`inline_webview.rs:88-91`) — so an observer/system consuming
  `TerminalModeChanged` despawns the surface's `FixedScreen` children before
  the next projection, **atomically with the screen switch**. There is no
  current consumer of this event; this is its first, purpose-built use, which
  avoids adding a `PrevAltScreen` tracking component or polling `grid.modes`.
  This structurally avoids the Kitty issue #2901 failure mode (a stale,
  non-scrolling rectangle left painted after the alt-screen is torn down).
- `FixedScreen` webviews are **never carried over** to the main screen. Only
  the mounted child entity is despawned; the underlying dynamic registration /
  handle in `DynamicRegistry` is **not** removed (that happens only on
  `unregister`, socket disconnect, or surface despawn —
  `src/control_plane.rs:366-387`). The SDK re-establishes the view on the next
  alt-screen session by re-mounting the still-valid handle (re-registering only
  if it let the handle lapse).
- `Scrollback` webviews are unaffected by alt-screen transitions (they remain
  hidden during alt-screen and reappear on return, as today).

### 4.7 `SurfaceKind` removal

`SurfaceKind` is vestigial (§2): production builds only `Terminal`. Remove it
and the dead `Extension` branches:

- Delete the `SurfaceKind` enum (`crates/multiplexer/src/components.rs:143-151`)
  and the `entry`-carrying `Extension` variant; update `add_surface` /
  `split_pane_with_surface` signatures (`crates/multiplexer/src/commands.rs`) to
  no longer take a kind.
- Delete the `Extension` arms and their now-constant counterparts:
  - `src/ui/tab_label.rs` `WebTitle` branch → always `Cwd`.
  - `src/theme.rs` `SURFACE_EXTENSION` + `kind_color`.
  - `src/ui/surface.rs`: `decorate_surface` loses its only `match` and
    `kind_color` collapses to the single `SURFACE_TERMINAL` constant — drop the
    `kind` parameter and inline the terminal background.
  - `src/ui/chrome.rs`: `sync_pane_veil` exists **solely** to veil non-terminal
    surfaces, so with `active_is_terminal` becoming a constant `true` the whole
    system is dead and is **deleted** (not reduced to "always `PaneDim`"); the
    `kinds` query and `slot_active_surface` helper go with it.
  - `src/ui.rs` `ExtensionSurfaceMarker`.
  - The `#[cfg(test)]` Extension fixtures (`extension_pane_keeps_pickable_ignore_veil`,
    `split_pane_with_surface_seeds_extension_surface`) are removed or rewritten
    against the simplified API.
- After removal, a webview surface is distinguished from a plain terminal solely
  by the presence of `InlineWebview` children and their `AnchorMode` — not by a
  surface-kind tag.

`TerminalSurfaceMarker`: keep only if a query still needs it after the
`Extension` arm is gone; if every remaining consumer can query the surface
component set directly, drop it too. This is settled during implementation by
grepping its consumers — the spec's intent is "no surface-kind discriminator
survives," not "preserve the marker." Codex review notes `finish_terminal_setup`
(`src/ui/terminal.rs:46-56`) is its only discriminating consumer; once every
surface is terminal-backed it duplicates `SurfaceMarker`, so dropping it and
querying `SurfaceMarker` + `Without<TerminalHandle>` is the likely outcome.

### 4.8 Edge policies

- **Z-order / overlap:** keep the existing `OVERLAY_SLOTS` cap (max 4 per
  surface). Ratatui layouts tile (non-overlapping), so overlap is not expected;
  when fixed rects do overlap, stacking is by slot index and is **defined but
  not guaranteed** (documented, not a feature). No new z-model is added.
- **Focus / input:** unchanged — click-to-focus, and the `interactive` flag
  from `register` governs whether the view takes input.
  **Invariant:** an in-place re-anchor (§4.3) must not despawn the webview
  entity or otherwise reset CEF focus/input state; per-frame re-emit must not
  steal or drop focus. (Guaranteed by never respawning on re-mount.)
- **Geometry limits:** unchanged — `rows ∈ 1..=200`, `cols ∈ 1..=400`, ≤ 4 per
  surface. A full-screen webview fits within these bounds.

## 5. Invariants

1. **No reload on re-anchor.** A re-emitted `mount-inline` for a live handle
   never reloads the CEF page; the `WebviewSource`/webview entity id is stable
   across re-emits. Only a `rows`/`cols` delta resizes the surface.
2. **Stamp at the stop-point.** The anchor (mode, cursor row/col) is captured at
   the OSC terminator stop-point, consistent with the existing scrollback tests.
3. **Atomic teardown.** Leaving the alternate screen despawns `FixedScreen`
   children in the same frame the mode flips, before the next projection — no
   stale rectangle is ever painted.
4. **Focus stability.** Per-frame re-anchor preserves CEF focus/input state.

## 6. Testing strategy

- **Engine (`crates/ozma_tty_engine`):**
  - alt-screen `mount-inline` stamps a `FixedScreen` anchor (no longer
    rejected); cursor row/col is viewport-relative.
  - alt-screen mount is **not** gated by saturation; main-screen mount still is.
  - stop-point coverage: OSC immediately after `ESC[?1049h` in the same chunk
    stamps `FixedScreen` with the correct (post-switch) cursor.
- **ozmux (`src/inline_webview.rs`):**
  - `duplicate_mount_updates_placement_in_place` (replaces the reject test):
    one child; placement updated; webview entity id unchanged.
  - resize via re-emit changes `WebviewSize` but not the webview entity id;
    move via re-emit changes neither `WebviewSize` nor the entity id.
  - projection: `FixedScreen` projects to `viewport_row == row` and is shown on
    alt-screen; `Scrollback` is hidden on alt-screen.
  - alt-screen exit despawns `FixedScreen` children in the transition frame;
    `Scrollback` children survive and reappear.
- **`SurfaceKind` removal:** the workspace compiles with the enum gone; tab
  label always renders `Cwd`; the veil path is always `PaneDim`; removed test
  fixtures do not regress unrelated coverage.

## 7. Out of scope / follow-up

- Subsystem B: the `ratatui-ozma` SDK crate (Widget + Rust RPC handler API +
  per-frame render loop). It consumes the §4.4 contract.
- A first-class overlapping/z-ordered fixed-webview model, if a future use case
  needs more than tiling layouts provide.
