# Mouse input: feature-first module reorganization

## Problem

The mouse input pipeline is organized along three inconsistent axes, unlike the
keyboard/shortcuts modules that PR #239/#242 unified into a consistent
`shortcuts/{default_mode, tmux}` (feature-first, per-mode) layout:

1. **Split axis is inconsistent.** Host-shared mouse is split by *input path*
   (`mouse/button.rs`, `mouse/wheel.rs`); tmux mouse is split by *pipeline stage*
   (`tmux/mouse/{decide, apply, effect}.rs`); webview routing is split by *mode*
   (`default_mode/webview.rs` vs `tmux/mouse/webview.rs`). No single organizing
   principle.
2. **Two near-duplicate webview routers.** `default_webview_pointer` and
   `tmux_webview_pointer` (plus their move-forward siblings) duplicate a
   window/scale/cursor → suppressed-frame → per-`Left` loop skeleton; they differ
   only in surface resolution and tmux's gesture hand-off.
3. **`src/webview_pointer.rs` sits at the crate root** and mixes two concerns: the
   `topmost_surface_at` surface-arbitration primitive (used by button/wheel/
   hyperlink/default-gate) and the webview routing helpers (used only by the two
   routers).
4. **`src/input/gesture.rs` sits at the `input/` root** though it is consumed only
   by mouse (host button/wheel and tmux mouse).
5. **`cell_dims`/`cell_pitch` is defined three times** (`mouse.rs`,
   `tmux/mouse.rs`, `default_mode/webview.rs`).
6. **`src/input/tmux/input.rs` is a misnomer.** Its `InputPlugin` registers only
   the tmux mouse-wheel forwarder (`forward_wheel_to_tmux`); keyboard forwarding
   moved to `tmux/forward.rs` in an earlier refactor, but the module doc still
   describes keyboard forwarding (stale).

## Goal

Reorganize **all** mouse code under `src/input/mouse/` using a **feature-first**
layout that follows the shortcuts pattern's spirit: a function is a directory, a
mode is a leaf file (`mouse/<function>/<mode>.rs`). Pull the tmux-mode mouse code
out of `src/input/tmux/` and into `src/input/mouse/`.

The analogy to `shortcuts/` is by *naming convention*, not depth: `shortcuts/` is
two levels (`shortcuts/<mode>.rs`), while mouse is three
(`mouse/<function>/<mode>.rs`) and four for the tmux gesture's preserved
gather→decide→apply split (`mouse/button/tmux/{decide,apply,effect}.rs`). The extra
depth reflects mouse's larger surface, not a divergence from the convention.

Scope is **reorganization + internal seam cleanup**: observable behavior is
preserved, but duplicated webview scaffolding is unified and some tests are
rewritten to follow the moved code.

## Non-goals

- **No behavior change** to any mouse interaction (app reporting, local selection/
  copy, hyperlink open, pane select/resize, copy-drag, webview click/move/wheel,
  wheel scrollback/app-forward).
- **No unification of the host-shared vs tmux mouse pipelines.** They are
  complementary responsibilities that coexist, coordinated by `MouseDisabled` +
  copy-mode state + the `TmuxGestureButtons` hand-off. Merging them would be a
  re-architecture, not a reorg.
- **No change** to the engine's pure `ButtonAction`/`WheelAction` routers, the
  tmux control-mode command protocol, or the `MouseEffect`/`TmuxMouseEffect` IRs'
  semantics.
- **`tmux_pane_at_phys` (`pane_hit.rs`) stays in `src/input/tmux/`** — it is a
  shared tmux-pane primitive used by tmux keyboard forwarding and gating, not only
  by mouse.

## Design

### 1. Target structure

Function-first: each mouse *function* is a directory whose root file holds the
shared/host code, with per-mode leaf files under it.

