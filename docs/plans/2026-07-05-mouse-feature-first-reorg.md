# Mouse input feature-first reorganization — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reorganize all mouse input code under `src/input/mouse/` in a feature-first layout (`mouse/<function>/<mode>.rs`), pulling tmux-mode mouse out of `src/input/tmux/`, while preserving behavior and unifying the duplicated webview pointer scaffolding.

**Architecture:** A pure module reorg executed as a sequence of independently-compilable moves (each ends green + committed), following the approved spec `docs/specs/2026-07-05-mouse-feature-first-reorg-design.md`. Shared symbols are held at a temporary `pub(crate)` / `pub(in crate::input)` visibility while their consumers still live outside `input::mouse`, then narrowed once every consumer is internalized. The only non-mechanical change is extracting shared webview scaffolding helpers (Task 8), done last and in isolation.

**Tech Stack:** Rust (edition 2024, toolchain 1.95), Bevy 0.18 ECS, `cargo`.

## Global Constraints

- No `mod.rs`: a module is `foo.rs` + `foo/bar.rs` (`.claude/rules/rust.md`).
- Systems/observers are registered by a `Plugin` defined in the same file; parent plugins are thin aggregators using `add_plugins`. `Plugin::build` bodies are a single method chain.
- Comments only `// TODO:` / `// NOTE:` / `// SAFETY:` (English). Every `pub` item and every module file needs a `///` / `//!` doc.
- Visibility is minimized: an item used only inside its module is private; prefer `pub(in path)` over `pub(crate)` when all callers are under `path`.
- `use` imports live at the top in one contiguous block; no inline fully-qualified paths in signatures / `.after()` / `run_if`.
- **Behavior-preserving:** every existing test must still pass unchanged in intent; no interaction behavior changes.
- Verification per task: `cargo build` then `cargo test` must succeed before committing. `cargo clippy --workspace` + `cargo fmt` run in the final task.
- Commit messages end with:
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`

---

## File Structure (end state)

```
src/surface/geometry.rs             gains topmost_surface_at (pub(crate))
src/input/mouse.rs                  MouseInputPlugin; hit-test kernel; MouseEffect IR +
                                    trigger_mouse_effects; private cell_dims; mod {gesture,button,wheel,webview};
                                    pub(crate) use button::tmux::divider_at
