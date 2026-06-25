# Pane Black-Screen Paint Recovery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop tmux panes from rendering fully black after a layout change (split / window resize / copy-mode entry) by making the seed→route→paint pipeline self-healing.

**Architecture:** Four independent fixes from the design spec. (C1) `route_tmux_output` buffers a pane's bytes when its handle/grid is not yet present and replays them once ready, instead of dropping them. (C2) a binary-side rescue system detects a structurally-unpainted grid via a pure sentinel and asks the tmux crate to re-`capture-pane` (debounced, in-flight-suppressed) until it paints. (C3) the copy-mode refresh loop gains a capture-in-flight resend and records `last_scroll` on *reply* (not send) so a lost capture is retried. (C4) the renderer paints an unpainted pane's padding with the theme background instead of pure black.

**Tech Stack:** Rust (edition 2024, toolchain 1.95), Bevy 0.18 ECS, `alacritty_terminal` VT, tmux `-CC` control mode. Crates touched: `ozma_tty_renderer`, `ozmux_tmux` (crate `tmux_session`), and the root binary `ozmux` (`src/`).

**Spec:** `docs/superpowers/specs/2026-06-26-pane-black-paint-recovery-design.md`

## Global Constraints

- Comments: only `// TODO:` / `// NOTE:` (critical caveat) / `// SAFETY:`. All comments in English. (`.claude/rules/rust.md`)
- No `mod.rs`. Every `pub` item gets a `///` doc; every module file gets a `//!`.
- Visibility: start private; widen only for a real out-of-module caller. New items used in one module stay private.
- All `use` at the top of the file in one contiguous block; no inline fully-qualified paths in signatures/bodies.
- `Plugin::build` bodies use a single method chain off `app`.
- Register systems/observers in the `Plugin` defined in the SAME file; parents only `add_plugins`.
- Mutable params before immutable in fn signatures (except `self` / `On<E>` first, or a documented semantic order).
- Gate whole-system change checks with `run_if`, not in-body `is_changed()` early returns.
- Do NOT use `set_changed()` / `bypass_change_detection()`; mutate conditionally so `DerefMut` drives change detection.
- `Query` params: descriptive noun, never a `_q` suffix.
- TDD: write the failing test first, watch it fail, implement minimally, watch it pass, commit. Frequent commits.
- Every commit message ends with the trailer:
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- Lint gate before each commit: `cargo clippy --workspace --all-targets` clean and `cargo fmt`.

## File Structure

- `crates/ozma_tty_renderer/src/material.rs` — **modify**: add a `padding_color` pure helper + `TerminalPaddingFallback` resource; consult it in `update_terminal_material`. Export the resource.
- `crates/ozma_tty_renderer/src/lib.rs` — **modify**: `pub use` the new resource.
- `src/tmux/render.rs` — **modify**: add `PendingPaneOutput` resource + buffering/replay in `route_tmux_output`; insert `TerminalPaddingFallback` from the theme; register the rescue plugin's ordering.
- `src/tmux/paint_rescue.rs` — **create**: `grid_needs_full_seed` pure helper, `PaneSeedDebounce` resource, `rescue_unpainted_panes` system, `PaintRescuePlugin`.
- `src/tmux.rs` — **modify**: declare `mod paint_rescue;` and `add_plugins(PaintRescuePlugin)`.
- `crates/tmux_session/src/output.rs` — **modify**: add the `RequestPaneReseed` message.
- `crates/tmux_session/src/plugin.rs` — **modify**: register the message + a `handle_pane_reseed_requests` system that calls `request_pane_capture`.
- `crates/tmux_session/src/lib.rs` — **modify**: `pub use` `RequestPaneReseed`.
- `src/tmux/copy_mode.rs` — **modify**: add `decide_capture` pure helper + `capture_in_flight` bookkeeping; record `last_scroll` on capture reply; age/resend lost captures.

The five tasks are independent and can be reviewed separately. Task 4 depends on Task 3's helper; the rest have no ordering dependency.

---

### Task 1: Component 4 — non-black padding fallback (renderer)

**Files:**
- Modify: `crates/ozma_tty_renderer/src/material.rs`
- Modify: `crates/ozma_tty_renderer/src/lib.rs`
- Test: inline `#[cfg(test)] mod tests` in `crates/ozma_tty_renderer/src/material.rs`

**Interfaces:**
- Produces: `pub struct TerminalPaddingFallback([u8; 3])` (resource; `Default` = `[0,0,0]`), and a private `fn padding_color(default_bg: [u8; 3], fallback: [u8; 3]) -> Vec4`.
- Consumes: nothing.