```
src/surface/geometry.rs             + topmost_surface_at (pub(crate))   [relocated]

src/input/mouse.rs                  MouseInputPlugin (aggregates all sub-plugins);
                                    shared hit-test kernel (CellContext, cell_at_cursor,
                                    hit_candidates, cell_context_for); MouseEffect IR +
                                    trigger_mouse_effects; unified cell_dims;
                                    pub(crate) use button::tmux::divider_at (re-export)
 ├ mouse/gesture.rs                 shared primitives (ClickTracker, DragGesture, WheelAccumulator)
 ├ mouse/button.rs                  [button] host button dispatch (MouseButtonInputPlugin, shared/both modes)
 │  └ mouse/button/tmux.rs          tmux left-button gesture: the arbiter `tmux_webview_pointer` (owns the
 │     │                            single MouseButtonInput reader; routes inline clicks to the shared CEF
 │     │                            helpers in mouse/webview.rs, hands the rest off to the gesture) + the
 │     │                            gesture gather systems (MouseButtonTmuxPlugin)
 │     ├ mouse/button/tmux/decide.rs   pure deciders (divider_at defined here)
 │     ├ mouse/button/tmux/apply.rs    on_tmux_mouse_effects observer
 │     └ mouse/button/tmux/effect.rs   TmuxMouseEffect IR
 ├ mouse/wheel.rs                   [wheel] host wheel dispatch (MouseWheelInputPlugin, shared)
 │  └ mouse/wheel/tmux.rs           tmux wheel forwarding, incl. its inline webview-wheel case (MouseWheelTmuxPlugin)
 └ mouse/webview.rs                 [webview] shared, gesture-free CEF routing helpers + scaffolding;
                                    the default-mode router plugin (MouseWebviewPlugin)
    └ mouse/webview/default_mode.rs default webview router: pointer + webview-wheel (MouseWebviewDefaultModePlugin)
```

Note: the tmux webview pre-step (`tmux_webview_pointer`) lives under **`button/tmux`**, not
`webview/`, because it is the *gather stage of the tmux left-button gesture* — it owns the
tmux pointer pipeline's single `MouseButtonInput` reader, resets `GestureState`, feeds
`TmuxGestureButtons`, and triggers `SelectPane`. Co-locating it with the gesture keeps the
`TmuxMouseGesture` / `GestureState` / `TmuxMouseEffect(s)` state and IR **inside one subtree**;
only the pure CEF-forwarding helpers cross into `mouse/webview.rs`. There is therefore no
`mouse/webview/tmux.rs`. `webview` routing is deliberately not the exclusive home of webview
handling: the tmux *webview-wheel* case stays folded inside `wheel/tmux.rs`'s single-reader
`forward_wheel_to_tmux` (per the same single-reader constraint §5 invokes for the pointer).

**Deleted:** `src/webview_pointer.rs`, `src/input/tmux/mouse.rs`,
`src/input/tmux/mouse/*`, `src/input/default_mode/webview.rs`,
`src/input/tmux/input.rs`.

**Structural asymmetry is intentional.** `button`/`wheel` have only a `tmux`
submodule (their host dispatchers are shared and run in both modes via
`MouseDisabled` gating; Default mode adds nothing button/wheel-specific). `webview`
has only a `default_mode` leaf: the Default shell's webview clicks/wheel have no
gesture to arbitrate against, so they get a standalone router, whereas tmux's webview
*pointer* is absorbed by the gesture arbiter in `button/tmux` and its webview *wheel*
by `wheel/tmux` (both bound to tmux's single-`MouseButtonInput`/`MouseWheel` reader
pipelines). `mouse/webview.rs` therefore holds the shared CEF core plus the one
default router.

**Plugin tree** (follows `ShortcutsPlugin` → `ShortcutsDefaultModePlugin` +
`ShortcutsTmuxModePlugin`): `MouseInputPlugin` aggregates the existing
`MouseButtonInputPlugin` and `MouseWheelInputPlugin` (kept by name to avoid an
unscheduled rename) plus `MouseWebviewPlugin`; `MouseButtonInputPlugin` aggregates
`MouseButtonTmuxPlugin`, `MouseWheelInputPlugin` aggregates `MouseWheelTmuxPlugin`,
and `MouseWebviewPlugin` aggregates `MouseWebviewDefaultModePlugin`. Each aggregates
with `add_plugins`, per the repo's "register in the defining file, parent is a thin
aggregator" rule.

### 2. `src/input/tmux/input.rs` → `mouse/wheel/tmux.rs` (whole-file move)

`tmux/input.rs` is already wheel-only (keyboard forwarding lives in
`tmux/forward.rs`). The move is therefore whole-file: relocate to
`mouse/wheel/tmux.rs`, rewrite the stale module doc to describe wheel forwarding
only, and rename `InputPlugin` → `MouseWheelTmuxPlugin`. Remove `mod input;` and
`InputPlugin` from `src/input/tmux.rs` (`TmuxInputPlugin` then aggregates only
`ForwardPlugin`, `GatePlugin`, `WindowBarInputPlugin`).

