# tmux Module Restructure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Consolidate the 11 scattered tmux-related source files into `src/tmux/` under a single `OzmuxTmuxPlugin` aggregator, and move the bootstrap session-picker to `src/picker.rs`.

**Architecture:** All `src/tmux_*.rs` files and all `src/ui/tmux_*.rs` files are relocated into `src/tmux/` via `git mv`. A new `src/tmux.rs` exposes one `pub struct OzmuxTmuxPlugin` that includes `TmuxSessionPlugin` and all sub-plugins. `src/main.rs` drops 6 `mod` declarations and 8 singleton `.add_plugins()` calls, replacing them with two entries (`OzmuxTmuxPlugin`, `OzmuxPickerPlugin`).

**Tech Stack:** Rust 1.95 / Edition 2024, Bevy 0.18, cargo check / cargo test for verification.

## Global Constraints

- No `mod.rs` files — module roots are `foo.rs` + `foo/bar.rs`.
- Comments: only `// TODO:`, `// NOTE:`, `// SAFETY:` permitted (no narrative comments).
- All code comments must be in English.
- Rust rules: mutable params first, private items last, visibility as narrow as possible.
- This is a pure mechanical refactor — zero logic changes, zero new features.
- `cargo clippy --workspace --fix --allow-dirty --allow-staged && cargo fmt` must pass.

---

## File Map

| Action | From | To |
|---|---|---|
| `git mv` | `src/tmux_render.rs` | `src/tmux/render.rs` |
| `git mv` | `src/tmux_input.rs` | `src/tmux/input.rs` |
| `git mv` | `src/tmux_mouse.rs` | `src/tmux/mouse.rs` |
| `git mv` | `src/tmux_copy_mode.rs` | `src/tmux/copy_mode.rs` |
| `git mv` | `src/tmux_pane_hit.rs` | `src/tmux/pane_hit.rs` |
| `git mv` | `src/ui/tmux_window_bar.rs` | `src/tmux/window_bar.rs` |
| `git mv` | `src/ui/tmux_window_bar_input.rs` | `src/tmux/window_bar_input.rs` |
| `git mv` | `src/ui/tmux_dialog.rs` | `src/tmux/dialog.rs` |
| `git mv` | `src/ui/tmux_divider_handle.rs` | `src/tmux/divider_handle.rs` |
| `git mv` | `src/ui/tmux_pane_focus.rs` | `src/tmux/pane_focus.rs` |
| `git mv` | `src/tmux_picker.rs` | `src/picker.rs` |
| Create | *(new)* | `src/tmux.rs` |
| Modify | `src/main.rs` | remove old mod/use/add_plugins, add new |
| Modify | `src/ui.rs` | remove 5 `pub(crate) mod tmux_*` declarations |
| Modify | `src/input/hyperlink.rs` | update `crate::tmux_pane_hit` → `crate::tmux::pane_hit` |

---

### Task 1: Move files with `git mv` and create `src/tmux.rs`

**Files:**
- Move: `src/tmux_render.rs` → `src/tmux/render.rs`
- Move: `src/tmux_input.rs` → `src/tmux/input.rs`
- Move: `src/tmux_mouse.rs` → `src/tmux/mouse.rs`
- Move: `src/tmux_copy_mode.rs` → `src/tmux/copy_mode.rs`
- Move: `src/tmux_pane_hit.rs` → `src/tmux/pane_hit.rs`
- Move: `src/ui/tmux_window_bar.rs` → `src/tmux/window_bar.rs`
- Move: `src/ui/tmux_window_bar_input.rs` → `src/tmux/window_bar_input.rs`
- Move: `src/ui/tmux_dialog.rs` → `src/tmux/dialog.rs`
- Move: `src/ui/tmux_divider_handle.rs` → `src/tmux/divider_handle.rs`
- Move: `src/ui/tmux_pane_focus.rs` → `src/tmux/pane_focus.rs`
- Move: `src/tmux_picker.rs` → `src/picker.rs`
- Create: `src/tmux.rs`

