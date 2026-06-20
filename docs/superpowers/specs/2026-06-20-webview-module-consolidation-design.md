# webview Module Consolidation Design

**Date:** 2026-06-20
**Status:** Approved

## Problem

The in-process webview integration is spread across three flat top-level
modules in the binary crate:

- `src/webview_render.rs` (+ `webview_render/preload.rs`, `webview_render/ozma_bridge.js`) — CEF wiring, the `window.ozma` Tier 1 back-channel, and focus sync (458 lines)
- `src/osc_webview.rs` — the OSC `mount-inline` / `unmount-inline` observer and the config-driven gate (90 lines)
- `src/inline_webview.rs` — inline webviews (components, mount/unmount policy, runtime systems) (2370 lines)

These three are tightly coupled (each imports from at least one of the
others), yet they sit as unrelated siblings in `src/`. Their three Bevy
plugins are also registered in two separate `.add_plugins()` tuples in
`src/main.rs`, mirroring no feature boundary.

This follows the same motivation as the earlier
`2026-06-18-tmux-module-restructure-design.md`: consolidate a feature's
modules under one directory behind a single aggregator plugin.

## Architectural note — why the binary layer, not `ozma_terminal`

The workspace layering is:

```
ozma_tty_renderer  (base)
  ^
ozma_tty_engine    -- owns OscWebviewRequest / OscWebviewVerb (terminal-level webview signal, already here)
  ^
ozma_terminal      -- depends only on tty_engine + tty_renderer; a reusable terminal building block
  ^
ozmux-gui (binary) -- composes: + ozmux_tmux, + bevy_cef, + ozmux_webview_host, + control_plane
```

Placing the webview modules in `ozma_terminal` was considered (philosophically,
ozmux uses ozma_terminal internally) and **rejected as infeasible without a
large decoupling project**:

- The webview modules are expressed in terms of `ozmux_tmux` (`TmuxPane` /
  `ActivePane`) and the `control_plane` (`DynamicRegistry`, `WebviewOwner`,
  `ConnectionWriters`, `OzmuxRpc`, …). Those are layers **above** the terminal.
  Moving the modules down would invert the dependency direction and make
  `ozma_terminal` depend on the multiplexer + CEF + the webview host.
- `control_plane` lives in the **binary** (`src/control_plane.rs`). A crate
  cannot depend on a binary, so the move is not even expressible without first
  cratifying the control plane.

The genuinely terminal-level part of the webview feature — parsing the OSC
escape and emitting `OscWebviewRequest` / `OscWebviewVerb` — already lives in
`ozma_tty_engine`. The rest is ozmux composition glue and correctly belongs at
the binary layer. This restructure therefore keeps the modules in `src/` and
only consolidates them under `src/webview/`.

## Decision

Adopt a **feature-slice** layout: consolidate the three webview runtime
modules under `src/webview/`, expose them through a single
`OzmuxWebviewPlugin` defined in `src/webview.rs`. `src/control_plane.rs` and
`src/cef_profile.rs` stay top-level (separate subsystems; out of scope).

## File Mapping

### Files that move into `src/webview/`

| Current path | New path |
|---|---|
| `src/webview_render.rs` | `src/webview/render.rs` |
| `src/webview_render/preload.rs` | `src/webview/render/preload.rs` |
| `src/webview_render/ozma_bridge.js` | `src/webview/render/ozma_bridge.js` |
| `src/osc_webview.rs` | `src/webview/osc.rs` |
| `src/inline_webview.rs` | `src/webview/inline.rs` |

Use `git mv` to preserve history. `ozma_bridge.js` **must** move alongside
`render/preload.rs`: `preload.rs` reads it via `include_str!("ozma_bridge.js")`
(path-relative); leaving it behind breaks the build.

### New files

| Path | Contents |
|---|---|
| `src/webview.rs` | `OzmuxWebviewPlugin` aggregator + `pub(crate) mod` declarations |

### Files unchanged (explicitly NOT moved)

- `src/control_plane.rs` (+ `control_plane/listener.rs`, `control_plane/protocol.rs`) — the Unix-socket control plane is a distinct subsystem.
- `src/cef_profile.rs` — per-process CEF profile dir; process infra, not webview-feature logic.