Because this module's `webview_wheel_target`/`webview_wheel_delta` consumer now
lives *inside* `input::mouse`, the webview wheel helpers no longer need to be
reachable from `input::tmux` (see §4).

### 3. Visibility decisions

| Item | Decision | Rationale |
| --- | --- | --- |
| `input/mouse/webview.rs` module | `mod webview;` (private to `input::mouse`); routing helpers `pub(in crate::input::mouse)` | The consumers are the default router (`webview/default_mode.rs`, a descendant of `webview`, which could reach even private items) and the tmux gesture arbiter (`button/tmux.rs`, **not** a descendant of `webview`), so the helpers need `pub(in crate::input::mouse)` — the module itself stays private since only `input::mouse` descendants name it. |
| `topmost_surface_at` | `surface/geometry.rs`, `pub(crate)` | Used beyond mouse (hyperlink hover, the Default input gate); belongs with `cell_at_pane`/`phys_to_pane_local`. |
| `tmux_pane_at_phys` / `pane_hit` | `pub(in crate::input)` (widened from `pub(super)`; NOT `pub(crate)`) | All callers (tmux `gate`, relocated `mouse::button::tmux` and `mouse::wheel::tmux`) are under `crate::input`; the visibility ladder prefers `pub(in path)` over `pub(crate)`. |
| `divider_at` | Re-exported from `input::mouse` (`pub(crate) use button::tmux::divider_at;`); UI import becomes `crate::input::mouse::divider_at` | `ui::tmux::divider_handle` is outside `input::mouse` (in `crate::ui`), so a `pub(crate)` re-export of the one symbol is narrower than making the whole `mouse::button::tmux` subtree `pub(crate)`. |
| `TmuxMouseGesture`, `GestureState`, `TmuxGestureButtons`, `TmuxMouseEffect(s)`, `tmux_webview_pointer` | unchanged visibility — stay within `button/tmux` | Because the arbiter is co-located with the gesture under `button/tmux` (§1), the gesture state machine, its effect IR, and the hand-off buffer never cross a module boundary; they need **no** widening. This is the omission both spec reviews flagged, resolved by placement rather than by widening four types across function directories. |

### 4. Webview-routing-helper encapsulation

The webview routing helpers (`WebviewPress`, `WebviewRouteParams`,
`route_webview_left_click`, `forward_webview_move`, `release_webview_press`,
`webview_wheel_target`, `webview_wheel_delta`) currently live in
`src/webview_pointer.rs`. Their consumers are exactly four files, and **all four
land inside `input::mouse` after this reorg**:

- `default_mode/webview.rs` → `mouse/webview/default_mode.rs`
- `tmux/mouse/webview.rs` (the arbiter `tmux_webview_pointer` + move-forward) → `mouse/button/tmux.rs`
- `tmux/mouse.rs` → `mouse/button/tmux.rs`
- `tmux/input.rs` → `mouse/wheel/tmux.rs`

`tmux/gate.rs` references `webview_pointer` only in a doc comment (no import).
Therefore `mouse/webview.rs` is a **private** module (`mod webview;`); its routing
helpers are `pub(in crate::input::mouse)` because one consumer (the tmux arbiter in
`button/tmux.rs`) is not a descendant of `webview` (§3). `topmost_surface_at` is the
only symbol from the old crate-root module with consumers outside `input::mouse`; it
is relocated to `surface/geometry.rs` (§3).

### 5. Webview scaffolding unification (the one non-mechanical change)

The two pointer systems (`webview/default_mode.rs` and the tmux arbiter
`tmux_webview_pointer` in `button/tmux.rs`) duplicate a per-frame skeleton. The
unification **keeps each mode's pointer/move system as a full Bevy system** — each
owns its own `MessageReader<MouseButtonInput>`, runs every frame, and performs its
mode-specific suppressed-frame mutation itself. This
preserves the documented tmux invariant that the pointer system must own the
single `MouseButtonInput` reader and run every frame (gating it with `run_if`
would freeze the reader cursor and re-read a stale press when the pointer
reactivates).

Shared helpers extracted into `mouse/webview.rs`:

- **Relocated as-is:** the seven routing helpers above.
- **`webview_pointer_frame(window, metrics) -> { scale, cell_w, cell_h, cursor_phys }`**
  — collapses the three duplicated geometry-extraction blocks (uses the unified
  `cell_dims`).