src/input/mouse/gesture.rs          shared primitives (was src/input/gesture.rs)
src/input/mouse/button.rs           host button dispatch (MouseButtonInputPlugin); mod tmux;
src/input/mouse/button/tmux.rs      tmux gesture gather + arbiter tmux_webview_pointer (MouseButtonTmuxPlugin)
src/input/mouse/button/tmux/decide.rs   pure deciders (divider_at)
src/input/mouse/button/tmux/apply.rs    on_tmux_mouse_effects observer (ApplyPlugin)
src/input/mouse/button/tmux/effect.rs   TmuxMouseEffect IR
src/input/mouse/wheel.rs            host wheel dispatch (MouseWheelInputPlugin); mod tmux;
src/input/mouse/wheel/tmux.rs       tmux wheel forwarding (MouseWheelTmuxPlugin) (was tmux/input.rs)
src/input/mouse/webview.rs          shared CEF routing helpers + scaffolding (MouseWebviewPlugin); mod default_mode;
src/input/mouse/webview/default_mode.rs default webview router (MouseWebviewDefaultModePlugin)
```

Deleted: `src/webview_pointer.rs`, `src/input/tmux/mouse.rs`, `src/input/tmux/mouse/*`, `src/input/default_mode/webview.rs`, `src/input/tmux/input.rs`.

`src/input/tmux/pane_hit.rs` stays; `tmux_pane_at_phys` widened to `pub(in crate::input)`.

---

## Task 1: Relocate `topmost_surface_at` to surface geometry

**Files:**
- Modify: `src/surface/geometry.rs` (add fn + its tests + `Entity` import)
- Modify: `src/webview_pointer.rs` (remove fn + its tests)
- Modify importers: `src/input/hyperlink.rs:19`, `src/input/default_mode.rs:21`, `src/input/mouse/wheel.rs:16`, `src/input/mouse/button.rs:17`, `src/input/default_mode/webview.rs:23`, `src/input/mouse.rs:300` (test)

**Interfaces:**
- Produces: `crate::surface::geometry::topmost_surface_at` — `pub(crate) fn topmost_surface_at<'a>(cursor_phys: Vec2, candidates: impl Iterator<Item = (Entity, &'a ComputedNode, &'a UiGlobalTransform)>) -> Option<Entity>` (signature unchanged from `src/webview_pointer.rs:207`).

- [ ] **Step 1: Move the function and its tests.** Cut `pub(crate) fn topmost_surface_at` (and its `///` doc) from `src/webview_pointer.rs` and paste into `src/surface/geometry.rs` (below the existing geometry fns, above `#[cfg(test)]`). Move the two tests `topmost_surface_at_picks_highest_stack_index_among_containing` and `topmost_surface_at_breaks_stack_index_ties_deterministically` from `src/input/mouse.rs`'s `#[cfg(test)] mod tests` into `src/surface/geometry.rs`'s test module. In `src/input/mouse.rs` tests, delete the now-unused `use crate::webview_pointer::topmost_surface_at;` line.

- [ ] **Step 2: Add the `Entity` import to geometry.** In `src/surface/geometry.rs`'s import block add `use bevy::ecs::entity::Entity;` (it currently imports only `bevy::math::Vec2` and `bevy::ui::{ComputedNode, UiGlobalTransform}`). Keep the import block contiguous.

- [ ] **Step 3: Repoint the 5 non-test importers.** In each file, change the import from `crate::webview_pointer::topmost_surface_at` to `crate::surface::geometry::topmost_surface_at`:
  - `src/input/hyperlink.rs:19`
  - `src/input/default_mode.rs:21`
  - `src/input/mouse/wheel.rs:16`
  - `src/input/mouse/button.rs:17`
  - `src/input/default_mode/webview.rs:23` (this line imports several names; change only `topmost_surface_at`'s path — it currently comes from `crate::webview_pointer::{... , topmost_surface_at, ...}`, so split it out: leave the other names on the `crate::webview_pointer::{...}` line and add a separate `use crate::surface::geometry::topmost_surface_at;`).

- [ ] **Step 4: Build.** Run: `cargo build`  Expected: success (no unresolved `topmost_surface_at`).

- [ ] **Step 5: Test.** Run: `cargo test topmost_surface_at`  Expected: the two relocated tests PASS. Then `cargo test` — all green.

- [ ] **Step 6: Commit.**
```bash
git add -A && git commit -m "refactor(mouse): move topmost_surface_at to surface::geometry"
```

---

## Task 2: Relocate the webview routing core to `mouse/webview.rs`

**Files:**
- Create: `src/input/mouse/webview.rs` (the routing helpers from `src/webview_pointer.rs`)
- Delete: `src/webview_pointer.rs`
- Modify: `src/main.rs:18` (remove `mod webview_pointer;`), `src/input/mouse.rs` (add `pub(crate) mod webview;`)
- Modify importers: `src/input/default_mode/webview.rs:21`, `src/input/tmux/mouse/webview.rs:16`, `src/input/tmux/input.rs:26`, `src/input/tmux/mouse.rs:458` (test)

**Interfaces:**
- Produces module `crate::input::mouse::webview` with these items at **temporary `pub(crate)`** (narrowed in Task 7): `WebviewPress`, `WebviewRouteParams`, `route_webview_left_click`, `forward_webview_move`, `release_webview_press`, `webview_wheel_target`, `webview_wheel_delta` (signatures unchanged from `src/webview_pointer.rs`). `webview_release_dip` stays private.

- [ ] **Step 1: Create the module file.** Move the entire remaining body of `src/webview_pointer.rs` (everything except `topmost_surface_at`, already moved in Task 1) into a new `src/input/mouse/webview.rs`. Update its `//!` module doc to describe "shared CEF pointer routing helpers for both the default-mode router and the tmux gesture arbiter." Keep all seven items `pub(crate)` for now.

- [ ] **Step 2: Fix the moved file's imports.** `src/input/mouse/webview.rs` still imports `crate::surface::OrzmaTerminal`, `crate::surface::geometry::phys_to_pane_local`, etc. — these `crate::` paths are unchanged by the move. Confirm no `super::`-relative imports remain (the crate-root file had none). Leave imports as-is.

- [ ] **Step 3: Declare the module and delete the old one.** In `src/input/mouse.rs` add `pub(crate) mod webview;` to the module declarations (near `mod button;` / `mod wheel;`). In `src/main.rs` delete line 18 `mod webview_pointer;`. Delete the file `src/webview_pointer.rs` (now empty of items).

- [ ] **Step 4: Repoint the 4 importers.** Change `crate::webview_pointer::` → `crate::input::mouse::webview::` in:
  - `src/input/default_mode/webview.rs:21` (the `use crate::webview_pointer::{...}` block — keep the name list, change the path)
  - `src/input/tmux/mouse/webview.rs:16`
  - `src/input/tmux/input.rs:26`
  - `src/input/tmux/mouse.rs:458` (test import of `WebviewPress`)

- [ ] **Step 5: Build.** Run: `cargo build`  Expected: success. If any file still names `crate::webview_pointer`, fix it.

- [ ] **Step 6: Test.** Run: `cargo test`  Expected: all green (webview routing tests move implicitly with their code; no test names change).

- [ ] **Step 7: Commit.**
```bash
git add -A && git commit -m "refactor(mouse): relocate webview routing core to input::mouse::webview"
```

---

## Task 3: Relocate gesture primitives to `mouse/gesture.rs`

**Files:**
- Move: `src/input/gesture.rs` → `src/input/mouse/gesture.rs`
- Modify: `src/input.rs` (remove `mod gesture;`), `src/input/mouse.rs` (add `pub(in crate::input) mod gesture;`)
- Modify importers: `src/input/mouse/button.rs:14`, `src/input/mouse/wheel.rs:12`, `src/input/tmux/mouse.rs:21`, `src/input/tmux/mouse/decide.rs:12`

**Interfaces:**
- Produces module `crate::input::mouse::gesture` (temporary `pub(in crate::input)`, narrowed to private in Task 7) re-exposing the existing `pub(crate)` items (`DragGesture`, `DragPhase`, `HeldPointer`, `OrzmaMouseGesture`, `ClickTracker`, `WheelAccumulator`, `accumulate_notches`, `lock_dominant_axis`, `wheel_delta_cells`, …) unchanged.

- [ ] **Step 1: Move the file.** Run: `git mv src/input/gesture.rs src/input/mouse/gesture.rs`

- [ ] **Step 2: Fix module declarations.** In `src/input.rs` delete `mod gesture;` (line 7). In `src/input/mouse.rs` add `pub(in crate::input) mod gesture;` to the module declarations.

- [ ] **Step 3: Repoint importers.** Change `crate::input::gesture::` → `crate::input::mouse::gesture::` in:
  - `src/input/mouse/button.rs:14`
  - `src/input/mouse/wheel.rs:12`
  - `src/input/tmux/mouse.rs:21`
  - `src/input/tmux/mouse/decide.rs:12`

- [ ] **Step 4: Build & test.** Run: `cargo build` then `cargo test`  Expected: all green.

- [ ] **Step 5: Commit.**
```bash
git add -A && git commit -m "refactor(mouse): move gesture primitives under input::mouse"
```

---

## Task 4: Relocate the default webview router to `mouse/webview/default_mode.rs`

**Files:**
- Move: `src/input/default_mode/webview.rs` → `src/input/mouse/webview/default_mode.rs`
- Modify: `src/input/default_mode.rs` (remove `mod webview;` line 12 and the `add_plugins(webview::DefaultWebviewPointerPlugin)` in `DefaultHostInputPlugin::build`), `src/input/mouse/webview.rs` (add `mod default_mode;` + `MouseWebviewPlugin`), `src/input/mouse.rs` (register `MouseWebviewPlugin`)

**Interfaces:**
- Consumes: `crate::input::mouse::webview::{WebviewPress, WebviewRouteParams, route_webview_left_click, forward_webview_move, release_webview_press, webview_wheel_target, webview_wheel_delta}` (Task 2); `crate::surface::geometry::topmost_surface_at` (Task 1).
- Produces: `MouseWebviewPlugin` (in `mouse/webview.rs`) which `init_resource::<WebviewPress>()` and `add_plugins(default_mode::MouseWebviewDefaultModePlugin)`; and `MouseWebviewDefaultModePlugin` (rename of `DefaultWebviewPointerPlugin`).

- [ ] **Step 1: Move the file.** Run: `git mv src/input/default_mode/webview.rs src/input/mouse/webview/default_mode.rs`

- [ ] **Step 2: Rename the plugin & fix imports in the moved file.** In `src/input/mouse/webview/default_mode.rs`: rename `DefaultWebviewPointerPlugin` → `MouseWebviewDefaultModePlugin` and make it `pub(in crate::input::mouse)`. Its webview-helper import `crate::input::mouse::webview::{...}` (repointed in Task 2) still resolves from the new location — leave it as the `crate::` path (no `super::` rewrite needed). `topmost_surface_at` stays `crate::surface::geometry::topmost_surface_at`. **Leave the file's local `fn cell_dims` untouched** — it keeps this task compiling; Task 7 removes it and repoints to `crate::input::mouse::cell_dims`. Update the `//!` header's cross-reference to the tmux equivalent (now `mouse::button::tmux`, not `tmux::mouse::webview`).

- [ ] **Step 3: Declare submodule + add `MouseWebviewPlugin`.** In `src/input/mouse/webview.rs` add `mod default_mode;` and define:
```rust
/// Registers the shared webview pointer resource and the per-mode webview routers.
pub(in crate::input) struct MouseWebviewPlugin;

impl Plugin for MouseWebviewPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WebviewPress>()
            .add_plugins(default_mode::MouseWebviewDefaultModePlugin);
    }
}
```
Add `use bevy::prelude::*;` to `mouse/webview.rs` if not already present. The `WebviewPress` `init_resource` moves here from the old router (remove the `init_resource::<WebviewPress>()` call from `MouseWebviewDefaultModePlugin::build`, since it is now owned by the parent).

- [ ] **Step 4: Detach from default_mode.rs.** In `src/input/default_mode.rs`: delete `mod webview;` (line 12). In `DefaultHostInputPlugin::build`, remove `.add_plugins(webview::DefaultWebviewPointerPlugin)` — re-chain the remaining calls so `maintain_input_gates` / `apply_ime_commit_to_terminal` registration is intact and the body is a single method chain starting `app.add_systems(...)`.

- [ ] **Step 5: Register under MouseInputPlugin.** In `src/input/mouse.rs`, change `MouseInputPlugin::build` to add the plugin:
```rust
app.add_plugins((MouseButtonInputPlugin, MouseWheelInputPlugin, webview::MouseWebviewPlugin))
    .init_resource::<OrzmaMouseConfig>();
```

- [ ] **Step 6: Build & test.** Run: `cargo build` then `cargo test`  Expected: all green, including `src/input/mouse/webview/default_mode.rs`'s own tests (`default_press_over_inline_rect_focuses_child`, `default_off_rect_press_clears_focus_and_records_no_press`, `default_suppressed_frame_releases_in_flight_press`).

- [ ] **Step 7: Commit.**
```bash
git add -A && git commit -m "refactor(mouse): move default webview router under input::mouse::webview"
```

---

## Task 5: Relocate the tmux gesture + arbiter to `mouse/button/tmux`

**Files:**
- Move: `src/input/tmux/mouse.rs` → `src/input/mouse/button/tmux.rs`; `src/input/tmux/mouse/decide.rs` → `src/input/mouse/button/tmux/decide.rs`; `src/input/tmux/mouse/apply.rs` → `src/input/mouse/button/tmux/apply.rs`; `src/input/tmux/mouse/effect.rs` → `src/input/mouse/button/tmux/effect.rs`
- Fold: `src/input/tmux/mouse/webview.rs` (the arbiter) into `src/input/mouse/button/tmux.rs`, then delete it
- Modify: `src/input/tmux.rs` (remove `pub(crate) mod mouse;` + `MousePlugin` from `TmuxInputPlugin`), `src/input/tmux/pane_hit.rs` (widen), `src/input/mouse/button.rs` (add `mod tmux;` + register `MouseButtonTmuxPlugin`), `src/input/mouse.rs` (re-export `divider_at`), `src/ui/tmux/divider_handle.rs:14` (repoint)

**Interfaces:**
- Consumes: `crate::input::tmux::pane_hit::tmux_pane_at_phys` (widened this task); `crate::input::mouse::webview::{route_webview_left_click, forward_webview_move, release_webview_press, WebviewPress, WebviewRouteParams}` (Task 2); `crate::input::mouse::gesture::ClickTracker` (Task 3).
- Produces: `MouseButtonTmuxPlugin` (rename of the tmux `MousePlugin`), registered by `MouseButtonInputPlugin`. `crate::input::mouse::divider_at` (re-export). The arbiter systems `tmux_webview_pointer` + `forward_tmux_webview_mouse_moves` now live in `mouse/button/tmux.rs`.

- [ ] **Step 1: Move the gesture files.**
```bash
git mv src/input/tmux/mouse.rs src/input/mouse/button/tmux.rs
git mv src/input/tmux/mouse/decide.rs src/input/mouse/button/tmux/decide.rs
git mv src/input/tmux/mouse/apply.rs src/input/mouse/button/tmux/apply.rs
git mv src/input/tmux/mouse/effect.rs src/input/mouse/button/tmux/effect.rs
```
(`git mv` a directory's files individually; the empty `src/input/tmux/mouse/` dir is removed automatically.)

- [ ] **Step 2: Fold the arbiter in.** Move the two systems `tmux_webview_pointer` and `forward_tmux_webview_mouse_moves` (and `WebviewPointerPlugin`'s registration logic) from `src/input/tmux/mouse/webview.rs` into `src/input/mouse/button/tmux.rs`. Merge `WebviewPointerPlugin`'s `add_systems` into the (renamed) `MouseButtonTmuxPlugin::build` so it registers, in one method chain: `init_resource::<TmuxMouseGesture>()`, `init_resource::<TmuxGestureButtons>()`, `tmux_webview_pointer` (`.in_set(InputPhase::Dispatch).in_set(TmuxActiveSet)`), `forward_tmux_webview_mouse_moves` (`.in_set(InputPhase::Hover).in_set(TmuxActiveSet)`), `tmux_gesture` (`.run_if(pointer_active).after(tmux_webview_pointer).in_set(InputPhase::Dispatch).in_set(TmuxActiveSet)`), and `.add_plugins(apply::ApplyPlugin)`. (`WebviewPress` is init'd by `MouseWebviewPlugin` from Task 4 — do not re-init it here.) Delete `src/input/tmux/mouse/webview.rs`.

- [ ] **Step 3: Rename the plugin & fix module declarations.** In `src/input/mouse/button/tmux.rs`: rename `MousePlugin` → `MouseButtonTmuxPlugin`, `pub(in crate::input::mouse)`. Keep `mod decide; mod apply; mod effect;` (the arbiter's `mod webview;` is gone). In `src/input/mouse/button.rs` add `pub(in crate::input::mouse) mod tmux;`.

- [ ] **Step 4: Rewrite relative imports to `crate::` paths.** In the moved files, every `super::` / `super::super::` that pointed within the old `input/tmux/` tree must become an absolute path:
  - In `mouse/button/tmux.rs` and its (folded) arbiter code: `super::pane_hit::tmux_pane_at_phys` / `super::super::pane_hit::tmux_pane_at_phys` → `crate::input::tmux::pane_hit::tmux_pane_at_phys`.
  - Webview helper imports (from the folded arbiter): `super::effect::{...}` stays (effect is still a child), but `crate::input::mouse::webview::{...}` for the routing helpers — rewrite the arbiter's `crate::webview_pointer::{...}` / repointed imports to `crate::input::mouse::webview::{...}`.
  - `cell_dims`: the moved `pub(super) fn cell_dims` in `mouse/button/tmux.rs` stays for now (removed in Task 7). Its callers inside these files keep using it.
  - In `decide.rs`: `use crate::input::gesture::ClickTracker;` is already `crate::input::mouse::gesture::ClickTracker` after Task 3 — confirm.
  - Test modules: `use crate::input::tmux::pane_hit::tmux_pane_at_phys;` → keep (that path is still valid; just widened). `use crate::webview_pointer::WebviewPress;` was repointed in Task 2 to `crate::input::mouse::webview::WebviewPress` — confirm.

- [ ] **Step 5: Widen `pane_hit`.** In `src/input/tmux.rs` change `mod pane_hit;` → `pub(in crate::input) mod pane_hit;`. In `src/input/tmux/pane_hit.rs` change `pub(super) fn tmux_pane_at_phys` → `pub(in crate::input) fn tmux_pane_at_phys`.

- [ ] **Step 6: Detach from tmux.rs + re-export divider_at.** In `src/input/tmux.rs`: remove `pub(crate) mod mouse;` and remove `MousePlugin` from `TmuxInputPlugin`'s `add_plugins` tuple (and its `use mouse::MousePlugin;`). In `src/input/mouse.rs` add `pub(crate) use button::tmux::divider_at;` (the `pub(crate) use decide::divider_at;` re-export inside `mouse/button/tmux.rs` stays, so the chain `mouse::button::tmux::divider_at` resolves). In `src/ui/tmux/divider_handle.rs:14` change `use crate::input::tmux::mouse::divider_at;` → `use crate::input::mouse::divider_at;`.

- [ ] **Step 7: Register under MouseButtonInputPlugin.** In `src/input/mouse/button.rs`, add `.add_plugins(tmux::MouseButtonTmuxPlugin)` to `MouseButtonInputPlugin::build`'s method chain.

- [ ] **Step 8: Build.** Run: `cargo build`  Expected: success. Common failures to fix: an unresolved `super::` path (rewrite to `crate::`), or `divider_at` visibility (ensure the `pub(crate) use` chain is intact).

- [ ] **Step 9: Test.** Run: `cargo test`  Expected: all green — the tmux gesture tests (`press_on_pane_focuses_and_enters_pressed`, `release_from_begun_selecting_copies`, `continuation_resizing_emits_only_on_target_change`, the `divider_at` `pixel_hit_test_*` tests, etc.) pass in their new location.

- [ ] **Step 10: Commit.**
```bash
git add -A && git commit -m "refactor(mouse): move tmux gesture + arbiter under input::mouse::button::tmux"
```

---

## Task 6: Relocate the tmux wheel forwarder to `mouse/wheel/tmux.rs`

**Files:**
- Move: `src/input/tmux/input.rs` → `src/input/mouse/wheel/tmux.rs`
- Modify: `src/input/tmux.rs` (remove `mod input;` + `InputPlugin` from `TmuxInputPlugin`), `src/input/mouse/wheel.rs` (add `mod tmux;` + register `MouseWheelTmuxPlugin`)

**Interfaces:**
- Consumes: `crate::input::tmux::pane_hit::tmux_pane_at_phys` (widened in Task 5); `crate::input::mouse::webview::{webview_wheel_target, webview_wheel_delta}` (Task 2).
- Produces: `MouseWheelTmuxPlugin` (rename of `InputPlugin`), registered by `MouseWheelInputPlugin`.

- [ ] **Step 1: Move the file.** Run: `git mv src/input/tmux/input.rs src/input/mouse/wheel/tmux.rs`

- [ ] **Step 2: Rewrite the stale module doc.** Replace the `//!` header of `src/input/mouse/wheel/tmux.rs` (which currently describes keyboard forwarding) with a wheel-only description: it forwards the mouse wheel to the active tmux pane for the cases the host `mouse::wheel` dispatcher does not own (inline webview under the pointer, copy-mode pane, alt-screen residual), accumulating sub-notch deltas into cells.

- [ ] **Step 3: Rename the plugin & fix imports.** Rename `InputPlugin` → `MouseWheelTmuxPlugin`, `pub(in crate::input::mouse)`. Rewrite imports: `use super::pane_hit::tmux_pane_at_phys;` → `use crate::input::tmux::pane_hit::tmux_pane_at_phys;`. The `crate::input::mouse::webview::{webview_wheel_delta, webview_wheel_target}` import (repointed in Task 2) — since this file now lives under `mouse`, change it to `use crate::input::mouse::webview::{webview_wheel_delta, webview_wheel_target};` (unchanged path; confirm it resolves).

- [ ] **Step 4: Detach from tmux.rs.** In `src/input/tmux.rs` remove `mod input;` and remove `InputPlugin` from `TmuxInputPlugin`'s `add_plugins` tuple (and `use input::InputPlugin;`). `TmuxInputPlugin` now aggregates only `ForwardPlugin`, `GatePlugin`, `WindowBarInputPlugin`. Update the `//!` doc of `src/input/tmux.rs` to drop "mouse gestures" / "mouse-wheel" from the description.

- [ ] **Step 5: Register under MouseWheelInputPlugin.** In `src/input/mouse/wheel.rs` add `pub(in crate::input::mouse) mod tmux;` and `.add_plugins(tmux::MouseWheelTmuxPlugin)` to `MouseWheelInputPlugin::build`.

- [ ] **Step 6: Build & test.** Run: `cargo build` then `cargo test`  Expected: all green (the wheel-owner tests `wheel_copy_mode_pane_owner_ignores_screen_and_mode_bits`, `wheel_alt_screen_*`, `aggregate_*`, `consume_notches_*` pass in the new location).

- [ ] **Step 7: Commit.**
```bash
git add -A && git commit -m "refactor(mouse): move tmux wheel forwarder under input::mouse::wheel::tmux"
```

---

## Task 7: Dedup `cell_dims` and narrow temporary visibilities

**Files:**
- Modify: `src/input/mouse.rs` (unify `cell_dims`, narrow `mod webview` / `mod gesture`), `src/input/mouse/webview.rs` (narrow helpers), `src/input/mouse/webview/default_mode.rs` (drop local `cell_dims`), `src/input/mouse/button/tmux.rs` (drop local `cell_dims`), `src/input/mouse/wheel/tmux.rs` (if it has a local cell pitch helper)

**Interfaces:**
- Produces: `crate::input::mouse::cell_dims(metrics: &TerminalCellMetricsResource) -> (f32, f32)` — **private** (no modifier); reachable by all descendants.

- [ ] **Step 1: Unify `cell_dims`.** In `src/input/mouse.rs` rename the existing private `fn cell_pitch` (line ~165) to `fn cell_dims` (keep it private, no modifier; body unchanged — floored advance/line-height clamped ≥ 1.0). Update its one internal caller in `mouse.rs`/`button.rs`/`wheel.rs` (the host dispatchers) from `cell_pitch` to `cell_dims`.

- [ ] **Step 2: Remove the duplicate copies.** Delete the local `fn cell_dims` in `src/input/mouse/webview/default_mode.rs` and the `pub(super) fn cell_dims` in `src/input/mouse/button/tmux.rs`. Repoint their callers to `crate::input::mouse::cell_dims`. If `src/input/mouse/wheel/tmux.rs` computes its own cell pitch inline, leave it unless it is a literal duplicate of `cell_dims`, in which case repoint it too.

- [ ] **Step 3: Narrow the webview module + helpers.** In `src/input/mouse.rs` change `pub(crate) mod webview;` → `mod webview;` (private). In `src/input/mouse/webview.rs` change the seven `pub(crate)` items to `pub(in crate::input::mouse)` (they are consumed only by `webview/default_mode.rs` and `button/tmux.rs`, both inside `input::mouse`).

- [ ] **Step 4: Narrow the gesture module.** In `src/input/mouse.rs` change `pub(in crate::input) mod gesture;` → `mod gesture;` (all consumers — `button`, `wheel`, `button/tmux`, `button/tmux/decide` — are now `input::mouse` descendants). Its item visibilities may stay `pub(crate)` or be narrowed to `pub(in crate::input::mouse)`; narrow them for minimality.

- [ ] **Step 5: Build.** Run: `cargo build`  Expected: success. If a `pub(in crate::input::mouse)` is too narrow for a caller, that caller is outside `input::mouse` and something in Tasks 4–6 was missed — trace and fix rather than re-widening.

- [ ] **Step 6: Test.** Run: `cargo test`  Expected: all green.

- [ ] **Step 7: Commit.**
```bash
git add -A && git commit -m "refactor(mouse): unify cell_dims and narrow mouse module visibilities"
```

---

## Task 8: Extract the shared webview scaffolding helpers

**Files:**
- Modify: `src/input/mouse/webview.rs` (add `webview_pointer_frame` + `forward_webview_move_at`)
- Modify: `src/input/mouse/webview/default_mode.rs` (thin the default pointer + move systems)
- Modify: `src/input/mouse/button/tmux.rs` (thin the tmux arbiter pointer + move systems)

**Interfaces:**
- Produces (in `mouse/webview.rs`, `pub(in crate::input::mouse)`):
  - `struct WebviewPointerFrame { scale: f32, cell_w: f32, cell_h: f32, cursor_phys: Option<Vec2> }`
  - `fn webview_pointer_frame(window: &Window, metrics: &TerminalCellMetricsResource) -> WebviewPointerFrame` — computes `scale = window.scale_factor()`, `(cell_w, cell_h) = crate::input::mouse::cell_dims(metrics)`, `cursor_phys = window.cursor_position().map(|c| c * scale)`.
  - `fn forward_webview_move_at(deps: &WebviewMoveDeps, resolve: impl Fn(Vec2) -> Option<(Entity, Vec2)>, cursor_phys: Vec2, frame: &WebviewPointerFrame)` where `WebviewMoveDeps<'a>` bundles `children: &Query<&Children>`, `webviews: &Query<(&Webview, Has<NonInteractive>)>`, `overlay_rects: &Query<&TerminalOverlays>`, `browsers: Option<&Browsers>`, `pressed_buttons: &ButtonInput<MouseButton>` (a borrowed struct, not positional args, to avoid `clippy::too_many_arguments`).

- [ ] **Step 1: Add the geometry helper.** In `src/input/mouse/webview.rs` add `WebviewPointerFrame` + `webview_pointer_frame` as specified above. Both `pub(in crate::input::mouse)`.

- [ ] **Step 2: Add the move-forward wrapper.** In `src/input/mouse/webview.rs` add `WebviewMoveDeps` + `forward_webview_move_at`, whose body resolves `(terminal, local_phys) = resolve(cursor_phys)?` then calls the existing `forward_webview_move(deps.children, deps.webviews, deps.overlay_rects, deps.browsers, deps.pressed_buttons, terminal, local_phys, frame.cell_w, frame.cell_h, frame.scale)`.

- [ ] **Step 3: Thin the default router.** In `src/input/mouse/webview/default_mode.rs`, replace the duplicated geometry-extraction block in `default_webview_pointer` with `let frame = webview_pointer_frame(window, &metrics);` (using `frame.scale`/`frame.cell_w`/`frame.cell_h`/`frame.cursor_phys`), and replace the body of `forward_default_webview_mouse_moves` with a single `forward_webview_move_at(&deps, |c| { let t = topmost_surface_at(c, surfaces.iter())?; let (_, node, transform) = surfaces.get(t).ok()?; Some((t, phys_to_pane_local(node, transform, c)?)) }, cursor_phys, &frame)` call. Keep the system's `MessageReader` ownership, every-frame execution, suppressed-frame `release_webview_press`, and the `Left`-event loop exactly as-is.

- [ ] **Step 4: Thin the tmux arbiter.** In `src/input/mouse/button/tmux.rs`, apply the same two substitutions to `tmux_webview_pointer` (geometry block → `webview_pointer_frame`) and `forward_tmux_webview_mouse_moves` (body → `forward_webview_move_at` with the `tmux_pane_at_phys(&panes, c)` resolver). Preserve exactly: the `buffer.0.clear()` at entry, the no-window/suppressed resets (`gesture.state = GestureState::Idle`, `webview_press.0 = None` / `release_webview_press`), the `SelectPane` trigger on a consumed press, and the non-consumed `buffer.0.push(*ev)` hand-off (the documented single-reader / every-frame invariant must not change).

- [ ] **Step 5: Build.** Run: `cargo build`  Expected: success. If `clippy::too_many_arguments` fires on `forward_webview_move_at`, ensure `deps` is a single struct ref, not expanded positional args.

- [ ] **Step 6: Test.** Run: `cargo test`  Expected: all green — especially the default webview tests and the tmux gesture/webview tests, which assert the preserved behavior.

- [ ] **Step 7: Commit.**
```bash
git add -A && git commit -m "refactor(mouse): extract shared webview pointer scaffolding"
```

---

## Task 9: Final verification

**Files:** none (verification + lint/format only).

- [ ] **Step 1: Clippy.** Run: `cargo clippy --workspace --all-targets`  Expected: no warnings. Fix any newly-surfaced `unreachable_pub` / visibility / import-ordering lints introduced by the moves (do not silence with `#[allow]`).

- [ ] **Step 2: Format.** Run: `cargo fmt`  Then `git diff --stat` to confirm only formatting changes.

- [ ] **Step 3: Full test.** Run: `cargo test`  Expected: all green.

- [ ] **Step 4: Grep for stragglers.** Run:
```bash
rg -n 'webview_pointer|input::tmux::mouse|input::gesture|cell_pitch' src
```
Expected: no matches (all references updated). Any hit is a missed rewrite — fix it.

- [ ] **Step 5: Smoke test the app (behavior preservation).** Run: `cargo run` and manually exercise, in both AppModes: tmux pane select / divider resize / copy-drag; inline webview click + pointer-move + wheel; terminal local selection + copy and Cmd-click hyperlink; wheel scrollback and app-forward. Confirm no regression versus `main`.

- [ ] **Step 6: Commit any lint/format fixups.**
```bash
git add -A && git commit -m "chore(mouse): clippy + fmt after feature-first reorg"
```

---

## Self-review notes

- **Spec coverage:** Tasks 1–8 map 1:1 to the spec's Migration order steps 1–8; Task 9 is spec step 9. The spec's §5 scaffolding contract → Task 8; §3 visibility table → the temporary-then-narrowed visibilities in Tasks 2/3/4/5/7; §4 encapsulation → Task 7 step 3; §6 cell_dims → Task 7 steps 1–2; the arbiter placement (§1) → Task 5 step 2.
- **Staging invariant:** every task ends with `cargo build && cargo test` green; shared symbols stay `pub(crate)` / `pub(in crate::input)` until Task 7 narrows them, so no intermediate task fails to compile.
- **Type/name consistency:** plugin renames are `MousePlugin`→`MouseButtonTmuxPlugin` (Task 5), `InputPlugin`→`MouseWheelTmuxPlugin` (Task 6), `DefaultWebviewPointerPlugin`→`MouseWebviewDefaultModePlugin` (Task 4); `MouseWebviewPlugin` new (Task 4); host `MouseButtonInputPlugin`/`MouseWheelInputPlugin` kept. `cell_pitch`→`cell_dims` (Task 7).
