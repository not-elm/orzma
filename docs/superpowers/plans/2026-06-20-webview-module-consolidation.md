# webview Module Consolidation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Consolidate the three flat webview modules (`src/webview_render.rs`, `src/osc_webview.rs`, `src/inline_webview.rs`) into a single `src/webview/` feature module behind an `OzmuxWebviewPlugin` aggregator, with no behavior change.

**Architecture:** Pure module restructure in the `ozmux-gui` binary crate. Move the three files (plus `render`'s `preload.rs` + `ozma_bridge.js` asset) under `src/webview/`, add a `src/webview.rs` module root that declares the three sub-modules `pub(crate)` and exposes an `OzmuxWebviewPlugin` that bundles the renamed sub-plugins. Rewrite all import paths: intra-`webview/` sibling references use `super::`, external callers use `crate::webview::…`. `control_plane.rs` and `cef_profile.rs` stay top-level.

**Tech Stack:** Rust 2024 (toolchain 1.95), Bevy 0.18, Cargo workspace. The binary is the workspace root package; `crates/` are libraries.

## Global Constraints

- **Behavior unchanged.** This is a pure restructure; the full pre-existing test suite must stay green. No logic edits.
- **No `mod.rs`.** Module roots are `foo.rs` + `foo/bar.rs` (Rust 2018 layout). `src/webview.rs` declares the `webview` module; sub-modules live in `src/webview/`.
- **Visibility minimization** (`.claude/rules/rust.md`): sub-*modules* are `pub(crate)` (each has a verified external caller); the renamed sub-plugin *structs* are `pub(super)` (only the parent `webview.rs` uses them); the `OzmuxWebviewPlugin` aggregator is `pub` (mirrors `OzmuxTmuxPlugin`).
- **Imports at the top, single contiguous block, no blank lines between groups.** No inline fully-qualified paths in signatures/bodies.
- **`Plugin::build` is one method chain** off `app.` (do not introduce new repeated `app.` statements; leave pre-existing ones in moved files untouched — out of scope).
- **Path-rewrite rule:** intra-`webview/` top-level sibling `use` → `super::<module>::…`; references inside `#[cfg(test)] mod tests` (nested) → absolute `crate::webview::<module>::…`; all callers outside `src/webview/` → `crate::webview::<module>::…`.
- **Comments in English only**; comment taxonomy `// TODO:` / `// NOTE:` / `// SAFETY:` only.
- **Do NOT touch** (name collisions / unrelated): `crates/ozma_tty_engine/src/osc_webview*`, `crates/ozmux_configs/src/osc_webview*`, the `osc_webview_gate` field name anywhere, and `OscWebviewRequest` / `OscWebviewVerb` (re-exported from `ozma_tty_engine`).

---

## File Structure

| Path | Responsibility |
|---|---|
| `src/webview.rs` | Module root: declares `pub(crate) mod {inline, osc, render};` and defines `OzmuxWebviewPlugin` |
| `src/webview/render.rs` | (was `webview_render.rs`) CEF wiring, `window.ozma` back-channel, focus sync; `RenderPlugin`, `cef_plugin` |
| `src/webview/render/preload.rs` | (was `webview_render/preload.rs`) preload-script builder |
| `src/webview/render/ozma_bridge.js` | (was `webview_render/ozma_bridge.js`) JS asset; `include_str!`'d by `preload.rs` |
| `src/webview/osc.rs` | (was `osc_webview.rs`) OSC mount/unmount observer + gate; `OscPlugin` |
| `src/webview/inline.rs` | (was `inline_webview.rs`) inline webviews; `InlinePlugin` |

Callers updated (unchanged files, paths only): `src/main.rs`, `src/control_plane.rs`, `src/input/ime.rs`, `src/tmux/input.rs`, `src/tmux/mouse.rs`, `src/tmux/render.rs`. Docs updated: `CLAUDE.md`; `docs/memo.md` deleted.

---

## Task 1: Move files into `src/webview/` and rewrite all import paths (no aggregator yet)

Leaves the tree compiling with the three plugins still registered individually in `main.rs`, just from their new paths. The aggregator + renames come in Task 2.

**Files:**
- Move: `src/webview_render.rs` → `src/webview/render.rs`
- Move: `src/webview_render/preload.rs` → `src/webview/render/preload.rs`
- Move: `src/webview_render/ozma_bridge.js` → `src/webview/render/ozma_bridge.js`
- Move: `src/osc_webview.rs` → `src/webview/osc.rs`
- Move: `src/inline_webview.rs` → `src/webview/inline.rs`
- Create: `src/webview.rs`
- Modify: `src/main.rs` (mod decls + 3 use paths)
- Modify (intra, → `super::`): `src/webview/render.rs`, `src/webview/osc.rs`, `src/webview/inline.rs`
- Modify (external, → `crate::webview::`): `src/control_plane.rs`, `src/input/ime.rs`, `src/tmux/input.rs`, `src/tmux/mouse.rs`, `src/tmux/render.rs`

**Interfaces:**
- Produces: module `crate::webview` with `pub(crate) mod {inline, osc, render}`. Reachable items keep their current names this task: `crate::webview::render::{OzmuxWebviewRenderPlugin, cef_plugin, sync_focused_webview, preload::build_dynamic_preload}`, `crate::webview::osc::{OzmuxOscWebviewPlugin, NonInteractive, OscWebviewGate, on_osc_webview_request}`, `crate::webview::inline::{OzmuxInlineWebviewPlugin, InlineWebview, focused_inline_of, inline_hit_at, inline_local_dip, PassthroughKeys}`.

- [ ] **Step 1: Move the files with `git mv`**

```bash
cd /Users/taiga/workspace/ozmux/wt/webview-module
mkdir -p src/webview/render
git mv src/webview_render/preload.rs      src/webview/render/preload.rs
git mv src/webview_render/ozma_bridge.js  src/webview/render/ozma_bridge.js
git mv src/webview_render.rs              src/webview/render.rs
git mv src/osc_webview.rs                 src/webview/osc.rs
git mv src/inline_webview.rs             src/webview/inline.rs
rmdir src/webview_render
```

- [ ] **Step 2: Create `src/webview.rs`** (module root, no aggregator yet)

```rust
//! In-process webview feature: CEF render wiring and the window.ozma Tier 1
//! back-channel (render), OSC mount/unmount of inline webviews (osc), and
//! inline webviews rendered into the terminal text flow (inline).

pub(crate) mod inline;
pub(crate) mod osc;
pub(crate) mod render;
```

- [ ] **Step 3: Update `src/main.rs` module declarations**

Remove these three lines:

```rust
mod inline_webview;
mod osc_webview;
mod webview_render;
```

Add `mod webview;` so the module list stays sorted (place it after `mod ui;`):

```rust
mod ui;
mod webview;
```

- [ ] **Step 4: Update `src/main.rs` plugin imports** (paths only; names unchanged this task)

Replace:

```rust
use crate::inline_webview::OzmuxInlineWebviewPlugin;
```
with
```rust
use crate::webview::inline::OzmuxInlineWebviewPlugin;
```

Replace:

```rust
use crate::osc_webview::OzmuxOscWebviewPlugin;
```
with
```rust
use crate::webview::osc::OzmuxOscWebviewPlugin;
```

Replace:

```rust
use crate::webview_render::{OzmuxWebviewRenderPlugin, cef_plugin};
```
with
```rust
use crate::webview::render::{OzmuxWebviewRenderPlugin, cef_plugin};
```

(The `.add_plugins((...))` registration entries `OzmuxWebviewRenderPlugin`, `OzmuxOscWebviewPlugin`, `OzmuxInlineWebviewPlugin` stay exactly as-is this task.)

- [ ] **Step 5: Rewrite intra-`webview/` sibling imports to `super::`**

In `src/webview/render.rs`, replace the two top-level `use` lines:

```rust
use crate::inline_webview::InlineWebview;
use crate::osc_webview::NonInteractive;
```
with
```rust
use super::inline::InlineWebview;
use super::osc::NonInteractive;
```

In `src/webview/render.rs`, inside `#[cfg(test)] mod tests` (the `non_interactive_webview_surface_never_takes_keyboard_focus` test), replace:

```rust
use crate::osc_webview::NonInteractive;
```
with
```rust
use crate::webview::osc::NonInteractive;
```

In `src/webview/osc.rs`, replace:

```rust
use crate::inline_webview::{
    InlineMountContext, InlineWebviewParams, mount_inline, unmount_inline,
};
```
with
```rust
use super::inline::{
    InlineMountContext, InlineWebviewParams, mount_inline, unmount_inline,
};
```

In `src/webview/inline.rs`, replace the two top-level `use` lines:

```rust
use crate::osc_webview::NonInteractive;
use crate::webview_render::preload::build_dynamic_preload;
```
with
```rust
use super::osc::NonInteractive;
use super::render::preload::build_dynamic_preload;
```

In `src/webview/inline.rs`, inside `#[cfg(test)] mod tests`, replace:

```rust
use crate::osc_webview::on_osc_webview_request;
```
with
```rust
use crate::webview::osc::on_osc_webview_request;
```

- [ ] **Step 6: Rewrite external caller imports to `crate::webview::…`**

In `src/control_plane.rs`, top-level:

```rust
use crate::inline_webview::InlineWebview;   →   use crate::webview::inline::InlineWebview;
use crate::osc_webview::NonInteractive;     →   use crate::webview::osc::NonInteractive;
```
and inside its test modules:
```rust
use crate::inline_webview::InlineWebview;        →   use crate::webview::inline::InlineWebview;
use crate::webview_render::sync_focused_webview; →   use crate::webview::render::sync_focused_webview;
```

In `src/input/ime.rs`:
```rust
use crate::inline_webview::{InlineWebview, focused_inline_of};
   →   use crate::webview::inline::{InlineWebview, focused_inline_of};
```

In `src/tmux/input.rs`:
```rust
use crate::inline_webview::{InlineWebview, PassthroughKeys, focused_inline_of, inline_hit_at};
   →   use crate::webview::inline::{InlineWebview, PassthroughKeys, focused_inline_of, inline_hit_at};
use crate::osc_webview::NonInteractive;
   →   use crate::webview::osc::NonInteractive;
```

In `src/tmux/mouse.rs`:
```rust
use crate::inline_webview::{InlineWebview, inline_hit_at, inline_local_dip};
   →   use crate::webview::inline::{InlineWebview, inline_hit_at, inline_local_dip};
use crate::osc_webview::NonInteractive;
   →   use crate::webview::osc::NonInteractive;
```

In `src/tmux/render.rs`:
```rust
use crate::osc_webview::OscWebviewGate;   →   use crate::webview::osc::OscWebviewGate;
```

- [ ] **Step 7: Verify no stale references remain**

Run:
```bash
grep -rn 'crate::inline_webview\|crate::osc_webview\|crate::webview_render\|mod inline_webview\|mod osc_webview\|mod webview_render' src/
```
Expected: **no output** (empty). Any hit is a missed rewrite — fix it before continuing. (This grep is `src/`-scoped, so the unrelated `crate::osc_webview` in `crates/` is correctly out of view.)

- [ ] **Step 8: Build**

Run: `cargo build`
Expected: compiles cleanly, no errors.

- [ ] **Step 9: Test**

Run: `cargo test`
Expected: PASS — all tests, including the relocated tests in `webview/render.rs`, `webview/render/preload.rs`, `webview/inline.rs`, and the `control_plane.rs` tests.

- [ ] **Step 10: Lint + format**

Run: `cargo clippy --workspace --all-targets && cargo fmt --check`
Expected: no clippy warnings; `fmt --check` clean. (If `fmt --check` reports diffs, run `cargo fmt` and re-run the build.)

- [ ] **Step 11: Commit**

```bash
git add -A
git commit -m "refactor(webview): move webview_render/osc_webview/inline_webview under src/webview/

Pure file move + import-path rewrite (super:: intra-module, crate::webview::
external). No behavior change; plugins still registered individually.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Introduce `OzmuxWebviewPlugin` aggregator + rename/narrow sub-plugins

Bundle the three sub-plugins behind one aggregator, drop the `Ozmux`/`Webview` prefixes, and narrow the structs to `pub(super)`.

**Files:**
- Modify: `src/webview.rs` (add aggregator)
- Modify: `src/webview/render.rs` (rename `OzmuxWebviewRenderPlugin` → `RenderPlugin`, `pub` → `pub(super)`)
- Modify: `src/webview/osc.rs` (rename `OzmuxOscWebviewPlugin` → `OscPlugin`, `pub(crate)` → `pub(super)`)
- Modify: `src/webview/inline.rs` (rename `OzmuxInlineWebviewPlugin` → `InlinePlugin`, `pub(crate)` → `pub(super)`)
- Modify: `src/main.rs` (imports + registration)

**Interfaces:**
- Consumes: `crate::webview::{render, osc, inline}` from Task 1.
- Produces: `pub struct OzmuxWebviewPlugin` at `crate::webview::OzmuxWebviewPlugin`, which `add_plugins((RenderPlugin, OscPlugin, InlinePlugin))`. `cef_plugin` remains at `crate::webview::render::cef_plugin`. The three sub-plugin structs become `pub(super)` (not referenceable outside `src/webview.rs`).

- [ ] **Step 1: Rename the plugin struct in `src/webview/render.rs`**

Replace:
```rust
pub struct OzmuxWebviewRenderPlugin;

impl Plugin for OzmuxWebviewRenderPlugin {
```
with
```rust
pub(super) struct RenderPlugin;

impl Plugin for RenderPlugin {
```

(Leave the `///` doc comment above the struct as-is — it does not name the struct.)

- [ ] **Step 2: Rename the plugin struct in `src/webview/osc.rs`**

Replace:
```rust
pub(crate) struct OzmuxOscWebviewPlugin;

impl Plugin for OzmuxOscWebviewPlugin {
```
with
```rust
pub(super) struct OscPlugin;

impl Plugin for OscPlugin {
```

- [ ] **Step 3: Rename the plugin struct in `src/webview/inline.rs`**

Replace:
```rust
pub(crate) struct OzmuxInlineWebviewPlugin;

impl Plugin for OzmuxInlineWebviewPlugin {
```
with
```rust
pub(super) struct InlinePlugin;

impl Plugin for InlinePlugin {
```

- [ ] **Step 4: Add the aggregator to `src/webview.rs`**

Replace the whole file body with:

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

- [ ] **Step 5: Update `src/main.rs` imports**

Remove these two lines:
```rust
use crate::webview::inline::OzmuxInlineWebviewPlugin;
use crate::webview::osc::OzmuxOscWebviewPlugin;
```

Replace:
```rust
use crate::webview::render::{OzmuxWebviewRenderPlugin, cef_plugin};
```
with these two lines (keep them in the existing sorted `use crate::…` block):
```rust
use crate::webview::OzmuxWebviewPlugin;
use crate::webview::render::cef_plugin;
```

- [ ] **Step 6: Update `src/main.rs` plugin registration**

In the main `.add_plugins((...))` tuple, replace the entry:
```rust
            OzmuxWebviewRenderPlugin,
```
with:
```rust
            OzmuxWebviewPlugin,
```

In the later `.add_plugins((...))` tuple, delete these two entries (the line above `OzmuxControlPlanePlugin::new(dyn_registry)` stays):
```rust
            OzmuxOscWebviewPlugin,
            OzmuxInlineWebviewPlugin,
```

- [ ] **Step 7: Verify the old plugin names are fully gone**

Run:
```bash
grep -rn 'OzmuxWebviewRenderPlugin\|OzmuxOscWebviewPlugin\|OzmuxInlineWebviewPlugin' src/
```
Expected: **no output** (the only remaining mention is a doc comment fixed in Task 3 — see Step note). If `src/webview/inline.rs` still shows `OzmuxInlineWebviewPlugin` in its `//!` header (line ~5), that is expected and handled in Task 3; everything else must be empty.

- [ ] **Step 8: Build**

Run: `cargo build`
Expected: compiles cleanly.

- [ ] **Step 9: Test**

Run: `cargo test`
Expected: PASS — all tests.

- [ ] **Step 10: Lint + format**

Run: `cargo clippy --workspace --all-targets && cargo fmt --check`
Expected: clean. (Run `cargo fmt` if needed.)

- [ ] **Step 11: Commit**

```bash
git add -A
git commit -m "refactor(webview): aggregate sub-plugins behind OzmuxWebviewPlugin

Rename OzmuxWebviewRenderPlugin/OzmuxOscWebviewPlugin/OzmuxInlineWebviewPlugin
to RenderPlugin/OscPlugin/InlinePlugin (pub(super)) and bundle them in one
OzmuxWebviewPlugin registered once in main.rs.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Documentation updates

Bring docs in line with the new module/plugin names and delete the seed memo.

**Files:**
- Modify: `src/webview/inline.rs` (`//!` header doc comment)
- Modify: `CLAUDE.md` (architecture prose + `src/` module map)
- Delete: `docs/memo.md`

**Interfaces:** none (docs/comments only).

- [ ] **Step 1: Fix the `src/webview/inline.rs` module doc comment**

In the `//!` header (top of file), replace:
```
//! `UnmountInline` arms of `osc_webview::on_osc_webview_request`, and the
//! `OzmuxInlineWebviewPlugin` runtime systems that keep `WebviewSize` in
```
with:
```
//! `UnmountInline` arms of `osc::on_osc_webview_request`, and the
//! `InlinePlugin` runtime systems that keep `WebviewSize` in
```

- [ ] **Step 2: Update `CLAUDE.md` architecture prose**

On line 14, replace `OzmuxWebviewRenderPlugin` with `OzmuxWebviewPlugin`:
```
… `OzmuxUiPlugin`, `OzmuxWebviewRenderPlugin`, `CopyModePlugin`, …
   →
… `OzmuxUiPlugin`, `OzmuxWebviewPlugin`, `CopyModePlugin`, …
```

On line 16, replace:
```
  - `OzmuxOscWebviewPlugin` (OSC 5379 `mount-inline` / `unmount-inline` of dynamically-registered webviews), `OzmuxInlineWebviewPlugin`, and `OzmuxControlPlanePlugin` (the control-socket listener that mints Tier 1 dynamic webview handles).
```
with:
```
  - and `OzmuxControlPlanePlugin` (the control-socket listener that mints Tier 1 dynamic webview handles). The in-process webview feature — CEF render wiring, the `window.ozma` back-channel, OSC 5379 `mount-inline` / `unmount-inline`, and inline webviews — is aggregated under `OzmuxWebviewPlugin` (above).
```

- [ ] **Step 3: Update the `CLAUDE.md` `src/` module map (line 41)**

Apply these three substring replacements in order:
```
`font`, `inline_webview`, `input`      →   `font`, `input`
`input`, `osc_webview`, `system_set`   →   `input`, `system_set`
`ui`, `webview_render`.                 →   `ui`, `webview`.
```
(Other stale entries on this line — e.g. `clipboard`, `tmux_*` — are pre-existing and out of scope.)

- [ ] **Step 4: Delete the seed memo**

Run:
```bash
rm -f docs/memo.md
```
(`docs/memo.md` is untracked/gitignored, so nothing to stage for it.)

- [ ] **Step 5: Sanity build + format**

Run: `cargo build && cargo fmt --check`
Expected: compiles (doc-comment change is harmless); `fmt --check` clean.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "docs(webview): update CLAUDE.md + inline module doc for the webview restructure

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Final manual verification (after Task 3)

Per the spec's Completion Criteria, the only check the automated suite cannot cover:

- Run `cargo run`, confirm the app boots and an inline webview still mounts and takes/clears focus correctly (covers the plugin-registration reorder from Task 2). This is a manual smoke test, not a commit gate.