- **`forward_webview_move_at(deps, resolve: impl Fn(Vec2) -> Option<(Entity, Vec2)>, …)`**
  — the two `forward_*_webview_mouse_moves` systems become one call each,
  differing only by the surface resolver. `deps` is bundled as a single borrowed
  struct (the inline queries + `Browsers` + held buttons), not a long positional
  list, so the helper does not re-trigger `clippy::too_many_arguments` (which the
  existing `forward_webview_move` / `route_webview_left_click` already carry an
  `#[expect]` for).

What stays per-mode (the thin adapter, ~20–30 lines each):

1. **surface resolver** — Default: `topmost_surface_at(cursor_phys, surfaces)` +
   `phys_to_pane_local`; tmux: `tmux_pane_at_phys(panes, cursor_phys)`.
2. **suppressed-frame reset** — tmux clears `TmuxGestureButtons` and resets the
   gesture to `Idle`; both call `release_webview_press`.
3. **per-`Left` outcome** — Default: none (routing handles focus internally);
   tmux: on a consumed press trigger `SelectPane`, on a non-consumed event push it
   to `TmuxGestureButtons` for `tmux_gesture`.

`tmux_gesture` and the arbiter `tmux_webview_pointer` now live in the **same file**
(`mouse/button/tmux.rs`), so the `tmux_gesture.after(tmux_webview_pointer)` ordering and
the `TmuxGestureButtons` hand-off are same-module — no cross-directory visibility or
ordering seam. The only cross-directory call from the arbiter is into
`mouse/webview.rs`'s `pub(in crate::input::mouse)` routing helpers for the CEF
forwarding itself.

### 6. `cell_dims` dedup

The three copies collapse into one `cell_dims(metrics) -> (f32, f32)` in
`mouse.rs` (advance/line-height, floored, clamped to ≥ 1.0). It needs **no**
visibility modifier: `mouse/wheel/tmux.rs`, `mouse/button/tmux/*`, and the webview
modules are all module *descendants* of `mouse.rs`, and Rust grants a descendant
access to a private ancestor item, so a bare `fn cell_dims` is reachable from all of
them (the narrowest option, matching the visibility-minimization rule). This differs
from the cross-*sibling* seam in §3, which genuinely requires widening.

## Migration order

Each step compiles and keeps the test suite green before the next, so the branch
is bisectable and every step is independently reviewable:

   Throughout: a moved file's `super::` / `super::super::` relative imports must be
   rewritten to absolute `crate::` paths for the new location (e.g. the tmux webview
   `super::super::pane_hit::tmux_pane_at_phys` → `crate::input::tmux::pane_hit::tmux_pane_at_phys`).

1. Move `topmost_surface_at` → `surface/geometry.rs` (add the `Entity` import it
   needs); update its ~6 importers.
2. Relocate the webview routing core `webview_pointer.rs` → `mouse/webview.rs`;
   delete the crate-root module (`mod webview_pointer;` in `main.rs`); update
   importers. Keep the helpers at their current `pub(crate)` visibility for now —
   their consumers are still outside `input::mouse` until steps 5–7, so narrowing to
   `pub(in crate::input::mouse)` happens in step 8, not here.
3. Move `input/gesture.rs` → `mouse/gesture.rs` (update `mod gesture;` from
   `input.rs` to `mouse.rs`; fix `button.rs`/`wheel.rs`/`decide.rs` imports).
4. Dedup `cell_dims` into `mouse.rs`.
5. Move `default_mode/webview.rs` → `mouse/webview/default_mode.rs`; **remove
   `mod webview;` and `add_plugins(webview::DefaultWebviewPointerPlugin)` from
   `input/default_mode.rs`**; register `MouseWebviewDefaultModePlugin` under
   `MouseWebviewPlugin`.
6. Move the tmux gesture (`tmux/mouse.rs` + `decide`/`apply`/`effect`) →
   `mouse/button.rs` subtree, **and fold `tmux/mouse/webview.rs`'s arbiter
   (`tmux_webview_pointer` + move-forward) into `mouse/button/tmux.rs`** (not a
   separate `webview/tmux.rs`); remove `mod mouse;` + `MousePlugin` from
   `input/tmux.rs`; widen `pane_hit` to `pub(in crate::input)`; re-export
   `divider_at` from `input::mouse` and update the `ui::tmux::divider_handle` import.
