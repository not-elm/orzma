# tmux Module Restructure Design

**Date:** 2026-06-18  
**Status:** Approved

## Problem

`src/main.rs` currently declares 18 top-level modules and individually registers
~15 plugins via 7 separate `.add_plugins()` calls (the latter exist only to
work around Bevy's 12-item tuple limit). The tmux-related modules are split
across two locations with no enforced boundary:

- `src/tmux_*.rs` — five flat files (render, input, mouse, copy_mode, pane_hit)
- `src/ui/tmux_*.rs` — five files mixed into the generic UI module
  (window_bar, window_bar_input, dialog, divider_handle, pane_focus)

The split is artificial: `ui/tmux_divider_handle.rs` already imports
`divider_at` from `src/tmux_mouse.rs`, so the dependency is already
bidirectional. "Layer" organization (logic vs. UI) provides no enforced
boundary in a binary crate — there is no crate split or `pub(crate)` firewall
between `src/` and `src/ui/`. Feature cohesion is the only axis that provides
real organizational benefit.

Additionally, `src/tmux_picker.rs` implements the session-chooser shown at
startup (a bootstrap-phase concern), yet it lives alongside the runtime tmux
modules.

## Decision

Adopt a **feature-slice** layout: consolidate all tmux runtime modules under
`src/tmux/`, expose them through a single `OzmuxTmuxPlugin` defined in
`src/tmux.rs`, and keep the bootstrap session picker as a separate top-level
module `src/picker.rs`.

## File Mapping

### Files that move into `src/tmux/`

| Current path | New path |
|---|---|
| `src/tmux_render.rs` | `src/tmux/render.rs` |
| `src/tmux_input.rs` | `src/tmux/input.rs` |
| `src/tmux_mouse.rs` | `src/tmux/mouse.rs` |
| `src/tmux_copy_mode.rs` | `src/tmux/copy_mode.rs` |
| `src/tmux_pane_hit.rs` | `src/tmux/pane_hit.rs` |
| `src/ui/tmux_window_bar.rs` | `src/tmux/window_bar.rs` |
| `src/ui/tmux_window_bar_input.rs` | `src/tmux/window_bar_input.rs` |
| `src/ui/tmux_dialog.rs` | `src/tmux/dialog.rs` |
| `src/ui/tmux_divider_handle.rs` | `src/tmux/divider_handle.rs` |
| `src/ui/tmux_pane_focus.rs` | `src/tmux/pane_focus.rs` |

### Files that move to top-level `src/`

| Current path | New path | Reason |
|---|---|---|
| `src/tmux_picker.rs` | `src/picker.rs` | Bootstrap lifecycle, separate from runtime |

### New files

| Path | Contents |
|---|---|
| `src/tmux.rs` | `OzmuxTmuxPlugin` aggregator |

### Files unchanged

`src/ui/` retains all non-tmux modules:
`copy_mode.rs`, `copy_mode_indicator.rs`, `copy_search.rs`,
`confirm_prompt.rs`, `rename_prompt.rs`, `ime_overlay.rs`, `palette.rs`,
`root.rs`.

`CopyModePlugin` and `CopyModeIndicatorPlugin` remain top-level in `main.rs`
because `CopyModeState` is shared with the native terminal path
(`input/ime.rs`, `ui/copy_mode_indicator.rs`) — not tmux-specific.

## `src/tmux.rs` Structure

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

`window_bar_input` and `pane_hit` have no plugin struct; they are declared as
modules and consumed by siblings via `super::`.

## Plugin Naming

Sub-plugins referenced only within `src/tmux.rs` drop the `Ozmux`/`Tmux`
prefix and narrow to `pub(crate)`:

| Old name | New name | Visibility |
|---|---|---|
| `OzmuxTmuxRenderPlugin` | `RenderPlugin` | `pub(crate)` |
| `OzmuxTmuxInputPlugin` | `InputPlugin` | `pub(crate)` |
| `OzmuxTmuxMousePlugin` | `MousePlugin` | `pub(crate)` |
| `OzmuxTmuxCopyModePlugin` | `CopyModePlugin` | `pub(crate)` |
| `TmuxDialogPlugin` | `DialogPlugin` | `pub(crate)` |
| `OzmuxTmuxDividerHandlePlugin` | `DividerHandlePlugin` | `pub(crate)` |
| `OzmuxTmuxPaneFocusPlugin` | `PaneFocusPlugin` | `pub(crate)` |
| `OzmuxTmuxWindowBarPlugin` | `WindowBarPlugin` | `pub(crate)` |
| `OzmuxTmuxPickerPlugin` | `OzmuxPickerPlugin` | `pub(crate)` |
| *(new)* | `OzmuxTmuxPlugin` | `pub` |

## `main.rs` Changes

`mod` declarations: **18 → 14** (six `tmux_*` mods removed, replaced by
`mod tmux;` and `mod picker;`).

`.add_plugins()` calls: the eight individual singleton tmux calls that existed only
to bypass Bevy's 12-tuple limit are replaced by one entry in the main tuple:

```rust
// before: 8 separate .add_plugins() calls for tmux sub-plugins
// after: one entry (TmuxSessionPlugin now lives inside OzmuxTmuxPlugin)
.add_plugins((
    TerminalHandlePlugin,
    TerminalRendererPlugin,
    OzmuxTmuxPlugin,       // entire tmux feature (includes TmuxSessionPlugin)
    OzmuxPickerPlugin,     // bootstrap chooser, separate lifecycle
    OzmuxConfigsPlugin,
    FontBridgePlugin,
    OzmuxBootstrapPlugin,
    OzmuxShortcutPlugin,
    OzmuxUiPlugin,
    OzmuxWebviewRenderPlugin,
    CopyModePlugin,
    CopyModeIndicatorPlugin,
))
```

## Import Path Changes

Within the moved files, `crate::` paths update as follows.
Intra-`tmux/` sibling references use `super::` rather than the full `crate::tmux::` path.

### Intra-`tmux/` paths (become `super::` after the move)

| Before | After (in moved file) |
|---|---|
| `crate::tmux_render::{PackedTmuxLayout, DividerPixelRect}` | `super::render::{PackedTmuxLayout, DividerPixelRect}` |
| `crate::tmux_mouse::divider_at` | `super::mouse::divider_at` |
| `crate::tmux_pane_hit::{cell_at_local, phys_to_pane_local, tmux_pane_at_phys}` | `super::pane_hit::{cell_at_local, phys_to_pane_local, tmux_pane_at_phys}` |
| `crate::tmux_copy_mode::{CopyModeSnapshot, cell_at_pane, cursor_deltas}` | `super::copy_mode::{CopyModeSnapshot, cell_at_pane, cursor_deltas}` |
| `crate::tmux_render::OzmuxTmuxRenderPlugin` (test-only) | `super::render::RenderPlugin` |
| `crate::ui::tmux_window_bar_input::…` | `super::window_bar_input::…` |

### Cross-boundary paths (become `crate::picker::` or `crate::tmux::`)

| Before | After | Affected file(s) |
|---|---|---|
| `crate::tmux_picker::SessionPicker` | `crate::picker::SessionPicker` | `tmux/mouse.rs`, `tmux/input.rs` |

### External callers (files NOT moving, but referencing tmux modules)

| File | Before | After |
|---|---|---|
| `src/input/hyperlink.rs` | `crate::tmux_pane_hit::{cell_at_local, tmux_pane_at_phys}` | `crate::tmux::pane_hit::{cell_at_local, tmux_pane_at_phys}` |

## `src/ui.rs` Changes

Remove the five `pub(crate) mod` declarations for the moved files:
`tmux_dialog`, `tmux_divider_handle`, `tmux_pane_focus`, `tmux_window_bar`,
`tmux_window_bar_input`.

## Out of Scope

- Internal refactoring of any individual module's logic
- Splitting large files (e.g. `tmux_render.rs` at 1437 lines)
- Changes to `crates/` library crates
- TypeScript workspace changes