**Background:** `update_terminal_material` currently computes `bg_padding_color` straight from `grid.default_bg`, which is `[0,0,0]` until a snapshot carries OSC 11 (`crates/ozma_tty_renderer/src/schema/grid.rs:46`). An unpainted grid therefore paints pure black. We map an unset `[0,0,0]` to a configurable fallback. The fallback resource defaults to `[0,0,0]` so behaviour is unchanged until the binary sets it.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests { ... }` block in `crates/ozma_tty_renderer/src/material.rs` (add `use super::*;` if not already present in that block):

```rust
#[test]
fn padding_color_falls_back_when_default_bg_is_black() {
    let got = padding_color([0, 0, 0], [30, 32, 40]);
    let c = Color::srgb_u8(30, 32, 40).to_linear();
    assert_eq!(got, Vec4::new(c.red, c.green, c.blue, 1.0));
}

#[test]
fn padding_color_uses_default_bg_when_set() {
    let got = padding_color([10, 20, 30], [99, 99, 99]);
    let c = Color::srgb_u8(10, 20, 30).to_linear();
    assert_eq!(got, Vec4::new(c.red, c.green, c.blue, 1.0));
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p ozma_tty_renderer padding_color`
Expected: FAIL — `cannot find function `padding_color` in this scope`.

- [ ] **Step 3: Add the resource and the pure helper**

Add near the other resources/types in `crates/ozma_tty_renderer/src/material.rs`:

```rust
/// Padding colour used for the area outside a terminal grid (and the whole
/// quad while a grid is unpainted) when the terminal has no OSC 11 default
/// background. Defaults to black (the prior behaviour); the binary sets it to
/// the theme background so a momentarily-unpainted pane is not pure black.
#[derive(Resource)]
pub struct TerminalPaddingFallback(pub [u8; 3]);

impl Default for TerminalPaddingFallback {
    fn default() -> Self {
        Self([0, 0, 0])
    }
}
```

Add the private helper alongside the other free functions in the same file:

```rust
fn padding_color(default_bg: [u8; 3], fallback: [u8; 3]) -> Vec4 {
    let [r, g, b] = if default_bg == [0, 0, 0] {
        fallback
    } else {
        default_bg
    };
    let c = Color::srgb_u8(r, g, b).to_linear();
    Vec4::new(c.red, c.green, c.blue, 1.0)
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p ozma_tty_renderer padding_color`
Expected: PASS (2 tests).

- [ ] **Step 5: Wire the helper + resource into the renderer**

In `crates/ozma_tty_renderer/src/material.rs`, add the resource param to `update_terminal_material` (immutable params group — place after the existing `Res`/`ResMut` params, keeping mutable-first ordering):

```rust
    fallback: Res<TerminalPaddingFallback>,
```

Replace the existing `bg_padding_color` block (currently `let bg_padding_color = { let [r, g, b] = grid.default_bg; let c = Color::srgb_u8(r, g, b).to_linear(); Vec4::new(c.red, c.green, c.blue, 1.0) };`) with:

```rust
        let bg_padding_color = padding_color(grid.default_bg, fallback.0);
```

In the same file's `TerminalMaterialPlugin` `build` (the plugin that registers `update_terminal_material`), add to the method chain:

```rust
            .init_resource::<TerminalPaddingFallback>()
```

In `crates/ozma_tty_renderer/src/lib.rs`, export it next to the other material re-exports:

```rust
pub use material::TerminalPaddingFallback;
```

- [ ] **Step 6: Set the fallback from the theme in the binary**

In `src/tmux/render.rs`, add to imports (in the existing top `use` block):

```rust
use ozma_tty_renderer::TerminalPaddingFallback;
```

In `RenderPlugin::build` (`src/tmux/render.rs:38`), add to the `app` method chain (after `.insert_resource(ClearColor(theme::PANE_GAP))`):

```rust
            .insert_resource(TerminalPaddingFallback(theme_background_bytes()))
```

Add this private helper to `src/tmux/render.rs` (bottom, with the other free fns):

```rust
fn theme_background_bytes() -> [u8; 3] {
    let s = theme::BACKGROUND.to_srgba();
    [
        (s.red * 255.0).round() as u8,
        (s.green * 255.0).round() as u8,
        (s.blue * 255.0).round() as u8,
    ]
}
```

- [ ] **Step 7: Verify the workspace builds and lints**

Run: `cargo build -p ozma_tty_renderer && cargo clippy -p ozma_tty_renderer --all-targets && cargo fmt`
Expected: builds clean, no clippy warnings.

- [ ] **Step 8: Commit**

```bash
git add crates/ozma_tty_renderer/src/material.rs crates/ozma_tty_renderer/src/lib.rs src/tmux/render.rs
git commit -m "$(cat <<'EOF'
feat(renderer): theme-background padding fallback for unpainted grids

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Component 1 — buffer & replay routed pane output (binary)

**Files:**
- Modify: `src/tmux/render.rs`
- Test: inline `#[cfg(test)] mod tests` in `src/tmux/render.rs`

**Interfaces:**
- Produces: `struct PendingPaneOutput` (resource) with `fn push(&mut self, pane: PaneId, data: &[u8])` and `fn take(&mut self, pane: PaneId) -> Option<Vec<u8>>`, and `const PENDING_PANE_OUTPUT_CAP: usize`.
- Consumes: `PaneOutput { pane: PaneId, data: Vec<u8> }`, `TerminalHandle`, `TmuxPane` (already imported in `render.rs`).

**Background:** `route_tmux_output` (`src/tmux/render.rs:165`) drops a pane's bytes when `entity_of.get(&pane)` misses or `handles.get_mut(entity)` errors (`continue`). For a `capture-pane` reply that arrives before the pane entity/handle is flushed, the authoritative seed bytes are lost and the grid stays black until the next unrelated `%output`. We buffer the bytes (bounded) and replay them once the pane is ready.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests { ... }` block in `src/tmux/render.rs` (create the block if absent; add `use super::*;` and `use tmux_control_parser::PaneId;`):

```rust
#[test]
fn pending_output_accumulates_then_takes() {
    let mut p = PendingPaneOutput::default();
    p.push(PaneId(1), b"ab");
    p.push(PaneId(1), b"cd");
    p.push(PaneId(2), b"xy");
    assert_eq!(p.take(PaneId(1)).as_deref(), Some(&b"abcd"[..]));
    assert_eq!(p.take(PaneId(1)), None);
    assert_eq!(p.take(PaneId(2)).as_deref(), Some(&b"xy"[..]));
}

#[test]
fn pending_output_drops_pane_buffer_over_cap() {
    let mut p = PendingPaneOutput::default();
    let big = vec![0u8; PENDING_PANE_OUTPUT_CAP];
    p.push(PaneId(1), &big);
    p.push(PaneId(1), b"z");
    let got = p.take(PaneId(1)).unwrap_or_default();
    assert!(
        got.len() <= PENDING_PANE_OUTPUT_CAP,
        "buffer must not grow past the cap"
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test pending_output`
Expected: FAIL — `cannot find type `PendingPaneOutput``.

- [ ] **Step 3: Add the resource**

Add to `src/tmux/render.rs` (near `LastClientSize`), and ensure `use std::collections::HashMap;` is present (it already is):

```rust
/// Total cap, across all panes, for bytes buffered while a pane's handle/grid
/// is not yet present. One MiB is far above any real capture seed; the cap only
/// guards a pane that never attaches.
const PENDING_PANE_OUTPUT_CAP: usize = 1 << 20;

/// Bytes routed to a pane before its `TerminalHandle` was queryable, held until
/// the pane is ready and then replayed. Prevents losing the authoritative
/// `capture-pane` seed (spec Component 1).
#[derive(Resource, Default)]
struct PendingPaneOutput {
    buf: HashMap<PaneId, Vec<u8>>,
    total: usize,
}

impl PendingPaneOutput {
    fn push(&mut self, pane: PaneId, data: &[u8]) {
        if self.total + data.len() > PENDING_PANE_OUTPUT_CAP {
            if let Some(old) = self.buf.remove(&pane) {
                self.total -= old.len();
            }
            tracing::warn!(
                pane = pane.0,
                "pending pane-output over cap; dropped buffered bytes"
            );
            if data.len() > PENDING_PANE_OUTPUT_CAP {
                return;
            }
        }
        let entry = self.buf.entry(pane).or_default();
        entry.extend_from_slice(data);
        self.total += data.len();
    }

    fn take(&mut self, pane: PaneId) -> Option<Vec<u8>> {
        let v = self.buf.remove(&pane)?;
        self.total -= v.len();
        Some(v)
    }
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test pending_output`
Expected: PASS (2 tests).

- [ ] **Step 5: Register the resource**

In `RenderPlugin::build` (`src/tmux/render.rs:38`), add to the `app` chain:

```rust
            .init_resource::<PendingPaneOutput>()
```

- [ ] **Step 6: Buffer-and-replay inside `route_tmux_output`**

Add the resource param to `route_tmux_output` (mutable params group, after `mut commands` / `mut reader` / `mut handles`):

```rust
    mut pending: ResMut<PendingPaneOutput>,
```

Replace the per-pane loop body (`src/tmux/render.rs:180-207`) so that unresolved panes buffer and ready panes drain. The new body of the function, from after `let entity_of = ...;`:

```rust
    let mut pane_ids: std::collections::HashSet<PaneId> = by_pane.keys().copied().collect();
    pane_ids.extend(pending.buf.keys().copied());
    for pane in pane_ids {
        let fresh = by_pane.remove(&pane).unwrap_or_default();
        let Some(&entity) = entity_of.get(&pane) else {
            pending.push(pane, &fresh);
            continue;
        };
        let Ok((mut handle, mut title)) = handles.get_mut(entity) else {
            pending.push(pane, &fresh);
            continue;
        };
        if let Some(buffered) = pending.take(pane) {
            handle.advance(&buffered);
        }
        handle.advance(&fresh);
        handle.drain_control_events(&mut commands, entity, &mut title);
        if copy_modes.get(entity).is_err() {
            handle.flush_emit(&mut commands, entity);
        }
        let _ = handle.take_replies();
    }
```

Add `use std::collections::HashSet;` to the top `use` block (do NOT inline the path — replace the inline `std::collections::HashSet` above with `HashSet` after adding the import).

- [ ] **Step 7: Verify build, lint, and the existing render tests**

Run: `cargo test -p ozmux render && cargo clippy --all-targets && cargo fmt`
Expected: builds clean; pre-existing `route_tmux_output` tests (if any) still pass.

- [ ] **Step 8: Commit**

```bash
git add src/tmux/render.rs
git commit -m "$(cat <<'EOF'
fix(tmux): buffer & replay pane output instead of dropping it before attach

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: `grid_needs_full_seed` pure sentinel helper (binary)

**Files:**
- Create: `src/tmux/paint_rescue.rs`
- Modify: `src/tmux.rs`
- Test: inline `#[cfg(test)] mod tests` in `src/tmux/paint_rescue.rs`

**Interfaces:**
- Produces: `fn grid_needs_full_seed(grid_cols: u16, grid_rows: u16, cells_len: usize, handle_cols: u16, handle_rows: u16) -> bool`.
- Consumes: nothing (pure).

**Background:** The detector for an unpainted grid is structural (spec §2.2/§3.2): the **dims-vs-handle** clause catches the common `0×0` grid; `cells.len() != rows` catches the dims-written-but-cells-empty variant (e.g. a lost resize snapshot). A genuinely blank captured pane yields `cells.len() == rows` (a snapshot builds one row vector per visible row), so it must NOT fire.

- [ ] **Step 1: Write the failing test**

Create `src/tmux/paint_rescue.rs` with only the module doc, the function stub's test, and `mod tests`:

```rust
//! Structural rescue for tmux panes whose grid was left unpainted after a
//! layout change: detects the unpainted state and asks `ozmux_tmux` to
//! re-`capture-pane` until the grid paints (spec Component 2).

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_grid_against_sized_handle_needs_seed() {
        assert!(grid_needs_full_seed(0, 0, 0, 80, 24));
    }

    #[test]
    fn dims_written_but_cells_empty_needs_seed() {
        assert!(grid_needs_full_seed(80, 24, 0, 80, 24));
    }

    #[test]
    fn blank_captured_pane_does_not_need_seed() {
        // A real snapshot yields one (possibly empty) row vector per row.
        assert!(!grid_needs_full_seed(80, 24, 24, 80, 24));
    }

    #[test]
    fn painted_matching_grid_does_not_need_seed() {
        assert!(!grid_needs_full_seed(80, 24, 24, 80, 24));
    }
}
```

Declare the module in `src/tmux.rs` (add next to the other `mod` lines):

```rust
mod paint_rescue;
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p ozmux grid_needs_full_seed`
Expected: FAIL — `cannot find function `grid_needs_full_seed``.

- [ ] **Step 3: Implement the helper**

Add to `src/tmux/paint_rescue.rs` above the `#[cfg(test)]` block:

```rust
/// Returns whether a pane's grid is structurally unpainted and needs a full
/// re-seed. The dims-vs-handle clause catches the common `0×0` grid; the
/// `cells_len != rows` clause catches a grid whose dims were written but whose
/// rows were never repopulated (e.g. a lost resize snapshot). A genuinely blank
/// captured pane has `cells_len == rows`, so it does not fire.
fn grid_needs_full_seed(
    grid_cols: u16,
    grid_rows: u16,
    cells_len: usize,
    handle_cols: u16,
    handle_rows: u16,
) -> bool {
    (grid_cols, grid_rows) != (handle_cols, handle_rows) || cells_len != grid_rows as usize
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p ozmux grid_needs_full_seed`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add src/tmux/paint_rescue.rs src/tmux.rs
git commit -m "$(cat <<'EOF'
feat(tmux): pure grid_needs_full_seed sentinel for paint rescue

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: Component 2 — reseed request message + rescue system

**Files:**
- Modify: `crates/tmux_session/src/output.rs` (add the message)
- Modify: `crates/tmux_session/src/plugin.rs` (register + handle it)
- Modify: `crates/tmux_session/src/lib.rs` (export it)
- Modify: `src/tmux/paint_rescue.rs` (debounce helper + system + plugin)
- Modify: `src/tmux.rs` (add the plugin)
- Test: inline `#[cfg(test)] mod tests` in `src/tmux/paint_rescue.rs`

**Interfaces:**
- Produces (crate): `pub struct RequestPaneReseed { pub pane: PaneId }` (a Bevy `Message`).
- Produces (binary): `fn should_emit_reseed(counter: &mut u8, needs_seed: bool, threshold: u8) -> bool`; `struct PaneSeedDebounce` resource; `fn rescue_unpainted_panes(...)`; `pub(crate) struct PaintRescuePlugin`.
- Consumes: `grid_needs_full_seed` (Task 3); `TmuxPane`, `TerminalHandle`, `TerminalGrid`, `CopyModeState`; crate `request_pane_capture` machinery (via the message).

**Background:** The binary owns the renderer-component read (`TerminalGrid`); the crate owns the `capture-pane` reply correlation (renderer-free). So the binary computes the sentinel and emits `RequestPaneReseed { pane }`; the crate re-uses `request_pane_capture` (with its existing `panes_with_cursor_pending` in-flight suppression). A binary-side per-pane counter debounces, so the 1-frame resize transient (dims written before the deferred snapshot flush, spec §3.2) does not trigger a capture; only a state persisting `threshold` frames does.

- [ ] **Step 1: Write the failing test (debounce helper)**

Add to the `#[cfg(test)] mod tests` block in `src/tmux/paint_rescue.rs`:

```rust
#[test]
fn debounce_emits_only_after_threshold_consecutive_true() {
    let mut c = 0u8;
    assert!(!should_emit_reseed(&mut c, true, 3)); // 1
    assert!(!should_emit_reseed(&mut c, true, 3)); // 2
    assert!(should_emit_reseed(&mut c, true, 3)); // 3 -> emit
}

#[test]
fn debounce_resets_on_false() {
    let mut c = 0u8;
    should_emit_reseed(&mut c, true, 3);
    should_emit_reseed(&mut c, true, 3);
    assert!(!should_emit_reseed(&mut c, false, 3)); // reset
    assert_eq!(c, 0);
    assert!(!should_emit_reseed(&mut c, true, 3)); // counting restarts at 1
}

#[test]
fn debounce_does_not_re_emit_every_frame_while_held() {
    let mut c = 0u8;
    for _ in 0..2 {
        should_emit_reseed(&mut c, true, 3);
    }
    assert!(should_emit_reseed(&mut c, true, 3)); // emits at threshold
    assert!(!should_emit_reseed(&mut c, true, 3)); // held: no re-emit until reset
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p ozmux should_emit_reseed`
Expected: FAIL — `cannot find function `should_emit_reseed``.

- [ ] **Step 3: Implement the debounce helper**

Add to `src/tmux/paint_rescue.rs` (above the test block):

```rust
/// Advances a per-pane debounce counter and returns whether to emit a reseed
/// request this frame. Emits exactly once when `needs_seed` has held for
/// `threshold` consecutive frames; a `false` resets the counter; once emitted it
/// will not re-emit until the counter resets (the saturated value stays above
/// `threshold`, and only the exact `== threshold` transition emits).
fn should_emit_reseed(counter: &mut u8, needs_seed: bool, threshold: u8) -> bool {
    if !needs_seed {
        *counter = 0;
        return false;
    }
    *counter = counter.saturating_add(1);
    *counter == threshold
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p ozmux should_emit_reseed`
Expected: PASS (3 tests).

- [ ] **Step 5: Add the crate message**

In `crates/tmux_session/src/output.rs`, add (with the other message/types; keep imports at the top):

```rust
/// Request from the renderer layer (the binary) to re-`capture-pane`-seed a
/// pane whose grid was left structurally unpainted after a layout change. The
/// crate handles it via `request_pane_capture`, reusing the existing reply
/// correlation and in-flight suppression.
#[derive(Message)]
pub struct RequestPaneReseed {
    pub pane: PaneId,
}
```

Ensure `use bevy::prelude::*;` and `use tmux_control_parser::PaneId;` are present at the top of `output.rs` (add if missing).

In `crates/tmux_session/src/lib.rs`, extend the `output` re-export line to:

```rust
pub use output::{PaneOutput, RequestPaneReseed};
```

- [ ] **Step 6: Register + handle the message in the crate plugin**

In `crates/tmux_session/src/plugin.rs`, import the message (top `use` block; extend the existing `crate::output` use):

```rust
use crate::output::{PaneOutput, RequestPaneReseed, collect_pane_outputs};
```

In `TmuxSessionPlugin::build` (`crates/tmux_session/src/plugin.rs:44`), add the message registration in the chain (next to `.add_message::<PaneOutput>()`):

```rust
            .add_message::<RequestPaneReseed>()
```

and add the handler to the post-projection systems tuple (`plugin.rs:67-72`), so it becomes:

```rust
            .add_systems(
                Update,
                (
                    request_pane_captures,
                    recapture_settled_panes,
                    handle_pane_reseed_requests.run_if(on_message::<RequestPaneReseed>),
                )
                    .after(TmuxProjectionSet)
                    .run_if(any_with_component::<TmuxClient>),
            );
```

Add the handler fn to `plugin.rs` (private; near `recapture_settled_panes`):

```rust
/// Re-seeds each requested pane via `request_pane_capture`, skipping panes that
/// already have a capture/cursor pair in flight (so a repeated request while the
/// reply is pending does not duplicate the command).
fn handle_pane_reseed_requests(
    mut client: Single<(&mut TmuxClient, &mut EnumerationState)>,
    mut requests: MessageReader<RequestPaneReseed>,
) {
    let (client, enumeration) = &mut *client;
    for req in requests.read() {
        if enumeration.panes_with_cursor_pending.contains(&req.pane) {
            continue;
        }
        request_pane_capture(client, enumeration, req.pane);
    }
}
```

Confirm `MessageReader` and `on_message` are in scope via `use bevy::prelude::*;` (already present).

- [ ] **Step 7: Verify the crate builds and lints**

Run: `cargo test -p ozmux_tmux && cargo clippy -p ozmux_tmux --all-targets && cargo fmt`
Expected: builds clean; crate tests pass.

- [ ] **Step 8: Write the rescue system + plugin (binary)**

Add to `src/tmux/paint_rescue.rs` (above the test block). Put the imports at the top of the file, in one block under the `//!`:

```rust
use crate::ui::copy_mode::CopyModeState;
use bevy::prelude::*;
use ozma_tty_engine::TerminalHandle;
use ozma_tty_renderer::schema::TerminalGrid;
use ozmux_tmux::{PaneId, RequestPaneReseed, TmuxPane, TmuxProjectionSet};
use std::collections::HashMap;
```

```rust
/// Frames the unpainted state must persist before a reseed is requested. Filters
/// the 1-frame resize transient (dims written before the deferred snapshot
/// flush) while still healing a genuinely lost seed quickly.
const RESEED_DEBOUNCE_FRAMES: u8 = 3;

/// Per-pane consecutive-frames-unpainted counters for the reseed debounce.
#[derive(Resource, Default)]
struct PaneSeedDebounce(HashMap<PaneId, u8>);

/// Wires the structural paint-rescue system after the tmux projection chain.
pub(crate) struct PaintRescuePlugin;

impl Plugin for PaintRescuePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PaneSeedDebounce>()
            .add_systems(
                Update,
                rescue_unpainted_panes
                    .after(TmuxProjectionSet)
                    .in_set(super::TmuxActiveSet),
            );
    }
}

/// Requests a tmux re-seed for each non-copy-mode pane whose grid is
/// structurally unpainted (see [`grid_needs_full_seed`]) once the state has held
/// for [`RESEED_DEBOUNCE_FRAMES`]. Copy-mode panes are skipped — they paint via
/// the separate `CopyRenderHandle` (Component 3).
fn rescue_unpainted_panes(
    mut debounce: ResMut<PaneSeedDebounce>,
    mut reseed: MessageWriter<RequestPaneReseed>,
    panes: Query<(&TmuxPane, &TerminalHandle, &TerminalGrid), Without<CopyModeState>>,
) {
    for (pane, handle, grid) in panes.iter() {
        let (h_cols, h_rows, _) = handle.read_geometry();
        let needs = grid_needs_full_seed(grid.cols, grid.rows, grid.cells.len(), h_cols, h_rows);
        let counter = debounce.0.entry(pane.id).or_default();
        if should_emit_reseed(counter, needs, RESEED_DEBOUNCE_FRAMES) {
            reseed.write(RequestPaneReseed { pane: pane.id });
        }
    }
}
```

- [ ] **Step 9: Register the plugin**

In `src/tmux.rs`, import and add the plugin to `OzmuxTmuxPlugin`'s `add_plugins` tuple (next to `RenderPlugin`):

```rust
use paint_rescue::PaintRescuePlugin;
```

```rust
                RenderPlugin,
                PaintRescuePlugin,
```

- [ ] **Step 10: Verify build, lint, all tests**

Run: `cargo test -p ozmux paint_rescue && cargo build && cargo clippy --all-targets && cargo fmt`
Expected: builds clean; helper tests pass.

- [ ] **Step 11: Commit**

```bash
git add crates/tmux_session/src/output.rs crates/tmux_session/src/plugin.rs crates/tmux_session/src/lib.rs src/tmux/paint_rescue.rs src/tmux.rs
git commit -m "$(cat <<'EOF'
fix(tmux): structurally rescue unpainted panes via debounced reseed

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 5: Component 3 — copy-mode capture backstop + last_scroll fix

**Files:**
- Modify: `src/tmux/copy_mode.rs`
- Test: inline `#[cfg(test)] mod tests` in `src/tmux/copy_mode.rs`

**Interfaces:**
- Produces: `enum CaptureDecision { Skip, Issue }` and `fn decide_capture(last_captured_scroll: Option<u32>, capture_in_flight: bool, scroll: u32) -> CaptureDecision`.
- Consumes / modifies: `CopyRefreshState` (adds a `capture_in_flight: HashMap<PaneId, CapturePending>` field), `apply_state_reply`, `apply_capture_reply`, `issue_copy_state`.

**Background:** Today `apply_state_reply` records `last_scroll` at **send** time (`src/tmux/copy_mode.rs:265`) and gates the capture on `last_scroll != scroll`. So a lost capture at the same scroll position permanently suppresses any retry, and there is no capture-in-flight resend (only the State query resends). We (a) record the captured scroll on the **capture reply**, (b) track an in-flight capture per pane, and (c) age it so a lost capture is retried.

- [ ] **Step 1: Write the failing test (decision helper)**

Add to the `#[cfg(test)] mod tests` block in `src/tmux/copy_mode.rs` (add `use super::*;`):

```rust
#[test]
fn decide_capture_issues_for_new_scroll() {
    assert_eq!(decide_capture(None, false, 5), CaptureDecision::Issue);
    assert_eq!(decide_capture(Some(3), false, 5), CaptureDecision::Issue);
}

#[test]
fn decide_capture_skips_when_already_captured_that_scroll() {
    assert_eq!(decide_capture(Some(5), false, 5), CaptureDecision::Skip);
}

#[test]
fn decide_capture_skips_while_in_flight() {
    // A capture for this scroll is already pending; do not duplicate.
    assert_eq!(decide_capture(None, true, 5), CaptureDecision::Skip);
}

#[test]
fn decide_capture_reissues_after_lost_capture_cleared() {
    // last_captured stayed stale (reply never landed) and the in-flight entry
    // was aged out; the next state reply must re-issue.
    assert_eq!(decide_capture(Some(3), false, 5), CaptureDecision::Issue);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p ozmux decide_capture`
Expected: FAIL — `cannot find function `decide_capture``.

- [ ] **Step 3: Implement the decision helper + in-flight type**

Add to `src/tmux/copy_mode.rs`:

```rust
/// Whether a `State` reply should trigger a fresh `capture-pane`.
#[derive(Debug, PartialEq, Eq)]
enum CaptureDecision {
    Skip,
    Issue,
}

/// Per-pane in-flight capture bookkeeping: the scroll position the pending
/// capture is for, and how many updates it has been outstanding (for the resend
/// backstop).
struct CapturePending {
    scroll: u32,
    age: u32,
}

/// Decides whether to issue a capture for `scroll`: issue when we have not yet
/// captured that scroll position and no capture for it is already in flight.
fn decide_capture(
    last_captured_scroll: Option<u32>,
    capture_in_flight: bool,
    scroll: u32,
) -> CaptureDecision {
    if capture_in_flight || last_captured_scroll == Some(scroll) {
        CaptureDecision::Skip
    } else {
        CaptureDecision::Issue
    }
}
```

Extend `CopyRefreshState` (`src/tmux/copy_mode.rs:61`) with the in-flight map (rename `last_scroll`'s role to "last *captured* scroll"):

```rust
#[derive(Resource, Default)]
struct CopyRefreshState {
    state_in_flight: HashMap<PaneId, u32>,
    last_scroll: HashMap<PaneId, u32>,
    capture_in_flight: HashMap<PaneId, CapturePending>,
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p ozmux decide_capture`
Expected: PASS (4 tests).

- [ ] **Step 5: Record the captured scroll on the *reply*, not the send**

In `apply_state_reply` (`src/tmux/copy_mode.rs:229`), replace the `changed`/send block (currently lines 251-270) with the in-flight-aware version:

```rust
    let in_flight = refresh.capture_in_flight.contains_key(&reply.pane);
    let last = refresh.last_scroll.get(&reply.pane).copied();
    if decide_capture(last, in_flight, state.scroll_position) == CaptureDecision::Skip {
        return;
    }
    let Some(client) = client else {
        return;
    };
    match client.send(CopyModeCapture {
        pane: reply.pane,
        scroll_position: state.scroll_position,
        pane_height: state.pane_height,
    }) {
        Ok(id) => {
            queries.register(id, reply.pane, CopyQueryKind::Capture);
            refresh.capture_in_flight.insert(
                reply.pane,
                CapturePending {
                    scroll: state.scroll_position,
                    age: 0,
                },
            );
        }
        Err(error) => tracing::warn!(?error, pane = reply.pane.0, "copy-capture send failed"),
    }
```

In `apply_capture_reply` (`src/tmux/copy_mode.rs:277`), add a `refresh: &mut CopyRefreshState` parameter (mutable-first, before `render_handles` is mutable too — place `refresh` first), and on a successful reply move the pending scroll into `last_scroll`. Change the signature and the success path:

```rust
fn apply_capture_reply(
    commands: &mut Commands,
    refresh: &mut CopyRefreshState,
    render_handles: &mut Query<&mut CopyRenderHandle>,
    pane_entity: Entity,
    reply: &CopyModeReply,
) {
    if !reply.ok {
        return;
    }
    if let Some(pending) = refresh.capture_in_flight.remove(&reply.pane) {
        refresh.last_scroll.insert(reply.pane, pending.scroll);
    }
    let bytes = capture_to_bytes(&reply.output);
    // ... unchanged from here ...
```

Update the `apply_capture_reply` call site in `consume_copy_reply` (`src/tmux/copy_mode.rs:213`) to pass `&mut refresh`:

```rust
                apply_capture_reply(&mut commands, &mut refresh, &mut render_handles, entity, reply);
```

- [ ] **Step 6: Age out a lost capture so it is retried**

In `issue_copy_state` (`src/tmux/copy_mode.rs:127`), after the existing per-pane state-query loop, age the capture-in-flight entries and drop stale ones so the next `State` reply re-issues (since `last_scroll` was not advanced for a lost capture). Add at the end of the function body:

```rust
    refresh.capture_in_flight.retain(|pane, pending| {
        pending.age += 1;
        let keep = pending.age < STALE_STATE_RESEND_UPDATES;
        if !keep {
            tracing::warn!(pane = pane.0, "copy capture reply lost; will re-issue");
        }
        keep
    });
```

(`issue_copy_state` already takes `mut refresh: ResMut<CopyRefreshState>`, so no signature change.)

- [ ] **Step 7: Prune capture-in-flight on copy-mode exit**

In `on_copy_mode_exit` (`src/tmux/copy_mode.rs:104`), where it already prunes `state_in_flight` / `last_scroll` for the pane, add:

```rust
        refresh.capture_in_flight.remove(&pane.id);
```

(inside the existing `if let Ok(pane) = panes.get(entity) { ... }` block).

- [ ] **Step 8: Run the focused + full copy-mode tests**

Run: `cargo test -p ozmux copy_mode && cargo test -p ozmux decide_capture`
Expected: PASS — the pre-existing `copy_mode_exit_repaints_live_grid_and_prunes_refresh_state` test still passes, plus the new helper tests.

- [ ] **Step 9: Verify build, lint**

Run: `cargo build && cargo clippy --all-targets && cargo fmt`
Expected: clean.

- [ ] **Step 10: Commit**

```bash
git add src/tmux/copy_mode.rs
git commit -m "$(cat <<'EOF'
fix(tmux): resend lost copy-mode captures; record last_scroll on reply

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Final Verification

- [ ] **Whole workspace builds:** `cargo build`
- [ ] **All tests pass:** `cargo test`
- [ ] **Lint clean:** `cargo clippy --workspace --all-targets`
- [ ] **Formatted:** `cargo fmt --check`
- [ ] **Manual smoke (per `verify` skill):** run the app, split a pane / resize the window / enter copy mode repeatedly and confirm no pane goes black. Document the observation.

## Self-Review (author checklist — completed at write time)

1. **Spec coverage:** C1 → Task 2; C2 → Tasks 3+4 (incl. dims-vs-handle primary clause, debounce/transient handling, in-flight suppression via `panes_with_cursor_pending`, `Without<CopyModeState>` gate); C3 → Task 5 (capture resend + `last_scroll`-on-reply); C4 → Task 1 (material fallback, not creation-time init). Open-question simplifications (fold into `recapture_settled_panes`, drop C1) are deliberately NOT taken here — C2 reuses `request_pane_capture` via a message rather than mutating the `pub(crate)` `PaneRecaptureState` from the binary, which the binary cannot name.
2. **Placeholder scan:** none — every step carries concrete code and commands.
3. **Type consistency:** `grid_needs_full_seed` signature identical in Tasks 3 and 4; `RequestPaneReseed { pane }` produced in crate (Task 4 Step 5) and consumed in the binary system (Step 8); `CopyRefreshState.capture_in_flight: HashMap<PaneId, CapturePending>` defined (Step 3) and used (Steps 5–7); `apply_capture_reply`'s new `refresh` param added at the definition (Step 5) and the call site (Step 5).