## `src/webview.rs` Structure

```rust
//! In-process webview feature: CEF render wiring and the window.ozma Tier 1
//! back-channel (render), OSC mount/unmount of inline webviews (osc), and
//! inline webviews rendered into the terminal text flow (inline). Aggregated
//! behind OzmuxWebviewPlugin.

pub(crate) mod inline;
pub(crate) mod osc;
pub(crate) mod render;

use bevy::prelude::*;
use inline::InlinePlugin;
use osc::OscPlugin;
use render::RenderPlugin;

/// Bevy plugin aggregating the in-process webview sub-plugins.
pub struct OzmuxWebviewPlugin;

impl Plugin for OzmuxWebviewPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((RenderPlugin, OscPlugin, InlinePlugin));
    }
}
```

Sub-module visibility is `pub(crate)` because external callers (e.g.
`control_plane.rs`, `tmux/*.rs`, `input/ime.rs`) reference items by full path
(`crate::webview::render::sync_focused_webview`, etc.). `render/preload.rs`
keeps its existing `pub(crate) mod preload;` declaration inside `render.rs`.

`cef_plugin` is a free function (not a plugin) called from `main.rs` while
building `CefPlugin`; it is not part of the aggregator and stays exported from
`render`, called as `crate::webview::render::cef_plugin`.

## Plugin Naming

The three sub-plugins are referenced only within `src/webview.rs` after this
change, so they drop their `Ozmux`/`Webview` prefix and narrow to
`pub(crate)`, matching the tmux precedent:

| Old name | Old visibility | New name | New visibility |
|---|---|---|---|
| `OzmuxWebviewRenderPlugin` | `pub` | `RenderPlugin` | `pub(crate)` |
| `OzmuxOscWebviewPlugin` | `pub(crate)` | `OscPlugin` | `pub(crate)` |
| `OzmuxInlineWebviewPlugin` | `pub(crate)` | `InlinePlugin` | `pub(crate)` |
| *(new)* | — | `OzmuxWebviewPlugin` | `pub` |

No test references these plugin structs directly (tests build minimal apps with
`add_systems` / `add_observer`), so the rename touches only the definitions and
`main.rs`.

## `main.rs` Changes

`mod` declarations: **16 -> 14** (remove `inline_webview`, `osc_webview`,
`webview_render`; add `webview`).

Imports collapse to one:

```rust
// remove:
//   use crate::inline_webview::OzmuxInlineWebviewPlugin;
//   use crate::osc_webview::OzmuxOscWebviewPlugin;
//   use crate::webview_render::{OzmuxWebviewRenderPlugin, cef_plugin};
// add:
use crate::webview::OzmuxWebviewPlugin;
use crate::webview::render::cef_plugin;
```

Plugin registration: replace the `OzmuxWebviewRenderPlugin` entry (currently in
the main feature tuple, next to `OzmuxTmuxPlugin`) with `OzmuxWebviewPlugin`,
and remove the `OzmuxOscWebviewPlugin` / `OzmuxInlineWebviewPlugin` entries from
the later input-plugins tuple. The three now build adjacently in
`Render -> Osc -> Inline` order.

Ordering risk (low): this moves `Osc`/`Inline` `build()` slightly earlier
(ahead of `CopyPromptPlugin` / `ConfirmPromptPlugin` / `RenamePromptPlugin` and
the input plugins). Plugin `build()` order only matters for resources read in
another plugin's `build()` and duplicate-plugin detection — system execution is
governed by `OzmuxSystems` sets and `run_if`, not add order. The pre-existing
`Inline`-before-`ControlPlane` order is preserved. Validated by the full test
suite plus a manual run (see Completion Criteria).

## Import Path Changes

Intra-`webview/` sibling references use `super::` (matching the tmux precedent);
external callers use the full `crate::webview::...` path.

### Intra-`webview/` paths (become `super::` after the move)

