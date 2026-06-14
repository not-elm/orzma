# tmux migration — Phase 3c: pane click-to-focus + dim

Design spec — 2026-06-15
Parent spec: `docs/superpowers/specs/2026-06-14-tmux-multiplexer-migration-design.md` (Phase 3 of the migration phasing)
Sibling specs: Phase 3a (keyboard input) and 3b (window status bar), both already on this branch.
Worktree/branch: `tmux-phase3` (extends PR #122 into `tmux-migration`).

## Goal

Make tmux panes **click-to-focusable** and visually distinguish the active pane
by **dimming the inactive panes** with an overlay. This is the first tmux mouse
feature; it is small and self-contained. Clicking a pane sends
`select-pane -t %<id>` and lets tmux's `%window-pane-changed` notification drive
the projection (command-echo); a system then shows a dim overlay on every pane
except the active one.

## Decisions settled during brainstorming

1. **Command-echo for focus.** A pane click sends `select-pane -t %<id>`; ozmux
   does NOT mutate `active_pane` locally. tmux's `%window-pane-changed` flips
   `ProjectionModel.active_pane`, which the dim system reacts to. Same model as
   3b window switching.
2. **Dim via overlay (pure Bevy UI).** Inactive panes are darkened by a
   semi-transparent overlay node, NOT by touching the renderer
   (`ozma_tty_renderer`) material/shader. Keeps the change binary-side only.
3. **Dim-only, no active border.** The active pane renders normally; inactive
   panes are dimmed. No border/highlight on the active pane (closest to tmux's
   default, simplest).
4. **Scope: click-to-focus + dim only.** Mouse wheel/scroll and text-selection
   forwarding into tmux panes are deferred to a later phase. A click here only
   focuses.

## Background (verified against the codebase)

- `crates/tmux_session/src/components.rs` — `TmuxPane { id: PaneId, dims:
  CellDims }`. Each projected pane is one entity.
- `src/tmux_render.rs` — `attach_tmux_pane_terminal` inserts, onto each
  `TmuxPane` entity that lacks a `TerminalHandle`, a detached `TerminalHandle` +
  `TerminalRenderBundle::new(material)` + an absolute `Node`. **The terminal is
  rendered by the material ON the pane node itself** (not a child), so adding
  `Interaction` to the pane node makes the pane the click target.
  `layout_tmux_panes` sets each pane's `Node` rect (`left/top/width/height`)
  from `TmuxPane.dims` every frame. `reconcile` parents panes via
  `ChildOf(window)`.
- `crates/tmux_session/src/model.rs` — `ProjectionModel.active_pane:
  Option<PaneId>`, set by the `%window-pane-changed` arm of `apply_event`. This
  is the source of truth for which pane is active.
- `crates/tmux_session/src/enumerate.rs` — hosts the typed command builders
  (`select_window_command`, `client_name_command`, …). The new
  `select_pane_command` joins it; exported from `lib.rs`.
- Interaction model: the repo uses **`bevy_ui` `Interaction`** (no
  `bevy_picking`/`Pickable` anywhere). `drive_tab_clicks` (`src/ui/tab_input.rs`)
  and the 3b window-bar entries use `(&Interaction, &Marker), Changed<Interaction>`
  in `InputPhase::Dispatch`. The same pattern is used here. UI focus picks the
  topmost node under the cursor whose `FocusPolicy` is `Block`; a child with
  `FocusPolicy::Pass` does not intercept — so the dim overlay must be
  `FocusPolicy::Pass` to let clicks reach the pane.
- `src/input/mouse_buttons.rs` — the legacy old-multiplexer mouse path
  (`PtyHandle`/`MultiplexerCommands`/surfaces). Dormant in tmux mode; NOT
  touched here (Phase 5 removes it). 3c adds a small dedicated tmux system
  rather than extending it.

## Architecture

```
click pane node ─▶ Interaction::Pressed ─▶ focus_pane_on_click
                     ─▶ select_pane_command(pane.id) ─▶ connection.handle().send
                     ─▶ tmux %window-pane-changed ─▶ ProjectionModel.active_pane (set_changed)
                     ─▶ sync_pane_dim: overlay Hidden on active, Visible on others
```

### Command builder (`crates/tmux_session`)

- `select_pane_command(id: PaneId) -> String` → `select-pane -t %<id>`
  (`pub`, exported; mirrors `select_window_command`; `%<id>` target is
  structurally safe/numeric, no quoting).

### Pane focus + dim (`src/ui/tmux_pane_focus.rs`, new binary module)

- **`augment_tmux_pane`** (Update system, runs each frame, targets each pane
  once — query `TmuxPane` `With<TerminalHandle>` `Without<PaneFocusReady>`, OR
  fold into a marker): for each rendered pane that hasn't been augmented yet:
  - add `Interaction` (make the pane node a click target, as the window-bar
    entries do; add `FocusPolicy::Block` if the pane node's default isn't Block),
  - spawn a `PaneDimOverlay` child: an absolute `Node` filling the pane
    (`top/left/right/bottom: 0` or `width/height: 100%`), a themed
    semi-transparent dark `BackgroundColor` (~35% black; pick a `src/theme`
    token or a documented constant), `FocusPolicy::Pass` (never intercept the
    pane's click), and `Visibility::Hidden` initially,
  - mark the pane augmented (a `PaneFocusReady` marker) so it's done once.
  (NOTE: the overlay is a CHILD of the pane node; UI z-order renders children
  above the parent, so the overlay dims the terminal material beneath it. It
  follows the pane on resize because it is sized relative to the parent.)
- **`focus_pane_on_click`** (`InputPhase::Dispatch`, mirroring
  `drive_tab_clicks` so the focus change is same-frame): query
  `(&Interaction, &TmuxPane), Changed<Interaction>` + `connection:
  NonSend<TmuxConnection>`. On `Interaction::Pressed`, if `connection.client()`
  is `Some`, send `select_pane_command(pane.id)`; `tracing::warn!` on error. No
  projection mutation (command-echo).
- **`sync_pane_dim`** (gated `run_if(resource_exists_and_changed::<ProjectionModel>)`):
  for each `(&TmuxPane, &PaneDimOverlay-child)` — resolve each pane's overlay
  child and set its `Visibility` to `Hidden` when `Some(pane.id) ==
  model.active_pane`, else `Visible`. Change-guard the write (only assign when
  the value differs) to avoid per-frame relayout/redraw. Find the overlay via
  the pane's `Children` + a `PaneDimOverlay` filter (or store the overlay entity
  on the pane via a component for an O(1) lookup — plan's choice).
- **`OzmuxTmuxPaneFocusPlugin`** registers the three systems; added in
  `src/main.rs` near the other tmux plugins. `src/tmux_render.rs` stays
  render-only.

## Testing

- **Pure unit (`ozmux_tmux`):** `select_pane_command(PaneId(3)) ==
  "select-pane -t %3"`.
- **Pure mapping (binary):** a pressed `TmuxPane` maps to
  `select_pane_command(pane.id)` (a tiny helper if it aids testing, like the
  window-bar `entry_command`).
- **Bevy headless (binary):** seed a `ProjectionModel` with `active_pane =
  Some(PaneId(1))` and two `TmuxPane` entities (ids 1 and 2) each with a
  `PaneDimOverlay` child; run `sync_pane_dim`; assert pane 1's overlay is
  `Hidden` and pane 2's is `Visible`. Then flip `active_pane` to `PaneId(2)`,
  re-run, assert the visibilities swap.
- **Gated real-tmux (`crates/tmux_session/tests/real_tmux_pane.rs`, ignored):**
  attach → `split-window` (now ≥2 panes; tmux auto-focuses the new one) → drain
  until `active_pane` moves off the first pane → `select-pane -t %<first>` →
  assert `active_pane` returns to the first. Mirrors the 3b
  `real_tmux_window.rs` round-trip; verify-live gate for `select-pane` +
  command-echo.
- **Manual GUI** (desktop, outside tmux): split a window (tmux `C-b %`/`"`),
  confirm inactive panes are dimmed and the active one is not; click an inactive
  pane → it focuses (un-dims) and the previously-active one dims.

## Risks / unknowns

- **Overlay must not steal the click:** the dim overlay child is rendered above
  the pane and could intercept the `Interaction` if its `FocusPolicy` is `Block`.
  It MUST be `FocusPolicy::Pass`. Verify against `bevy_ui` 0.18 focus semantics
  (the default `FocusPolicy` for a plain `Node` may already be `Pass`, but set it
  explicitly). Covered by the manual GUI check.
- **Click vs future text-selection:** when mouse selection/forwarding is added
  later, a click will need to both focus and (maybe) start selection; 3c's
  click-to-focus is the baseline those will build on. Deferred, noted.
- **`active_pane` availability:** set by `%window-pane-changed`; on initial
  attach it should already be present (the projection seeds it). If `None`, no
  pane is treated as active → all panes dim until the first focus event;
  acceptable (and self-corrects on the first `%window-pane-changed`). Consider
  treating `active_pane == None` as "dim nothing" to avoid an all-dim flash —
  plan's call.

## Deferred scope (later phases)

- Mouse wheel/scroll forwarding and text selection into tmux panes.
- Active-pane border/highlight (chose dim-only).
- Removal of the old `ozmux_multiplexer` mouse path (`mouse_buttons.rs`) — Phase 5.