7. Move `tmux/input.rs` → `mouse/wheel/tmux.rs` (rewrite the module doc, rename
   `InputPlugin` → `MouseWheelTmuxPlugin`); remove `mod input;` + `InputPlugin`
   from `input/tmux.rs`.
8. Extract the shared webview scaffolding helpers (§5); thin the default router and
   the tmux arbiter. Now that every routing-helper consumer is inside `input::mouse`,
   narrow the helpers from `pub(crate)` to `pub(in crate::input::mouse)` and make
   `mod webview;` private.
9. `cargo build && cargo test && cargo clippy --workspace && cargo fmt`.

## Test updates

- Test modules move with their code; assertions are unchanged in intent. The
  mechanical fixups are import paths: e.g. `use crate::webview_pointer::topmost_surface_at`
  (in `mouse.rs` tests) → `crate::surface::geometry::topmost_surface_at`;
  `use crate::webview_pointer::WebviewPress` (in the tmux gesture tests) →
  `crate::input::mouse::webview::WebviewPress`.
- The default webview router's own test module moves with it to
  `mouse/webview/default_mode.rs`.
- After §5, the two routers' tests are consolidated where the shared helpers'
  behavior is asserted once; the mode-specific outcome (tmux `SelectPane`/buffer,
  suppressed-frame reset) keeps a per-mode test.

## Verification

Behavior-preserving, so the moved test suites staying green is the primary
evidence. Manual smoke test in both `AppMode`s: tmux pane select / divider resize
/ copy-drag; inline webview click, pointer-move, and wheel; terminal local
selection + copy and Cmd-click hyperlink open; wheel scrollback and app-forward
reporting.

## Considered alternatives

- **Mode-first layout** (`input/mouse/` for host-shared + `input/tmux/mouse/` for
  tmux). Rejected: the user wants feature-first, matching `shortcuts/`. Mode-first
  would keep the tmux mouse code split away from the host mouse code it parallels.
- **Deferring the tmux wheel forwarder to a second PR.** Rejected: pulling
  `tmux/input.rs` into `mouse/wheel/tmux.rs` now is what makes the wheel function
  symmetric and is the change that fully encapsulates the webview helpers (§4).
- **Moving `tmux_pane_at_phys` to `surface/geometry.rs`** alongside
  `topmost_surface_at`. Rejected: `geometry.rs` is deliberately mode-agnostic;
  `pane_hit` depends on `ozmux_tmux::{PaneId, TmuxPane}`, so the move would couple
  a low-level geometry module to the tmux crate.
- **Merging the two webview routers into one system** parameterized by a resolver.
  Rejected: they need different `SystemParam` queries (`OzmaTerminal` surfaces vs
  `TmuxPane`s), different run gating, and each must own its `MouseButtonInput`
  reader every frame. Sharing scaffolding helpers (§5) captures the dedup without
  fighting Bevy's system-parameter model.
- **Keeping the tmux webview pre-step under `webview/tmux.rs`** (a sibling of
  `button/tmux`), the earlier draft. Rejected on both spec reviews' advice: the
  pre-step is the *gather stage of the tmux gesture* — it owns the reader, resets
  `GestureState`, feeds `TmuxGestureButtons`, and triggers `SelectPane` — so a
  `webview/` home would force `TmuxMouseGesture` / `GestureState` /
  `TmuxMouseEffect(s)` to widen to `pub(in crate::input::mouse)` and cross two
  function directories. Co-locating the arbiter with the gesture in `button/tmux`
  keeps that state and IR private to one subtree; only the pure CEF-forwarding
  helpers cross into `mouse/webview.rs`.

## Risks

- **Cross-directory coupling is minimal by construction.** Placing the arbiter with
  the gesture in `button/tmux` (§1) keeps `TmuxGestureButtons`, `TmuxMouseGesture`,
  `GestureState`, and the effect IR inside one subtree; the only cross-directory
  dependency is `button/tmux` → `mouse/webview.rs`'s `pub(in crate::input::mouse)`
  CEF-routing helpers. Behavior is unchanged.
- **The webview scaffolding extraction (§5) is the only non-mechanical change.**
  Mitigated by keeping each mode router a full system that owns its reader and
  suppressed-frame mutation, so the extracted helpers carry no per-frame state.
- **Import churn across ~12+ files.** Mechanical and compiler-caught.