| File | Before | After |
|---|---|---|
| `render.rs` | `crate::inline_webview::InlineWebview` | `super::inline::InlineWebview` |
| `render.rs` | `crate::osc_webview::NonInteractive` (body + test) | `super::osc::NonInteractive` |
| `osc.rs` | `crate::inline_webview::{InlineMountContext, InlineWebviewParams, mount_inline, unmount_inline}` | `super::inline::{…}` |
| `inline.rs` | `crate::osc_webview::NonInteractive` | `super::osc::NonInteractive` |
| `inline.rs` | `crate::webview_render::preload::build_dynamic_preload` | `super::render::preload::build_dynamic_preload` |
| `inline.rs` (test) | `crate::osc_webview::on_osc_webview_request` | `super::osc::on_osc_webview_request` |

### External callers (files NOT moving)

| File | Before | After |
|---|---|---|
| `src/control_plane.rs` | `crate::inline_webview::InlineWebview` (body + test L1121) | `crate::webview::inline::InlineWebview` |
| `src/control_plane.rs` | `crate::osc_webview::NonInteractive` | `crate::webview::osc::NonInteractive` |
| `src/control_plane.rs` (test) | `crate::webview_render::sync_focused_webview` | `crate::webview::render::sync_focused_webview` |
| `src/input/ime.rs` | `crate::inline_webview::{InlineWebview, focused_inline_of}` | `crate::webview::inline::{…}` |
| `src/tmux/input.rs` | `crate::inline_webview::{InlineWebview, PassthroughKeys, focused_inline_of, inline_hit_at}` | `crate::webview::inline::{…}` |
| `src/tmux/input.rs` | `crate::osc_webview::NonInteractive` | `crate::webview::osc::NonInteractive` |
| `src/tmux/mouse.rs` | `crate::inline_webview::{InlineWebview, inline_hit_at, inline_local_dip}` | `crate::webview::inline::{…}` |
| `src/tmux/mouse.rs` | `crate::osc_webview::NonInteractive` | `crate::webview::osc::NonInteractive` |
| `src/tmux/render.rs` | `crate::osc_webview::OscWebviewGate` | `crate::webview::osc::OscWebviewGate` |

## Documentation Updates

- Doc comments inside the moved files that name old paths/plugins:
  `inline.rs` `//!` header references `osc_webview::on_osc_webview_request`
  (-> `osc::on_osc_webview_request`) and `OzmuxInlineWebviewPlugin`
  (-> `InlinePlugin`).
- `CLAUDE.md`: the `src/` module map entry (`inline_webview`, `osc_webview`,
  `webview_render` -> `webview`) and the architecture prose naming
  `OzmuxOscWebviewPlugin` / `OzmuxInlineWebviewPlugin` / `OzmuxWebviewRenderPlugin`
  (-> `OzmuxWebviewPlugin`). Update the webview-related entries for accuracy;
  pre-existing staleness elsewhere in the map is out of scope.
- `docs/memo.md` (the original 4-line sketch that seeded this work) is
  superseded by this spec and may be deleted.

## Must NOT touch (collision guard)

These match `osc_webview` / `webview` by name but are unrelated:

- `crates/ozma_tty_engine/src/osc_webview*` — a separate crate-internal module.
- `crates/ozmux_configs/src/osc_webview*` — a separate crate-internal module.
- The `osc_webview_gate` field name (everywhere) — a field, not a module path.
- `OscWebviewRequest` / `OscWebviewVerb` re-exported from `ozma_tty_engine` —
  unchanged crate items.

## Out of Scope

- Internal refactoring of any module's logic.
- Splitting large files (e.g. `inline.rs` at 2370 lines).
- Cratifying `control_plane` or moving any module into a `crates/` library.
- The larger "decouple webview from tmux/control_plane, move to the terminal
  layer" project described in the architectural note.

## Completion Criteria

Behavior is unchanged (pure restructure). The work is done when:

1. `cargo build` succeeds.
2. `cargo test` passes — in particular the relocated tests in
   `webview/render.rs`, `webview/render/preload.rs`, `webview/inline.rs`, and
   the affected tests in `control_plane.rs`.
3. `cargo clippy --workspace` is clean.
4. `cargo fmt --check` is clean.
5. A manual `cargo run` boots the app and inline webview mount/focus still work
   (covers the plugin-registration reorder).