**Interfaces:**
- Produces: `src/tmux.rs` with `pub struct OzmuxTmuxPlugin` (consumed by Task 2's `main.rs` update)
- Produces: `src/picker.rs` with `pub(crate) struct OzmuxPickerPlugin` (consumed by Task 2)

- [ ] **Step 1: Create the `src/tmux/` directory and move all files**

```bash
mkdir -p src/tmux
git mv src/tmux_render.rs src/tmux/render.rs
git mv src/tmux_input.rs src/tmux/input.rs
git mv src/tmux_mouse.rs src/tmux/mouse.rs
git mv src/tmux_copy_mode.rs src/tmux/copy_mode.rs
git mv src/tmux_pane_hit.rs src/tmux/pane_hit.rs
git mv src/ui/tmux_window_bar.rs src/tmux/window_bar.rs
git mv src/ui/tmux_window_bar_input.rs src/tmux/window_bar_input.rs
git mv src/ui/tmux_dialog.rs src/tmux/dialog.rs
git mv src/ui/tmux_divider_handle.rs src/tmux/divider_handle.rs
git mv src/ui/tmux_pane_focus.rs src/tmux/pane_focus.rs
git mv src/tmux_picker.rs src/picker.rs
```

Run from: `/Users/taiga/workspace/ozmux/wt/module`

- [ ] **Step 2: Verify files are in the right places**

```bash
ls src/tmux/
```

Expected output (order may vary):
```
copy_mode.rs  dialog.rs  divider_handle.rs  input.rs  mouse.rs
pane_focus.rs  pane_hit.rs  render.rs  window_bar.rs  window_bar_input.rs
```

```bash
ls src/picker.rs
```

Expected: `src/picker.rs`

- [ ] **Step 3: Create `src/tmux.rs`**

Write the following content to `src/tmux.rs`:

```rust
//! tmux feature plugin: aggregates all tmux runtime sub-plugins.

mod copy_mode;
mod dialog;
mod divider_handle;
mod input;
mod mouse;
mod pane_focus;
pub(crate) mod pane_hit;
mod render;
mod window_bar;
mod window_bar_input;

use bevy::prelude::*;
use copy_mode::CopyModePlugin;
use dialog::DialogPlugin;
use divider_handle::DividerHandlePlugin;
use input::InputPlugin;
use mouse::MousePlugin;
use ozmux_tmux::TmuxSessionPlugin;
use pane_focus::PaneFocusPlugin;
use render::RenderPlugin;
use window_bar::WindowBarPlugin;

/// Bevy plugin aggregating all tmux runtime sub-plugins.
pub struct OzmuxTmuxPlugin;

impl Plugin for OzmuxTmuxPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            TmuxSessionPlugin,
            RenderPlugin,
            InputPlugin,
            MousePlugin,
            CopyModePlugin,
            WindowBarPlugin,
            DialogPlugin,
            DividerHandlePlugin,
            PaneFocusPlugin,
        ));
    }
}
```

Note: `pane_hit` is `pub(crate)` because `src/input/hyperlink.rs` (an external caller) imports from it.
`window_bar_input` is private (`mod`) because only `window_bar.rs` uses it via `super::`.

- [ ] **Step 4: Commit (code will NOT compile yet — that is expected)**

```bash
git add src/tmux.rs src/tmux/ src/picker.rs
git status
git commit -m "refactor(tmux): git mv all tmux modules into src/tmux/, create aggregator"
```

Expected: commit succeeds. `cargo check` will fail at this point — that is expected and will be fixed in Tasks 2–3.

---

### Task 2: Update `src/main.rs`, `src/ui.rs`, and `src/input/hyperlink.rs`

**Files:**
- Modify: `src/main.rs`
- Modify: `src/ui.rs`
- Modify: `src/input/hyperlink.rs`

**Interfaces:**
- Consumes: `pub struct OzmuxTmuxPlugin` from `src/tmux.rs` (Task 1)
- Consumes: `pub(crate) struct OzmuxPickerPlugin` from `src/picker.rs` (Task 3 will rename it)

- [ ] **Step 1: Rewrite `src/main.rs`**

Replace the entire contents of `src/main.rs` with:

```rust
//! ozmux Bevy GUI entry point.

mod bootstrap;
mod clipboard;
mod configs;
mod control_plane;
mod font;
mod inline_webview;
mod input;
mod osc_webview;
mod picker;
mod system_set;
mod theme;
mod tmux;
mod ui;
mod webview_render;

use crate::control_plane::OzmuxControlPlanePlugin;
use crate::inline_webview::OzmuxInlineWebviewPlugin;
use crate::input::hyperlink::HyperlinkInputPlugin;
use crate::osc_webview::OzmuxOscWebviewPlugin;
use crate::webview_render::{OzmuxWebviewRenderPlugin, cef_plugin};
use bevy::prelude::*;
use bootstrap::OzmuxBootstrapPlugin;
use configs::OzmuxConfigsPlugin;
use font::FontBridgePlugin;
use input::OzmuxShortcutPlugin;
use input::ime::ImePlugin;
use input::option_as_alt::OptionAsAltPlugin;
use ozma_tty_engine::TerminalHandlePlugin;
use ozma_tty_renderer::TerminalRendererPlugin;
use ozmux_webview_host::DynAssetRegistry;
use picker::OzmuxPickerPlugin;
use tmux::OzmuxTmuxPlugin;
use ui::ime_overlay::ImeOverlayPlugin;
use ui::{
    OzmuxUiPlugin, confirm_prompt::ConfirmPromptPlugin, copy_mode::CopyModePlugin,
    copy_mode_indicator::CopyModeIndicatorPlugin, copy_search::CopyPromptPlugin,
    rename_prompt::RenamePromptPlugin,
};

fn main() {
    let dyn_registry = DynAssetRegistry::default();
    App::new()
        .add_plugins((
            DefaultPlugins.set(WindowPlugin {
                primary_window: Some(Window {
                    title: "ozmux".to_string(),
                    ime_enabled: true,
                    ..default()
                }),
                ..default()
            }),
            cef_plugin(dyn_registry.clone()),
        ))
        .add_plugins((
            TerminalHandlePlugin,
            TerminalRendererPlugin,
            OzmuxTmuxPlugin,
            OzmuxPickerPlugin,
            OzmuxConfigsPlugin,
            FontBridgePlugin,
            OzmuxBootstrapPlugin,
            OzmuxShortcutPlugin,
            OzmuxUiPlugin,
            OzmuxWebviewRenderPlugin,
            CopyModePlugin,
            CopyModeIndicatorPlugin,
        ))
        .add_plugins(CopyPromptPlugin)
        .add_plugins(ConfirmPromptPlugin)
        .add_plugins(RenamePromptPlugin)
        .add_plugins((
            HyperlinkInputPlugin,
            ImePlugin,
            ImeOverlayPlugin,
            OptionAsAltPlugin,
            OzmuxOscWebviewPlugin,
            OzmuxInlineWebviewPlugin,
            OzmuxControlPlanePlugin::new(dyn_registry),
        ))
        .run();
}
```

- [ ] **Step 2: Update `src/ui.rs` — remove the five moved `mod` declarations**

The current `src/ui.rs` declares these five modules that have been moved to `src/tmux/`:
```
pub(crate) mod tmux_dialog;
pub(crate) mod tmux_divider_handle;
pub(crate) mod tmux_pane_focus;
pub(crate) mod tmux_window_bar;
pub(crate) mod tmux_window_bar_input;
```

Remove all five of those lines. The file should become:

```rust
//! Bevy UI Plugin and shared UI markers. Spawns the singleton `UiRoot` /
//! `WorkspaceUiRoot` Node tree (via `OzmuxUiRootPlugin`) that the tmux render
//! layer attaches its window container under.

use crate::ui::root::OzmuxUiRootPlugin;
use bevy::prelude::*;

pub(crate) mod confirm_prompt;
pub mod copy_mode;
pub mod copy_mode_indicator;
pub(crate) mod copy_search;
pub(crate) mod ime_overlay;
pub mod palette;
pub(crate) mod rename_prompt;
pub mod root;

/// Marker for the single root UI Node entity. Spawned once in Startup,
/// never despawned. Hosts `WorkspaceUiRoot` (the tmux window container's
/// attachment point) and the tmux window status bar (`WindowBarRoot`) as
/// direct children.
#[derive(Component)]
pub struct UiRoot;

/// Marker for the single attachment-point `Node` child of `UiRoot` under
/// which the tmux render layer parents its window container. Spawned once in
/// Startup; never despawned.
#[derive(Component)]
pub struct WorkspaceUiRoot;

/// Bevy Plugin spawning the singleton UI root Node tree.
pub struct OzmuxUiPlugin;

impl Plugin for OzmuxUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(OzmuxUiRootPlugin);
    }
}
```

- [ ] **Step 3: Update `src/input/hyperlink.rs` line 7**

In `src/input/hyperlink.rs`, find and replace line 7:

```rust
// Before:
use crate::tmux_pane_hit::{cell_at_local, tmux_pane_at_phys};

// After:
use crate::tmux::pane_hit::{cell_at_local, tmux_pane_at_phys};
```

- [ ] **Step 4: Commit**

```bash
git add src/main.rs src/ui.rs src/input/hyperlink.rs
git commit -m "refactor(tmux): update main.rs, ui.rs, hyperlink.rs for new module layout"
```

---

### Task 3: Fix plugin struct names and import paths in moved files

All 11 moved files have stale `crate::tmux_*` paths and old plugin struct names. This task fixes them all. After this task `cargo check` should succeed.

**Files:**
- Modify: `src/tmux/render.rs`
- Modify: `src/tmux/input.rs`
- Modify: `src/tmux/mouse.rs`
- Modify: `src/tmux/copy_mode.rs`
- Modify: `src/tmux/window_bar.rs`
- Modify: `src/tmux/dialog.rs`
- Modify: `src/tmux/divider_handle.rs`
- Modify: `src/tmux/pane_focus.rs`
- Modify: `src/picker.rs`
- (No changes needed: `src/tmux/pane_hit.rs`, `src/tmux/window_bar_input.rs`)

**Interfaces:**
- All plugin structs referenced by `src/tmux.rs`: `RenderPlugin`, `InputPlugin`, `MousePlugin`, `CopyModePlugin`, `WindowBarPlugin`, `DialogPlugin`, `DividerHandlePlugin`, `PaneFocusPlugin` — all `pub(crate)`.
- `src/picker.rs` exports `pub(crate) struct OzmuxPickerPlugin`.

- [ ] **Step 1: Fix `src/tmux/render.rs` — rename plugin struct only**

Find and replace (the struct name appears twice — definition and `impl`):

```rust
// Before:
pub struct OzmuxTmuxRenderPlugin;

impl Plugin for OzmuxTmuxRenderPlugin {
```

```rust
// After:
pub(crate) struct RenderPlugin;

impl Plugin for RenderPlugin {
```

No import path changes needed in this file — it has no `crate::tmux_*` imports.

- [ ] **Step 2: Fix `src/tmux/input.rs` — rename plugin + fix 2 imports**

**2a. Rename plugin struct** (two occurrences — definition and `impl`):

```rust
// Before:
pub struct OzmuxTmuxInputPlugin;

impl Plugin for OzmuxTmuxInputPlugin {
```

```rust
// After:
pub(crate) struct InputPlugin;

impl Plugin for InputPlugin {
```

**2b. Fix import on line 15** — `crate::tmux_pane_hit` → `super::pane_hit`:

```rust
// Before:
use crate::tmux_pane_hit::tmux_pane_at_phys;
```

```rust
// After:
use super::pane_hit::tmux_pane_at_phys;
```

**2c. Fix import on line 16** — `crate::tmux_picker` → `crate::picker`:

```rust
// Before:
use crate::tmux_picker::SessionPicker;
```

```rust
// After:
use crate::picker::SessionPicker;
```

- [ ] **Step 3: Fix `src/tmux/mouse.rs` — rename plugin + fix 4 imports**

**3a. Rename plugin struct** (two occurrences):

```rust
// Before:
pub(crate) struct OzmuxTmuxMousePlugin;

impl Plugin for OzmuxTmuxMousePlugin {
```

```rust
// After:
pub(crate) struct MousePlugin;

impl Plugin for MousePlugin {
```

**3b. Fix import — `crate::tmux_copy_mode` → `super::copy_mode`**:

```rust
// Before:
use crate::tmux_copy_mode::{CopyModeSnapshot, cell_at_pane, cursor_deltas};
```

```rust
// After:
use super::copy_mode::{CopyModeSnapshot, cell_at_pane, cursor_deltas};
```

**3c. Fix import — `crate::tmux_pane_hit` → `super::pane_hit`**:

```rust
// Before:
use crate::tmux_pane_hit::{cell_at_local, phys_to_pane_local, tmux_pane_at_phys};
```

```rust
// After:
use super::pane_hit::{cell_at_local, phys_to_pane_local, tmux_pane_at_phys};
```

**3d. Fix import — `crate::tmux_picker` → `crate::picker`**:

```rust
// Before:
use crate::tmux_picker::SessionPicker;
```

```rust
// After:
use crate::picker::SessionPicker;
```

**3e. Fix import — `crate::tmux_render` → `super::render`**:

```rust
// Before:
use crate::tmux_render::{DividerPixelRect, PackedTmuxLayout};
```

```rust
// After:
use super::render::{DividerPixelRect, PackedTmuxLayout};
```

- [ ] **Step 4: Fix `src/tmux/copy_mode.rs` — rename plugin + fix 2 test-only imports**

**4a. Rename plugin struct** (two occurrences):

```rust
// Before:
pub struct OzmuxTmuxCopyModePlugin;

impl Plugin for OzmuxTmuxCopyModePlugin {
```

```rust
// After:
pub(crate) struct CopyModePlugin;

impl Plugin for CopyModePlugin {
```

**4b. Fix test-only import on line ~840** (inside `#[cfg(test)] mod tests`):

```rust
// Before:
use crate::tmux_render::OzmuxTmuxRenderPlugin;
```

```rust
// After:
use super::render::RenderPlugin;
```

This replacement must be applied to **both** occurrences (lines ~840 and ~1021 — both are inside separate `#[cfg(test)]` blocks).

**4c. Update any reference to `OzmuxTmuxRenderPlugin` in tests** — after changing the import, each test block that uses `OzmuxTmuxRenderPlugin` must be updated to use `RenderPlugin`:

```rust
// Before (in test App setup):
app.add_plugins(OzmuxTmuxRenderPlugin);
```

```rust
// After:
app.add_plugins(RenderPlugin);
```

Apply this to all occurrences within `#[cfg(test)]` blocks.

- [ ] **Step 5: Fix `src/tmux/window_bar.rs` — rename plugin + fix 1 import**

**5a. Rename plugin struct** (two occurrences):

```rust
// Before:
pub struct OzmuxTmuxWindowBarPlugin;

impl Plugin for OzmuxTmuxWindowBarPlugin {
```

```rust
// After:
pub(crate) struct WindowBarPlugin;

impl Plugin for WindowBarPlugin {
```

**5b. Fix import — `crate::ui::tmux_window_bar_input` → `super::window_bar_input`**:

```rust
// Before:
use crate::ui::tmux_window_bar_input::{switch_window_on_click, window_entry_hover_cursor};
```

```rust
// After:
use super::window_bar_input::{switch_window_on_click, window_entry_hover_cursor};
```

- [ ] **Step 6: Fix `src/tmux/dialog.rs` — rename plugin only**

**6a. Rename plugin struct** (two occurrences):

```rust
// Before:
pub(crate) struct TmuxDialogPlugin;

impl Plugin for TmuxDialogPlugin {
```

```rust
// After:
pub(crate) struct DialogPlugin;

impl Plugin for DialogPlugin {
```

No import path changes needed.

- [ ] **Step 7: Fix `src/tmux/divider_handle.rs` — rename plugin + fix 2 imports**

**7a. Rename plugin struct** (two occurrences):

```rust
// Before:
pub(crate) struct OzmuxTmuxDividerHandlePlugin;

impl Plugin for OzmuxTmuxDividerHandlePlugin {
```

```rust
// After:
pub(crate) struct DividerHandlePlugin;

impl Plugin for DividerHandlePlugin {
```

**7b. Fix import — `crate::tmux_mouse::divider_at` → `super::mouse::divider_at`**:

```rust
// Before:
use crate::tmux_mouse::divider_at;
```

```rust
// After:
use super::mouse::divider_at;
```

**7c. Fix import — `crate::tmux_render` → `super::render`**:

```rust
// Before:
use crate::tmux_render::{DividerPixelRect, PackedTmuxLayout};
```

```rust
// After:
use super::render::{DividerPixelRect, PackedTmuxLayout};
```

- [ ] **Step 8: Fix `src/tmux/pane_focus.rs` — rename plugin only**

**8a. Rename plugin struct** (two occurrences):

```rust
// Before:
pub struct OzmuxTmuxPaneFocusPlugin;

impl Plugin for OzmuxTmuxPaneFocusPlugin {
```

```rust
// After:
pub(crate) struct PaneFocusPlugin;

impl Plugin for PaneFocusPlugin {
```

No import path changes needed.

- [ ] **Step 9: Fix `src/picker.rs` — rename plugin struct only**

**9a. Rename plugin struct** (two occurrences):

```rust
// Before:
pub(crate) struct OzmuxTmuxPickerPlugin;

impl Plugin for OzmuxTmuxPickerPlugin {
```

```rust
// After:
pub(crate) struct OzmuxPickerPlugin;

impl Plugin for OzmuxPickerPlugin {
```

No import path changes needed.

- [ ] **Step 10: Verify no stale `crate::tmux_` references remain**

```bash
grep -rn "crate::tmux_" src/
```

Expected: **no output** (zero matches). If any remain, fix them now before proceeding.

- [ ] **Step 11: Commit**

```bash
git add src/tmux/ src/picker.rs
git commit -m "refactor(tmux): rename plugin structs and fix import paths in moved files"
```

---

### Task 4: Verify compilation, run tests, and final cleanup

**Files:**
- No new edits expected — this task verifies Tasks 1–3 are correct and fixes any remaining compiler errors.

- [ ] **Step 1: Run `cargo check`**

```bash
cargo check 2>&1
```

Expected: `Checking ozmux-gui ...` then `Finished`. If errors appear, fix them — they will be import path issues or missed plugin renames. Common patterns:
- `error[E0432]: unresolved import` → find the stale `crate::tmux_*` path and update it to `crate::tmux::*` or `super::*`.
- `error[E0412]: cannot find type ... in this scope` → the plugin struct was renamed; update the reference.

- [ ] **Step 2: Run `cargo test`**

```bash
cargo test 2>&1
```

Expected: all tests pass. The two `#[cfg(test)]` blocks in `src/tmux/copy_mode.rs` that use `RenderPlugin` are the most likely source of test failures if Step 4c of Task 3 was incomplete.

- [ ] **Step 3: Run lint + format**

```bash
cargo clippy --workspace --fix --allow-dirty --allow-staged && cargo fmt
```

Expected: exits 0. If clippy reports warnings about unreachable `pub` items or unused imports from the refactor, fix them.

- [ ] **Step 4: Confirm final file counts**

```bash
grep "^mod " src/main.rs | wc -l
```

Expected: `14`

```bash
grep -c "add_plugins" src/main.rs
```

Expected: `4` (the two DefaultPlugins/cef tuple, the main 12-item tuple, CopyPromptPlugin singleton, ConfirmPromptPlugin singleton, RenamePromptPlugin singleton, and the final webview/input tuple — 6 calls). Adjust expectation if the number differs from the current file state.

```bash
ls src/tmux/
```

Expected: 10 files.

```bash
ls src/tmux_*.rs 2>/dev/null || echo "none"
```

Expected: `none` — no flat `tmux_*.rs` files remain at `src/`.

- [ ] **Step 5: Final commit**

```bash
git add -A
git status
git commit -m "refactor(tmux): consolidate tmux modules into src/tmux/ feature slice

- 11 files moved: 5 from src/tmux_*.rs, 5 from src/ui/tmux_*.rs, picker
- New src/tmux.rs aggregates TmuxSessionPlugin + 8 sub-plugins
- main.rs: 18->14 mod declarations, 8 singleton add_plugins removed
- Sub-plugin structs renamed (OzmuxTmux* prefix dropped, pub(crate))
- src/input/hyperlink.rs updated for new pane_hit path"
```
