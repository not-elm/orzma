# Alt-Screen Fixed-Anchor Inline Webviews + SurfaceKind Removal — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let an inline webview be placed on the alternate screen at a fixed, reload-free, per-frame-re-anchored viewport rectangle, and remove the now-vestigial `SurfaceKind` enum.

**Architecture:** Replace the scalar scrollback line in the inline-webview anchor with a tagged `AnchorMode` (`Scrollback { line, col }` | `FixedScreen { row, col }`), defined once in `ozma_tty_engine` and reused by ozmux. The VT thread stamps `FixedScreen` (cursor's viewport-relative row/col) when the terminal is on the alternate screen, `Scrollback` otherwise. Projection gains a `FixedScreen` branch; a re-mount of a live handle updates the placement in place (no respawn, slot preserved); leaving the alternate screen despawns `FixedScreen` children via the existing `TerminalModeChanged` event. Phase 2 deletes `SurfaceKind` and its dead `Extension` branches.

**Tech Stack:** Rust edition 2024, Bevy 0.18 ECS, `alacritty_terminal` 0.26, `bevy_cef`. Crates: `ozma_tty_engine`, `ozma_tty_renderer`, `ozmux_multiplexer`, root binary `src/`.

**Spec:** `docs/superpowers/specs/2026-06-14-altscreen-fixed-anchor-webview-design.md`

**Conventions (from `.claude/rules/rust.md`):** no `mod.rs`; comments only `// TODO:` / `// NOTE:` / `// SAFETY:`; doc-comment every `pub` item; all `use` at top, single block; mutable params before immutable; private items last in a block. Run `cargo fmt` before every commit. Co-author commits per repo policy.

**Phasing:** Phase 1 (Tasks 1–6) delivers alt-screen support and is mergeable on its own. Phase 2 (Tasks 7–11) deletes `SurfaceKind` and is independent of Phase 1. Within each task, run the cited tests and only commit on green.

---

## File Structure

| File | Responsibility | Phase |
| --- | --- | --- |
| `crates/ozma_tty_engine/src/vt/listener.rs` | `AnchorMode` enum (new) + reshaped `InlineAnchor` payload | 1 |
| `crates/ozma_tty_engine/src/lib.rs` | re-export `AnchorMode` | 1 |
| `crates/ozma_tty_engine/src/handle.rs` | mount-time stamping: `FixedScreen` on alt-screen, `Scrollback` else | 1 |
| `src/inline_webview.rs` | `InlinePlacement.anchor: AnchorMode`; projection branch; in-place re-anchor; alt-exit teardown observer | 1 |
| `crates/multiplexer/src/components.rs` | delete `SurfaceKind` enum | 2 |
| `crates/multiplexer/src/lib.rs` | drop `SurfaceKind` re-export | 2 |
| `crates/multiplexer/src/commands.rs` | drop `kind` param from `add_surface` / `split_pane_with_surface` | 2 |
| `src/ui/surface.rs` | collapse `decorate_surface` / delete `kind_color` | 2 |
| `src/ui/chrome.rs` | delete `sync_pane_veil`; drop `kinds` queries | 2 |
| `src/ui/tab_label.rs` | tab label always `Cwd` | 2 |
| `src/ui/web_title.rs`, `src/ui.rs`, `src/theme.rs`, `src/ui/palette.rs` | delete `ExtensionSurfaceMarker`, `WebTitle` plumbing, `SURFACE_EXTENSION` | 2 |

---

## PHASE 1 — Alt-Screen Fixed-Anchor Support

### Task 1: Introduce `AnchorMode` in the engine and reshape `InlineAnchor`

This is a pure refactor: the payload changes shape but the stamped values and behavior are identical (scrollback only). No alt-screen behavior yet.

**Files:**
- Modify: `crates/ozma_tty_engine/src/vt/listener.rs:41-53`
- Modify: `crates/ozma_tty_engine/src/lib.rs:37`
- Modify: `crates/ozma_tty_engine/src/handle.rs:921-942` (anchor construction)
- Test: engine tests in `crates/ozma_tty_engine/src/handle.rs` that build `InlineAnchor`

- [ ] **Step 1: Replace the `InlineAnchor` struct with a mode-tagged form**

In `crates/ozma_tty_engine/src/vt/listener.rs`, replace the current struct (lines 41-53):

```rust
/// Anchor stamped by the VT thread at the exact byte position of a
/// `mount-inline` OSC: absolute scrollback line (top of the rect),
/// column, and the `frame_seq` the next grid emit will carry (used by
/// the GUI to defer first projection until the grid catches up).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InlineAnchor {
    /// Absolute line = history_base + history_size + live-grid cursor row.
    pub line: u64,
    /// Cursor column at the OSC byte position.
    pub col: u16,
    /// The seq value the next emitted frame will carry (wrap-aware compare).
    pub frame_seq: u32,
}
```

with:

```rust
/// How an inline webview is anchored to its terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnchorMode {
    /// Anchored to an absolute scrollback line; scrolls with the text
    /// (`line = history_base + history_size + live-grid cursor row`).
    Scrollback {
        /// Absolute scrollback line of the rect's top row.
        line: u64,
        /// Cursor column at the OSC byte position.
        col: u16,
    },
    /// Anchored to a viewport-relative cell; fixed on the visible alternate
    /// screen (`row` is the 0-based grid row of the cursor at the OSC).
    FixedScreen {
        /// Viewport-relative row of the rect's top cell.
        row: u16,
        /// Cursor column at the OSC byte position.
        col: u16,
    },
}

/// Anchor stamped by the VT thread at the exact byte position of a
/// `mount-inline` OSC: the anchor mode (scrollback vs alternate-screen) and
/// the `frame_seq` the next grid emit will carry (used by the GUI to defer
/// first projection until the grid catches up).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InlineAnchor {
    /// Where the rect is anchored.
    pub mode: AnchorMode,
    /// The seq value the next emitted frame will carry (wrap-aware compare).
    pub frame_seq: u32,
}
```

- [ ] **Step 2: Re-export `AnchorMode` from the crate root**

In `crates/ozma_tty_engine/src/lib.rs`, change line 37:

```rust
pub use vt::listener::{InlineAnchor, OscWebviewVerb};
```

to:

```rust
pub use vt::listener::{AnchorMode, InlineAnchor, OscWebviewVerb};
```

- [ ] **Step 3: Update the anchor construction in `handle_webview_verb`**

In `crates/ozma_tty_engine/src/handle.rs`, the `MountInline` arm currently builds (lines ~921-942):

```rust
        let anchor = if matches!(verb, OscWebviewVerb::MountInline { .. }) {
            if self.term.mode().contains(TermMode::ALT_SCREEN) {
                tracing::debug!("mount-inline rejected: alternate screen active");
                return;
            }
            if self.saturated {
                tracing::debug!("mount-inline rejected: scrollback saturated");
                return;
            }
            let cursor = self.term.grid().cursor.point;
            self.force_next_emit = true;
            Some(InlineAnchor {
                line: self.history_base
                    + self.term.history_size() as u64
                    + cursor.line.0.max(0) as u64,
                col: cursor.column.0 as u16,
                frame_seq: self.frame_seq,
            })
        } else {
            None
        };
```

Replace ONLY the `Some(InlineAnchor { ... })` construction (keep the two rejection guards exactly as-is for this task — alt-screen still rejects here; that guard is removed in Task 3):

```rust
            let cursor = self.term.grid().cursor.point;
            self.force_next_emit = true;
            Some(InlineAnchor {
                mode: AnchorMode::Scrollback {
                    line: self.history_base
                        + self.term.history_size() as u64
                        + cursor.line.0.max(0) as u64,
                    col: cursor.column.0 as u16,
                },
                frame_seq: self.frame_seq,
            })
```

Add `AnchorMode` to the existing `use` that brings in `InlineAnchor` at the top of `handle.rs` (it already imports `InlineAnchor` / `OscWebviewVerb` from `crate::vt::listener` or the crate root — add `AnchorMode` to that same `use` list).

- [ ] **Step 4: Update engine tests that build `InlineAnchor`**

Grep the engine crate for `InlineAnchor {`:

Run: `rg -n 'InlineAnchor \{' crates/ozma_tty_engine/src`

For each engine-side test construction of the form `InlineAnchor { line: L, col: C, frame_seq: F }`, rewrite to `InlineAnchor { mode: AnchorMode::Scrollback { line: L, col: C }, frame_seq: F }`. For assertions that read `anchor.line` / `anchor.col`, match on `AnchorMode::Scrollback { line, col }` instead. Add `use crate::vt::listener::AnchorMode;` (or `use super::*;` already in scope) to those test modules as needed.

- [ ] **Step 5: Build and test the engine crate**

Run: `cargo test -p ozma_tty_engine`
Expected: PASS (this is a behavior-preserving refactor; the same scrollback anchors are produced).

- [ ] **Step 6: Commit**

```bash
cargo fmt
git add crates/ozma_tty_engine
git commit -m "refactor(engine): tagged AnchorMode payload for InlineAnchor (scrollback only)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Adopt `AnchorMode` in ozmux `InlinePlacement`

Pure refactor on the ozmux side: the component carries `anchor: AnchorMode` instead of `anchor_line`/`anchor_col`. Projection still does scrollback-only. No behavior change.

**Files:**
- Modify: `src/inline_webview.rs:53-66` (struct), `:262-268` (mount stamping), `:611-629` (projection), and the module's `#[cfg(test)]` block
- Test: existing `src/inline_webview.rs` tests

- [ ] **Step 1: Reshape `InlinePlacement`**

In `src/inline_webview.rs`, replace the struct (lines 53-66):

```rust
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct InlinePlacement {
    /// Absolute scrollback line of the rect's TOP row.
    pub(crate) anchor_line: u64,
    /// Column of the anchor cell.
    pub(crate) anchor_col: u16,
    /// Rect height in terminal cells.
    pub(crate) rows: u16,
    /// Rect width in terminal cells.
    pub(crate) cols: u16,
    /// The VT frame seq stamped at mount; grid frames at or after this seq
    /// (wrap-aware compare) may project the placement.
    pub(crate) frame_seq: u32,
}
```

with:

```rust
/// Where an inline webview sits: its anchor mode (scrollback line vs fixed
/// viewport cell), the rect extent in cells, and the VT `frame_seq` the next
/// grid emit carries (`project_inline_overlays` defers first projection until
/// the grid catches up).
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct InlinePlacement {
    /// Where the rect is anchored.
    pub(crate) anchor: AnchorMode,
    /// Rect height in terminal cells.
    pub(crate) rows: u16,
    /// Rect width in terminal cells.
    pub(crate) cols: u16,
    /// The VT frame seq stamped at mount; grid frames at or after this seq
    /// (wrap-aware compare) may project the placement.
    pub(crate) frame_seq: u32,
}
```

Update the import at line 24 from `use ozma_tty_engine::InlineAnchor;` to `use ozma_tty_engine::{AnchorMode, InlineAnchor};`.

- [ ] **Step 2: Update the mount stamping**

In `mount_inline`, the component insert (lines 262-268) currently builds:

```rust
        InlinePlacement {
            anchor_line: anchor.line,
            anchor_col: anchor.col,
            rows: ctx.rows,
            cols: ctx.cols,
            frame_seq: anchor.frame_seq,
        },
```

Replace with (carry the mode straight through from the stamped anchor):

```rust
        InlinePlacement {
            anchor: anchor.mode,
            rows: ctx.rows,
            cols: ctx.cols,
            frame_seq: anchor.frame_seq,
        },
```

- [ ] **Step 3: Update the projection to read the scrollback variant**

In `project_inline_overlays`, the formula block (lines 608-629) currently is:

```rust
                if (grid.last_seq.wrapping_sub(placement.frame_seq) as i32) < 0 {
                    continue;
                }
                let viewport_row = placement.anchor_line as i64
                    - (grid.history_base as i64 + i64::from(grid.history_size)
                        - i64::from(grid.display_offset));
                if viewport_row + i64::from(placement.rows) <= 0
                    || viewport_row >= i64::from(grid.rows)
                    || u32::from(placement.anchor_col) >= u32::from(grid.cols)
                {
                    continue;
                }
                let slot = usize::from(view.slot);
                if slot >= OVERLAY_SLOTS {
                    continue;
                }
                overlays.rects[slot] = IVec4::new(
                    viewport_row as i32,
                    i32::from(placement.anchor_col),
                    i32::from(placement.rows),
                    i32::from(placement.cols),
                );
                overlays.textures[slot] = Some(texture.0.clone());
```

Replace with a version that destructures the (currently only) `Scrollback` variant. The `FixedScreen` branch is added in Task 4; for now treat `FixedScreen` as `continue` so the match is exhaustive:

```rust
                if (grid.last_seq.wrapping_sub(placement.frame_seq) as i32) < 0 {
                    continue;
                }
                let (viewport_row, anchor_col) = match placement.anchor {
                    AnchorMode::Scrollback { line, col } => {
                        let row = line as i64
                            - (grid.history_base as i64 + i64::from(grid.history_size)
                                - i64::from(grid.display_offset));
                        (row, col)
                    }
                    AnchorMode::FixedScreen { .. } => continue,
                };
                if viewport_row + i64::from(placement.rows) <= 0
                    || viewport_row >= i64::from(grid.rows)
                    || u32::from(anchor_col) >= u32::from(grid.cols)
                {
                    continue;
                }
                let slot = usize::from(view.slot);
                if slot >= OVERLAY_SLOTS {
                    continue;
                }
                overlays.rects[slot] = IVec4::new(
                    viewport_row as i32,
                    i32::from(anchor_col),
                    i32::from(placement.rows),
                    i32::from(placement.cols),
                );
                overlays.textures[slot] = Some(texture.0.clone());
```

- [ ] **Step 4: Update the module's tests**

In the `#[cfg(test)]` block of `src/inline_webview.rs`:

- `test_anchor()` (lines 684-690): change to
  ```rust
  fn test_anchor() -> InlineAnchor {
      InlineAnchor {
          mode: AnchorMode::Scrollback { line: 42, col: 3 },
          frame_seq: 7,
      }
  }
  ```
- Every `InlinePlacement { anchor_line: L, anchor_col: C, rows, cols, frame_seq }` literal in the tests (the `mount_spawns_child_with_inline_components` assertion at :820, `projection_holds_until_grid_seq_reaches_anchor_seq` at :1199, `formula_placement()` at :1238, the three placements in `projection_culls_fully_outside_rects` at :1300-1320, and the placements at :1337, :1368, :1401, :1428) becomes `InlinePlacement { anchor: AnchorMode::Scrollback { line: L, col: C }, rows, cols, frame_seq }`.

Run: `rg -n 'anchor_line|anchor_col' src/inline_webview.rs` to find every site; all must be migrated.

- [ ] **Step 5: Build and test**

Run: `cargo test -p ozmux-gui --lib inline_webview`
(If the root package name differs, use `cargo test --lib inline_webview` from the workspace root.)
Expected: PASS — same scrollback projection as before.

- [ ] **Step 6: Commit**

```bash
cargo fmt
git add src/inline_webview.rs
git commit -m "refactor(inline-webview): InlinePlacement.anchor: AnchorMode (scrollback only)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Stamp `FixedScreen` on the alternate screen (engine)

**Files:**
- Modify: `crates/ozma_tty_engine/src/handle.rs:921-942`
- Test: `crates/ozma_tty_engine/src/handle.rs` `#[cfg(test)]`

- [ ] **Step 1: Write the failing test**

Add to the engine `#[cfg(test)]` module in `handle.rs` (next to the existing mount-inline anchor tests around `:1937`). This test drives a `mount-inline` while the terminal is on the alternate screen and asserts a `FixedScreen` anchor is produced instead of a rejection. Use the same harness the existing anchor tests use; the key new assertion is the variant.

```rust
    #[test]
    fn mount_inline_on_alt_screen_stamps_fixed_screen_anchor() {
        let mut h = test_handle();
        // Enter the alternate screen, move the cursor to row 2, then mount.
        h.advance(b"\x1b[?1049h");
        h.advance(b"\r\n\r\n");
        h.advance(b"\x1b]5379;mount-inline;v;3;5\x1b\\");
        let frame = h.take_control_osc_webview();
        let anchor = frame.expect("alt-screen mount must produce an anchor, not a rejection");
        match anchor.mode {
            AnchorMode::FixedScreen { row, col } => {
                assert_eq!(row, 2, "fixed anchor row is the cursor's viewport row");
                assert_eq!(col, 0);
            }
            other => panic!("expected FixedScreen, got {other:?}"),
        }
    }
```

NOTE: `test_handle()` and `take_control_osc_webview()` are illustrative helper names — use the actual harness the existing `handle.rs` anchor tests use (find them with `rg -n 'fn .*anchor' crates/ozma_tty_engine/src/handle.rs` and copy their setup/drain pattern). The assertion on `AnchorMode::FixedScreen` is the real content of this step.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p ozma_tty_engine mount_inline_on_alt_screen_stamps_fixed_screen_anchor`
Expected: FAIL — currently the alt-screen guard returns early, so no anchor is produced.

- [ ] **Step 3: Replace the alt-screen rejection with a `FixedScreen` branch**

In `handle_webview_verb`, the `MountInline` arm (lines ~922-942) currently is:

```rust
        let anchor = if matches!(verb, OscWebviewVerb::MountInline { .. }) {
            if self.term.mode().contains(TermMode::ALT_SCREEN) {
                tracing::debug!("mount-inline rejected: alternate screen active");
                return;
            }
            if self.saturated {
                tracing::debug!("mount-inline rejected: scrollback saturated");
                return;
            }
            let cursor = self.term.grid().cursor.point;
            self.force_next_emit = true;
            Some(InlineAnchor {
                mode: AnchorMode::Scrollback {
                    line: self.history_base
                        + self.term.history_size() as u64
                        + cursor.line.0.max(0) as u64,
                    col: cursor.column.0 as u16,
                },
                frame_seq: self.frame_seq,
            })
        } else {
            None
        };
```

Replace with (alt-screen branches to `FixedScreen`; saturation gate now scopes only the scrollback path):

```rust
        let anchor = if matches!(verb, OscWebviewVerb::MountInline { .. }) {
            let cursor = self.term.grid().cursor.point;
            let col = cursor.column.0 as u16;
            let mode = if self.term.mode().contains(TermMode::ALT_SCREEN) {
                AnchorMode::FixedScreen {
                    row: cursor.line.0.max(0) as u16,
                    col,
                }
            } else {
                if self.saturated {
                    tracing::debug!("mount-inline rejected: scrollback saturated");
                    return;
                }
                AnchorMode::Scrollback {
                    line: self.history_base
                        + self.term.history_size() as u64
                        + cursor.line.0.max(0) as u64,
                    col,
                }
            };
            self.force_next_emit = true;
            Some(InlineAnchor {
                mode,
                frame_seq: self.frame_seq,
            })
        } else {
            None
        };
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p ozma_tty_engine mount_inline_on_alt_screen_stamps_fixed_screen_anchor`
Expected: PASS.

- [ ] **Step 5: Run the full engine suite (guard the scrollback path and same-chunk ordering)**

Run: `cargo test -p ozma_tty_engine`
Expected: PASS — existing scrollback and saturation tests are unaffected; if a test asserted "alt-screen rejects mount", it should now be updated/removed (search `rg -n 'alt.?screen' crates/ozma_tty_engine/src/handle.rs` and reconcile: alt-screen now stamps `FixedScreen`).

- [ ] **Step 6: Commit**

```bash
cargo fmt
git add crates/ozma_tty_engine
git commit -m "feat(engine): stamp FixedScreen anchor for mount-inline on the alternate screen

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: Project `FixedScreen` placements (ozmux)

**Files:**
- Modify: `src/inline_webview.rs:579-637` (projection), module doc at `:562-578`
- Test: `src/inline_webview.rs` `#[cfg(test)]`

- [ ] **Step 1: Write the failing tests**

Add to the projection tests in `src/inline_webview.rs` (near `alt_screen_blanks_all_slots` at :1354). Two behaviors: a `FixedScreen` placement projects to `viewport_row == row` while on the alternate screen, and a `Scrollback` placement is hidden while on the alternate screen (unchanged) but a `FixedScreen` placement is hidden on the primary screen.

```rust
    #[test]
    fn fixed_screen_projects_to_its_row_on_alt_screen() {
        let mut app = make_test_app();
        let terminal = app
            .world_mut()
            .spawn(TerminalGrid {
                modes: vec![ALT_SCREEN_MODE.to_string()],
                ..projection_grid(7)
            })
            .id();
        spawn_projection_child(
            &mut app,
            terminal,
            0,
            InlinePlacement {
                anchor: AnchorMode::FixedScreen { row: 5, col: 2 },
                rows: 4,
                cols: 10,
                frame_seq: 7,
            },
        );

        project(&mut app);
        assert_eq!(
            overlays_of(&app, terminal).rects[0],
            IVec4::new(5, 2, 4, 10),
            "a FixedScreen placement projects to its own viewport row on the alt screen"
        );
    }

    #[test]
    fn fixed_screen_is_hidden_on_primary_screen() {
        let mut app = make_test_app();
        let terminal = app.world_mut().spawn(projection_grid(7)).id();
        spawn_projection_child(
            &mut app,
            terminal,
            0,
            InlinePlacement {
                anchor: AnchorMode::FixedScreen { row: 5, col: 2 },
                rows: 4,
                cols: 10,
                frame_seq: 7,
            },
        );

        project(&mut app);
        assert_all_sentinel(overlays_of(&app, terminal));
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test --lib inline_webview::tests::fixed_screen`
Expected: FAIL — `FixedScreen` currently `continue`s (Task 2), so the first test sees a sentinel.

- [ ] **Step 3: Replace the unconditional alt-screen blank with a per-anchor rule**

In `project_inline_overlays`, the current per-child head (lines 604-610) is:

```rust
                has_inline_child = true;
                if on_alt_screen {
                    continue;
                }
                if (grid.last_seq.wrapping_sub(placement.frame_seq) as i32) < 0 {
                    continue;
                }
                let (viewport_row, anchor_col) = match placement.anchor {
                    AnchorMode::Scrollback { line, col } => {
                        let row = line as i64
                            - (grid.history_base as i64 + i64::from(grid.history_size)
                                - i64::from(grid.display_offset));
                        (row, col)
                    }
                    AnchorMode::FixedScreen { .. } => continue,
                };
```

Replace it with (drop the blanket `on_alt_screen` skip; instead each variant is shown on exactly one screen mode):

```rust
                has_inline_child = true;
                if (grid.last_seq.wrapping_sub(placement.frame_seq) as i32) < 0 {
                    continue;
                }
                let (viewport_row, anchor_col) = match placement.anchor {
                    AnchorMode::Scrollback { line, col } => {
                        if on_alt_screen {
                            continue;
                        }
                        let row = line as i64
                            - (grid.history_base as i64 + i64::from(grid.history_size)
                                - i64::from(grid.display_offset));
                        (row, col)
                    }
                    AnchorMode::FixedScreen { row, col } => {
                        if !on_alt_screen {
                            continue;
                        }
                        (i64::from(row), col)
                    }
                };
```

The downstream cull (`viewport_row + rows <= 0 || viewport_row >= grid.rows || anchor_col >= grid.cols`) and the slot write are unchanged and now apply to both variants — a `FixedScreen` rect whose `row`/`col` fell outside the (possibly resized) grid is culled for free.

- [ ] **Step 4: Update the projection doc comment**

Replace the rule-1 line in the `project_inline_overlays` doc (lines 565-573) so it states the per-anchor visibility instead of "alt-screen blanks everything":

```rust
/// Per-child projection rules, in order:
/// 1. Seq-hold: a placement is skipped until the grid's `last_seq` reaches the
///    mount-stamped `frame_seq` (wrap-aware compare).
/// 2. Screen-mode gating: a `Scrollback` placement projects only on the primary
///    screen (hidden while on the alternate screen); a `FixedScreen` placement
///    projects only on the alternate screen.
/// 3. `Scrollback`: `viewport_row = line - (history_base + history_size -
///    display_offset)`. `FixedScreen`: `viewport_row = row` (already
///    viewport-relative). Rects fully above/below the viewport or anchored at
///    or past the right edge are culled; a partially-above rect keeps its
///    negative row (the shader clips).
```

- [ ] **Step 5: Reconcile the existing alt-screen test**

`alt_screen_blanks_all_slots` (:1354) uses a `Scrollback` placement and asserts it is blanked on the alt screen and re-projects on return — both still hold under the new rule, so it should pass unchanged. Run the full projection group to confirm.

Run: `cargo test --lib inline_webview`
Expected: PASS (new `fixed_screen_*` tests pass; all existing projection tests still pass).

- [ ] **Step 6: Commit**

```bash
cargo fmt
git add src/inline_webview.rs
git commit -m "feat(inline-webview): project FixedScreen placements on the alternate screen

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: Re-mount = in-place re-anchor (no reload, slot preserved)

**Files:**
- Modify: `src/inline_webview.rs:200-290` (`mount_inline`)
- Test: `src/inline_webview.rs` `#[cfg(test)]` (replace `duplicate_mount_same_view_is_rejected`)

- [ ] **Step 1: Replace the reject test with an update-in-place test**

In `src/inline_webview.rs`, replace `duplicate_mount_same_view_is_rejected` (lines 863-877) with a test asserting the second mount updates the placement on the SAME entity (no reload):

```rust
    #[test]
    fn duplicate_mount_updates_placement_in_place() {
        let mut app = make_test_app();
        let terminal = spawn_terminal(&mut app);
        register_dyn(&mut app, "dash", terminal, true);

        mount(&mut app, terminal, "dash", Some(test_anchor()));
        let before = inline_children_of(&app, terminal);
        assert_eq!(before.len(), 1, "first mount spawns one child");
        let entity = before[0];
        let slot_before = app.world().get::<InlineWebview>(entity).unwrap().slot;

        // Re-mount the same handle with a different anchor.
        app.world_mut().trigger(OscWebviewRequest {
            entity: terminal,
            verb: OscWebviewVerb::MountInline {
                view_id: "dash".into(),
                rows: 12,
                cols: 50,
                instance_id: None,
            },
            anchor: Some(InlineAnchor {
                mode: AnchorMode::Scrollback { line: 99, col: 7 },
                frame_seq: 9,
            }),
        });
        app.world_mut().flush();

        let after = inline_children_of(&app, terminal);
        assert_eq!(after.len(), 1, "re-mount must NOT spawn a second child");
        assert_eq!(after[0], entity, "re-mount must reuse the same entity (no reload)");
        assert_eq!(
            app.world().get::<InlinePlacement>(entity),
            Some(&InlinePlacement {
                anchor: AnchorMode::Scrollback { line: 99, col: 7 },
                rows: 12,
                cols: 50,
                frame_seq: 9,
            }),
            "re-mount updates the placement in place"
        );
        assert_eq!(
            app.world().get::<InlineWebview>(entity).unwrap().slot,
            slot_before,
            "re-mount preserves the overlay slot"
        );
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --lib inline_webview::tests::duplicate_mount_updates_placement_in_place`
Expected: FAIL — current code drops the duplicate, so the placement keeps the first anchor.

- [ ] **Step 3: Add the in-place re-anchor fast path**

In `mount_inline`, the duplicate check (lines 213-224) currently is:

```rust
    let live = live_inline_children(&params.children, &params.views, ctx.terminal_surface);
    if live
        .iter()
        .any(|(_, v)| v.view_id == ctx.view_id && v.instance_id.as_deref() == ctx.instance_id)
    {
        tracing::debug!(
            view_id = %ctx.view_id,
            instance_id = ?ctx.instance_id,
            "osc-webview: duplicate inline mount on this terminal, dropping"
        );
        return;
    }
```

Replace it with a fast path that updates the existing child's `InlinePlacement` and returns — BEFORE slot allocation, size seeding, preload build, or `WebviewSource` insertion. Note the existing entity is found from the same `live` scan:

```rust
    let live = live_inline_children(&params.children, &params.views, ctx.terminal_surface);
    if let Some((existing, _)) = live
        .iter()
        .find(|(_, v)| v.view_id == ctx.view_id && v.instance_id.as_deref() == ctx.instance_id)
    {
        // In-place re-anchor: keep the entity (and its CEF page + slot); only
        // the placement changes. `set_if_neq` elides a no-op re-emit so an
        // unchanged frame triggers neither a projection move nor a CEF resize.
        let next = InlinePlacement {
            anchor: anchor.mode,
            rows: ctx.rows,
            cols: ctx.cols,
            frame_seq: anchor.frame_seq,
        };
        if let Ok(mut placement) = params.placements.get_mut(*existing) {
            placement.set_if_neq(next);
        }
        return;
    }
```

`params.placements` is a new query — add it to `InlineWebviewParams` (the `SystemParam` at lines 133-141):

```rust
#[derive(SystemParam)]
pub(crate) struct InlineWebviewParams<'w, 's> {
    commands: Commands<'w, 's>,
    images: ResMut<'w, Assets<Image>>,
    placements: Query<'w, 's, &'static mut InlinePlacement>,
    children: Query<'w, 's, &'static Children>,
    views: Query<'w, 's, &'static InlineWebview>,
    metrics: Option<Res<'w, TerminalCellMetricsResource>>,
    windows: Query<'w, 's, &'static Window, With<PrimaryWindow>>,
}
```

(Mutable query placed before the immutable ones, per the mutable-params-first rule applied to `SystemParam` fields.)

NOTE: keep the `anchor` already destructured at the top of `mount_inline` (lines 205-208) — the fast path uses `anchor.mode` / `anchor.frame_seq`, and `resolve_mount` still runs first only for the spawn path. Move the `resolve_mount` call (lines 209-212) to AFTER this fast-path block so a re-anchor skips the registry lookup; the spawn path below it is unchanged.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test --lib inline_webview::tests::duplicate_mount_updates_placement_in_place`
Expected: PASS.

- [ ] **Step 5: Run the full inline-webview suite**

Run: `cargo test --lib inline_webview`
Expected: PASS. NOTE: `duplicate_view_instance_tuple_is_rejected` (:1017) asserted a duplicate `(view_id, instance_id)` does not spawn a second child — that still holds (one child), so it passes; if its assertion message says "rejected", leave it (the count is still 1).

- [ ] **Step 6: Commit**

```bash
cargo fmt
git add src/inline_webview.rs
git commit -m "feat(inline-webview): re-mount updates placement in place (no reload, slot preserved)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: Auto-unmount `FixedScreen` children on alt-screen exit

**Files:**
- Modify: `src/inline_webview.rs` (new observer + plugin registration at `:82-107`)
- Test: `src/inline_webview.rs` `#[cfg(test)]`

- [ ] **Step 1: Write the failing test**

Add to `src/inline_webview.rs` tests. Trigger `TerminalModeChanged { removed: ["alt-screen"] }` on a terminal that has one `FixedScreen` child and one `Scrollback` child; assert only the `FixedScreen` child is despawned.

```rust
    #[test]
    fn alt_screen_exit_despawns_only_fixed_screen_children() {
        use ozma_tty_engine::TerminalModeChanged;

        let mut app = make_test_app();
        app.add_observer(despawn_fixed_screen_on_alt_exit);
        let terminal = app.world_mut().spawn(projection_grid(7)).id();
        let fixed = spawn_projection_child(
            &mut app,
            terminal,
            0,
            InlinePlacement {
                anchor: AnchorMode::FixedScreen { row: 1, col: 0 },
                rows: 4,
                cols: 10,
                frame_seq: 7,
            },
        );
        let _scroll = spawn_projection_child(
            &mut app,
            terminal,
            1,
            InlinePlacement {
                anchor: AnchorMode::Scrollback { line: 42, col: 0 },
                rows: 4,
                cols: 10,
                frame_seq: 7,
            },
        );
        // The fixed/scroll handles are Image handles; resolve child entities.
        let fixed_entity = inline_children_of(&app, terminal)
            .into_iter()
            .find(|e| matches!(
                app.world().get::<InlinePlacement>(*e).unwrap().anchor,
                AnchorMode::FixedScreen { .. }
            ))
            .unwrap();

        app.world_mut().trigger(TerminalModeChanged {
            entity: terminal,
            added: vec![],
            removed: vec![ALT_SCREEN_MODE.to_string()],
        });
        app.world_mut().flush();
        app.update();

        let remaining = inline_children_of(&app, terminal);
        assert_eq!(remaining.len(), 1, "the FixedScreen child must be despawned");
        assert!(
            !remaining.contains(&fixed_entity),
            "the despawned child must be the FixedScreen one"
        );
        assert!(matches!(
            app.world().get::<InlinePlacement>(remaining[0]).unwrap().anchor,
            AnchorMode::Scrollback { .. }
        ));
        let _ = fixed;
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --lib inline_webview::tests::alt_screen_exit_despawns_only_fixed_screen_children`
Expected: FAIL — `despawn_fixed_screen_on_alt_exit` does not exist yet (compile error).

- [ ] **Step 3: Add the observer**

In `src/inline_webview.rs` (place it after `unmount_inline`, before the private helpers, honoring "private items after pub"):

```rust
/// Despawns the `FixedScreen` inline children of a terminal when it leaves the
/// alternate screen. The teardown lands before the next `PostUpdate`
/// projection (the engine triggers `TerminalModeChanged` before the frame
/// trigger, and despawn commands flush at the `Update`→`PostUpdate` boundary),
/// so no stale rectangle is painted (spec §4.6, Kitty issue #2901).
fn despawn_fixed_screen_on_alt_exit(
    event: On<TerminalModeChanged>,
    mut commands: Commands,
    children: Query<&Children>,
    placements: Query<&InlinePlacement>,
) {
    if !event.removed.iter().any(|m| m == ALT_SCREEN_MODE) {
        return;
    }
    let Ok(kids) = children.get(event.entity) else {
        return;
    };
    for child in kids.iter() {
        if let Ok(placement) = placements.get(child) {
            if matches!(placement.anchor, AnchorMode::FixedScreen { .. }) {
                commands.entity(child).despawn();
            }
        }
    }
}
```

Add `TerminalModeChanged` to the `ozma_tty_engine` import at the top of the file (it is re-exported at the crate root):

```rust
use ozma_tty_engine::{AnchorMode, InlineAnchor, TerminalModeChanged};
```

- [ ] **Step 4: Register the observer in the plugin**

In `OzmuxInlineWebviewPlugin::build` (lines 82-107), add after the `add_systems` calls:

```rust
        app.add_observer(despawn_fixed_screen_on_alt_exit);
```

- [ ] **Step 5: Run to verify it passes**

Run: `cargo test --lib inline_webview::tests::alt_screen_exit_despawns_only_fixed_screen_children`
Expected: PASS.

- [ ] **Step 6: Full workspace build + test**

Run: `cargo test`
Expected: PASS.
Run: `cargo clippy --workspace`
Expected: no warnings (fix any; prefer `#[expect(..., reason = "...")]` over `#[allow]`).

- [ ] **Step 7: Commit**

```bash
cargo fmt
git add src/inline_webview.rs
git commit -m "feat(inline-webview): auto-unmount FixedScreen children on alt-screen exit

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

**Phase 1 complete.** Inline webviews now mount, re-anchor, and tear down correctly on the alternate screen. This is mergeable independently.

---

## PHASE 2 — Remove the vestigial `SurfaceKind`

`SurfaceKind` has only a `Terminal` variant in production; `Extension` is constructed only in `#[cfg(test)]`. These tasks delete the enum and the dead `Extension` machinery. Each task ends compiling and green.

### Task 7: Drop the `kind` parameter from surface constructors

**Files:**
- Modify: `crates/multiplexer/src/commands.rs:117-124` (`create_workspace`), `:224-253` (`split_pane_with_surface`), `:266` (`split_pane`), `:359-368` (`add_surface`)
- Modify all production + test call sites listed below
- Test: `cargo test -p ozmux_multiplexer`

- [ ] **Step 1: Change `add_surface` to always stamp `Terminal`**

In `crates/multiplexer/src/commands.rs`, replace `add_surface` (lines 359-368):

```rust
pub fn add_surface(&mut self, pane: Entity, kind: SurfaceKind) -> Entity {
    let surface = self
        .commands
        .spawn((SurfaceMarker, kind, Name::new("surface")))
        .id();
    self.commands
        .entity(surface)
        .insert((ChildOf(pane), SurfaceOf(pane)));
    surface
}
```

with:

```rust
pub fn add_surface(&mut self, pane: Entity) -> Entity {
    let surface = self
        .commands
        .spawn((SurfaceMarker, Name::new("surface")))
        .id();
    self.commands
        .entity(surface)
        .insert((ChildOf(pane), SurfaceOf(pane)));
    surface
}
```

- [ ] **Step 2: Change `split_pane_with_surface` to drop `kind`**

Replace the signature + spawn (lines 224-232):

```rust
pub fn split_pane_with_surface(
    &mut self,
    target_pane: Entity,
    side: Side,
    orientation: SplitOrientation,
    kind: SurfaceKind,
) -> MultiplexerResult<SplitOutcome> {
    let surface = self
        .commands
        .spawn((SurfaceMarker, kind, Name::new("surface: split")))
        .id();
```

with:

```rust
pub fn split_pane_with_surface(
    &mut self,
    target_pane: Entity,
    side: Side,
    orientation: SplitOrientation,
) -> MultiplexerResult<SplitOutcome> {
    let surface = self
        .commands
        .spawn((SurfaceMarker, Name::new("surface: split")))
        .id();
```

And its caller `split_pane` (line 266): `self.split_pane_with_surface(target_pane, side, orientation, SurfaceKind::Terminal)` → `self.split_pane_with_surface(target_pane, side, orientation)`.

- [ ] **Step 3: Fix `create_workspace`’s bootstrap spawn**

Replace (lines 117-124):

```rust
        let surface = self
            .commands
            .spawn((
                SurfaceMarker,
                SurfaceKind::Terminal,
                Name::new(format!("surface: {name}#0")),
            ))
            .id();
```

with:

```rust
        let surface = self
            .commands
            .spawn((SurfaceMarker, Name::new(format!("surface: {name}#0"))))
            .id();
```

- [ ] **Step 4: Update all production call sites of `add_surface`**

Replace `add_surface(X, SurfaceKind::Terminal)` → `add_surface(X)` and remove the now-unused `SurfaceKind` import at:
- `src/action/new_terminal_surface.rs:39` (and import at `:4`)
- `src/action/split_pane.rs:48` (and import at `:4`) — this is a `split_pane_with_surface(..., SurfaceKind::Terminal)`; drop the last arg.

- [ ] **Step 5: Update all test call sites**

Apply the same `add_surface(X, SurfaceKind::Terminal)` → `add_surface(X)` rewrite (and drop `SurfaceKind` test imports) at every remaining site:
- `crates/multiplexer/src/commands.rs`: `:632`, `:672`, `:886` (`set_active...`), `:1115`, `:1162`, `:1339`, `:1365`, `:1401`
- `src/ui.rs`: `:286`, `:507` (and imports `:253`, `:481`)
- `src/ui/tab_input.rs`: `:119` (import `:92`)
- `src/ui/workspace.rs`: `:428`, `:485` (imports `:402`, `:454`)
- `src/action/focus_surface.rs`: `:95` (import `:65`)

Delete the test `split_pane_with_surface_seeds_extension_surface` (`crates/multiplexer/src/commands.rs:1208-1263`) entirely — it asserts the `Extension` variant, which is gone.

Run: `rg -n 'add_surface\(|split_pane_with_surface\(' src crates` to confirm no call still passes a kind.

- [ ] **Step 6: Build the multiplexer crate**

Run: `cargo test -p ozmux_multiplexer`
Expected: PASS (the enum still exists; only the constructors changed). Compile errors here point to a missed call site — fix and re-run.

- [ ] **Step 7: Commit**

```bash
cargo fmt
git add crates/multiplexer src/action
git commit -m "refactor(multiplexer): drop kind param from surface constructors (always Terminal)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 8: Delete the `SurfaceKind` enum and collapse surface decoration

**Files:**
- Modify: `crates/multiplexer/src/components.rs:140-151`, `crates/multiplexer/src/lib.rs:20-26`
- Modify: `src/ui/surface.rs:14-49` and its tests
- Modify: `src/theme.rs:32`, `src/ui/palette.rs:5-8`

- [ ] **Step 1: Delete the enum and its re-export**

Remove the `SurfaceKind` enum (`crates/multiplexer/src/components.rs:140-151`). In `crates/multiplexer/src/lib.rs`, remove `SurfaceKind,` from the `components::{...}` re-export (lines 20-26). Leave `SurfaceMarker` in place.

- [ ] **Step 2: Collapse `kind_color` and `decorate_surface`**

In `src/ui/surface.rs`, delete `kind_color` (lines 14-19) and rewrite `decorate_surface` (lines 27-49) to drop the `kind` param and always stamp the terminal marker + terminal background:

```rust
/// Inserts the surface's flex `Node`, terminal background, and the
/// `TerminalSurfaceMarker` that `finish_terminal_setup` queries.
pub(crate) fn decorate_surface(commands: &mut Commands, surface: Entity) {
    commands.entity(surface).insert((
        Node {
            flex_grow: 1.0,
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            flex_direction: FlexDirection::Row,
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
        BackgroundColor(palette::SURFACE_TERMINAL),
        TerminalSurfaceMarker,
    ));
}
```

Update the import line `:7` to drop `ExtensionSurfaceMarker` (keep `TerminalSurfaceMarker`, `palette`), and remove the `SurfaceKind` import at `:11`.

- [ ] **Step 3: Fix `decorate_surface`’s caller**

In `src/ui/chrome.rs` `slot_active_surface` (around :127), the call is currently guarded by a kind lookup:

```rust
        if let Ok(kind) = kinds.get(new_surface) {
            decorate_surface(&mut commands, new_surface, kind);
        }
```

Replace with an unconditional call (every surface is a terminal):

```rust
        decorate_surface(&mut commands, new_surface);
```

(The `kinds` query param in `slot_active_surface` at `:115` is removed in Task 9 together with the other `kinds` queries; for now it becomes unused — that is fine, it compiles, and Task 9 deletes it.)

- [ ] **Step 4: Fix the `surface.rs` tests**

Delete `kind_color_terminal_uses_surface_terminal_constant` (lines 57-62). Update the `decorate_surface(&mut commands, surface, &SurfaceKind::Terminal)` call at `:72` to `decorate_surface(&mut commands, surface)`. The `TerminalSurfaceMarker` assertion at `:77` still holds.

- [ ] **Step 5: Remove `SURFACE_EXTENSION`**

Delete `SURFACE_EXTENSION` from `src/theme.rs:32` and from the `src/ui/palette.rs` re-export (line 7). `rg -n 'SURFACE_EXTENSION' src` must come back empty.

- [ ] **Step 6: Build**

Run: `cargo build`
Expected: compiles. Remaining errors are the `kinds`-query users and `tab_label`/`sync_pane_veil` — addressed in Task 9. If `cargo build` still fails ONLY in `chrome.rs`/`tab_label.rs` for `SurfaceKind`, that is expected; proceed to Task 9 before committing. Otherwise, if it builds, run `cargo test -p ozmux_multiplexer` and commit now:

```bash
cargo fmt
git add crates/multiplexer src/ui/surface.rs src/theme.rs src/ui/palette.rs src/ui/chrome.rs
git commit -m "refactor(ui): delete SurfaceKind enum and collapse surface decoration

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

NOTE: if `chrome.rs`/`tab_label.rs` still reference `SurfaceKind`, fold this commit into Task 9 (stage all together) so the tree never has a non-compiling commit.

---

### Task 9: Delete the veil system and simplify the tab label

`sync_pane_veil` exists solely to veil non-terminal surfaces; with every surface a terminal it is dead. `tab_label` always takes the `Cwd` path.

**Files:**
- Modify: `src/ui/chrome.rs:108-134` (`slot_active_surface` query), `:150` (`refresh_pane_tabs` query), `:217-278` (`sync_pane_veil`), `:31-36` (registration)
- Modify: `src/ui/tab_label.rs:27-51` and tests
- Modify: `src/ui/web_title.rs`, `src/ui.rs` (`WebTitle` plumbing, `ExtensionSurfaceMarker`)

- [ ] **Step 1: Delete `sync_pane_veil` and its registration**

Remove the entire `sync_pane_veil` function (`src/ui/chrome.rs:217-278`). In the system registration (lines 31-36), drop `sync_pane_veil` from the tuple:

```rust
        .add_systems(
            Update,
            (slot_active_surface, refresh_pane_tabs).after(OzmuxSystems::BuildChrome),
        );
```

Remove the now-unused `kinds: Query<&SurfaceKind, With<SurfaceMarker>>` param from `slot_active_surface` (line 115) and from `refresh_pane_tabs` (line 150). Remove the `SurfaceKind` import at `:21`. If `PaneDimOverlay` is now unused (grep `rg -n 'PaneDimOverlay' src`), delete its definition too; if it is still used by the renderer-side `PaneDim` path, leave it.

NOTE: `sync_pane_veil` was the only veil for non-terminal surfaces; terminals are dimmed by the renderer `PaneDim` uniform, which is untouched. Inactive-pane dimming for terminals still works.

- [ ] **Step 2: Simplify `tab_label`**

Replace `tab_label` (`src/ui/tab_label.rs:27-51`) to always render the terminal `Cwd` path:

```rust
/// The tab label for a terminal surface: the home-abbreviated, sanitized,
/// front-truncated current working directory (or the terminal placeholder).
pub(crate) fn tab_label(cwd: Option<&Cwd>, home: Option<&Path>, max_chars: usize) -> String {
    let Some(Cwd(path)) = cwd else {
        return TERMINAL_PLACEHOLDER.to_string();
    };
    let abbreviated = abbreviate_home(path, home);
    let sanitized = sanitize_title(&abbreviated);
    front_truncate(&sanitized, max_chars)
}
```

Remove the `SurfaceKind` and `WebTitle` imports (`:6`, `:8`) and the `WEB_PLACEHOLDER` const if now unused. Update the caller in `refresh_pane_tabs` (`src/ui/chrome.rs:~148-150`) to call `tab_label(cwd, home, max)` without the `kind` / `web_title` args (drop the `web_title` query/lookup there too).

- [ ] **Step 3: Fix the `tab_label` tests**

In `src/ui/tab_label.rs` tests: delete the `ext()` fixture (lines 193-197) and the extension tests that use it (`extension_renders_web_title`, `extension_blank_without_title`, `extension_blank_on_empty_title`, `web_title_back_truncates`, `web_title_truncate_boundary`, `web_title_control_chars_stripped` — lines ~199-240). Delete the `term()` fixture (lines 132-134) and update the terminal tests to call `tab_label(cwd, home, max)` directly (drop the `&term()` first arg).

- [ ] **Step 4: Remove `WebTitle` and `ExtensionSurfaceMarker` if now dead**

Run: `rg -n 'WebTitle' src` and `rg -n 'ExtensionSurfaceMarker' src`. After Step 2 the only `WebTitle` readers were `tab_label` / `refresh_pane_tabs`. If nothing reads `WebTitle` anymore:
- Delete `src/ui/web_title.rs` (the component, its plugin `WebTitlePlugin`, and the observer) and its `mod web_title;` + plugin registration in `src/ui.rs` (`:124`).
- Delete `ExtensionSurfaceMarker` (`src/ui.rs:71-76`).

If something still reads `WebTitle` (unexpected), leave it and note why in the commit message.

- [ ] **Step 5: Delete the `extension_pane_keeps_pickable_ignore_veil` test**

Remove `src/ui.rs:691-741` (the test constructs `SurfaceKind::Extension` and asserts a veil overlay that no longer exists).

- [ ] **Step 6: Build the whole workspace**

Run: `cargo build`
Expected: compiles cleanly.
Run: `rg -n 'SurfaceKind' src crates` — expected: no matches anywhere.

- [ ] **Step 7: Commit**

```bash
cargo fmt
git add src/ui crates
git commit -m "refactor(ui): delete dead pane-veil system, WebTitle/Extension plumbing; tab label is Cwd-only

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 10: Optionally drop `TerminalSurfaceMarker`

`finish_terminal_setup` is the only discriminating consumer of `TerminalSurfaceMarker`; with every surface terminal-backed it duplicates `SurfaceMarker`. This task is OPTIONAL — do it only if it stays clean.

**Files:**
- Inspect: every `TerminalSurfaceMarker` reader (`src/ui/terminal.rs:52`, `src/ui/workspace.rs:130`, `src/input/ime.rs:156`, plus tests)

- [ ] **Step 1: Audit the readers**

Run: `rg -n 'TerminalSurfaceMarker' src`
For each production query (`finish_terminal_setup`, `sync_active_workspace`, `set_ime_target`), decide whether `With<SurfaceMarker>` (optionally `+ Without<TerminalHandle>` where the marker was gating un-set-up surfaces) is an equivalent filter.

- [ ] **Step 2: If equivalent, replace and delete the marker**

Replace `With<TerminalSurfaceMarker>` filters with the chosen `SurfaceMarker`-based filter, delete the `TerminalSurfaceMarker` definition (`src/ui.rs:65-69`) and its insertion in `decorate_surface`, and update the `surface.rs:77` test assertion. If ANY reader is not cleanly expressible without the marker, STOP and keep `TerminalSurfaceMarker` — it is harmless and this task is optional.

- [ ] **Step 3: Build + test**

Run: `cargo test`
Expected: PASS.

- [ ] **Step 4: Commit (only if Step 2 was done)**

```bash
cargo fmt
git add src
git commit -m "refactor(ui): drop redundant TerminalSurfaceMarker in favor of SurfaceMarker

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 11: Full verification

- [ ] **Step 1: Workspace test + lint**

Run: `cargo test`
Expected: PASS.
Run: `cargo clippy --workspace --all-targets`
Expected: no warnings. Fix any; prefer `#[expect(..., reason = "...")]` over `#[allow]`.
Run: `cargo fmt --check`
Expected: clean.

- [ ] **Step 2: Confirm the dead surface taxonomy is gone**

Run: `rg -n 'SurfaceKind|SURFACE_EXTENSION|sync_pane_veil' src crates`
Expected: no matches.

- [ ] **Step 3: Manual smoke (optional, requires CEF)**

Per `docs/dyn-webview.md`: `cargo run --features debug`, then inside a pane run a client that enters the alternate screen (`printf '\e[?1049h'`), registers an inline view, and re-emits `mount-inline` each "frame" at a fixed cursor position. Confirm the webview renders on the alt screen, follows a moved cursor without reloading, and disappears on `\e[?1049l`.

---

## Self-Review Notes

- **Spec coverage:** §4.1 `AnchorMode` → Task 1–2; §4.2 stamping → Task 3; §4.3 in-place re-anchor + slot preservation → Task 5; §4.4 per-frame contract → exercised by Task 5 (re-anchor) + Task 4 (projection); §4.5 projection table → Task 4; §4.6 alt-exit auto-unmount via `TerminalModeChanged` → Task 6; §4.7 `SurfaceKind` removal → Tasks 7–10; §4.8 edge policies (slot cap, geometry limits unchanged; focus preserved by never respawning) → Tasks 4–6 (no code change needed beyond not touching those paths). §5 invariants are asserted by the Task 5/6 tests.
- **Type consistency:** `AnchorMode { Scrollback { line: u64, col: u16 }, FixedScreen { row: u16, col: u16 } }` is used identically in the engine (`InlineAnchor.mode`) and ozmux (`InlinePlacement.anchor`). `InlineAnchor` is `{ mode, frame_seq }` everywhere after Task 1.
- **Ordering caveat (Task 6):** the alt-exit despawn relies on `TerminalModeChanged` firing before the frame trigger (engine `handle.rs:690-703`) and despawn commands flushing before `PostUpdate` projection; Task 4 additionally gates `FixedScreen` to `on_alt_screen`, so even a one-frame race cannot paint a stale rect.
