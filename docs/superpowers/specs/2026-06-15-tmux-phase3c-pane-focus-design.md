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
  `bevy_picking`/`Pickable` anywhere). The repo's existing interactive nodes
  (`src/ui/tab_bar.rs`, `src/ui/tmux_window_bar.rs`) spawn **`Button`** (which
  requires `Interaction`); their click systems read
  `(&Interaction, &Marker), Changed<Interaction>` in `InputPhase::Dispatch`.
  3c follows the same pattern — the pane node gets `Button`, not bare
  `Interaction`. UI focus picks the topmost node under the cursor whose
  effective `FocusPolicy` is `Block`. **Verified in `bevy_ui 0.18.1`
  (`focus.rs:324`): a node with NO `FocusPolicy` component is treated as
  `Block`** (`unwrap_or(&FocusPolicy::Block)`) — only `FocusPolicy::default()`
  is `Pass`. So the pane node needs no `FocusPolicy` to be a click target, but
  the dim overlay child MUST carry an **explicit** `FocusPolicy::Pass` or it
  intercepts the click meant for the pane.
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

- **`augment_tmux_pane`** (Update system, targets each pane once via its own
  idempotency gate — query `TmuxPane` `With<TerminalHandle>, Without<Button>`,
  so a pane is augmented exactly once after it gets its render node; NO separate
  `PaneFocusReady` marker). For each such pane:
  - insert `Button` on the pane node (makes it a click target; `Button` requires
    `Interaction`). The pane needs no `FocusPolicy` (absent ⇒ `Block` ⇒ it
    receives clicks).
  - spawn a dim-overlay child: an absolute `Node` filling the pane
    (`top/left/right/bottom: Val::Px(0.0)`), a themed semi-transparent dark
    `BackgroundColor` (~35% black; a `src/theme` token or a documented
    constant), an **explicit** `FocusPolicy::Pass` (so it never intercepts the
    pane's click), and `Visibility::Hidden` initially.
  - store the overlay entity on the pane as a `PaneDim(Entity)` component for an
    O(1) lookup in `sync_pane_dim` (avoids a `Children` scan).
  (NOTE: the overlay is a CHILD of the pane node; verified against
  `bevy_ui_render 0.18.1` stack ordering — the child's stack index is above the
  parent's `MaterialNode`, so the overlay composites above and dims the terminal
  beneath it. It follows the pane on resize because it is sized to the parent.)
- **`focus_pane_on_click`** (`InputPhase::Dispatch`, mirroring
  `drive_tab_clicks` so the focus change is same-frame): query
  `(&Interaction, &TmuxPane), Changed<Interaction>` + `connection:
  NonSend<TmuxConnection>`. On `Interaction::Pressed`, if `connection.client()`
  is `Some`, send `select_pane_command(pane.id)`; `tracing::warn!` on error. No
  projection mutation (command-echo).
- **`sync_pane_dim`** (gated `run_if(resource_exists_and_changed::<ProjectionModel>)`):
  for each `(&TmuxPane, &PaneDim)`, look up the overlay entity and set its
  `Visibility` with `set_if_neq` to `Hidden` when `Some(pane.id) ==
  model.active_pane`, else `Visible`. **When `model.active_pane` is `None`, dim
  NOTHING** (all overlays `Hidden`) — avoids an all-panes-dim flash on attach
  before the first `%window-pane-changed`. Tolerate a pane whose overlay entity
  isn't resolvable yet (augment may run a frame later) — `get_mut` miss is a
  no-op, never `expect()`/panic.
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
  Some(PaneId(1))` and two `TmuxPane` entities (ids 1 and 2), each carrying a
  `PaneDim(overlay)` component pointing at a spawned overlay entity with a
  `Visibility`; run `sync_pane_dim`; assert pane 1's overlay is `Hidden` and
  pane 2's is `Visible`. Flip `active_pane` to `PaneId(2)`, re-run, assert the
  visibilities swap. Set `active_pane = None`, re-run, assert BOTH overlays are
  `Hidden` (dim nothing).
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
  the pane and WOULD intercept the `Interaction` by default — verified in
  `bevy_ui 0.18.1` (`focus.rs:324`), a node with no `FocusPolicy` component is
  treated as `Block`. The overlay MUST carry an **explicit** `FocusPolicy::Pass`.
  Covered by the headless test (click maps to `select_pane_command`) and the
  manual GUI check (clicking a dimmed pane focuses it).
- **Click vs future text-selection:** when mouse selection/forwarding is added
  later, a click will need to both focus and (maybe) start selection; 3c's
  click-to-focus is the baseline those will build on. Deferred, noted.
- **`active_pane` availability:** set by `%window-pane-changed`; on initial
  attach it should already be present (the projection seeds it). When `None`,
  `sync_pane_dim` dims NOTHING (decided — see Architecture), so there is no
  all-panes-dim flash before the first `%window-pane-changed`.

## Deferred scope (later phases)

- Mouse wheel/scroll forwarding and text selection into tmux panes.
- Active-pane border/highlight (chose dim-only).
- Removal of the old `ozmux_multiplexer` mouse path (`mouse_buttons.rs`) — Phase 5.
