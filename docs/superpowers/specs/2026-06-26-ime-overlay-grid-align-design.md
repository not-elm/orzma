# IME Preedit Overlay — Cell-Grid Alignment — Design

- **Date:** 2026-06-26
- **Branch:** `hoge`
- **Status:** Approved (brainstorming) — pending spec review → implementation plan
- **Scope:** One fix in the binary's IME overlay (`src/ui/ime_overlay.rs`, plus a
  dependency line in the root `Cargo.toml`). No renderer / engine changes.

---

## Problem

While the IME preedit overlay is shown, as the user keeps typing the caret
(beam) bar drifts away from the visual end (suffix) of the preedit string. The
drift grows with composition length — i.e. it accumulates per glyph.

## Root cause

The preedit **text body** and the **caret/clause/background** are positioned by
two different layout models that diverge per glyph, so the error accumulates:

1. **Text body** — `src/ui/ime_overlay.rs` writes the whole composition into a
   single Bevy `Text` node (`root_text.0 = comp.text()`). Bevy's UI text pipeline
   (cosmic-text) shapes it **proportionally** using each glyph's real advance, not
   snapped to the terminal cell grid. CJK glyphs come from a separate fallback
   font (UDEVGothic35, registered in `src/font.rs::register_cjk_fallback_with_cosmic`).

2. **Caret / clause / background** — computed analytically by monospace cell
   arithmetic: `cell_w_logical = advance_phys.floor() / scale`, then
   `beam_x = pos.x + end_cells * cell_w_logical`, where
   `end_cells = UnicodeWidthStr::width(text[..end])`.

Two accumulating error sources, confirmed by a Codex second-opinion pass that
parsed the bundled TTFs:

- **(a) `floor()` truncation.** The cell pitch is the primary `'0'` advance
  (`crates/ozma_tty_renderer/src/glyph/font.rs` `advance_phys = h_advance('0')`),
  floored at `material.rs:553` for the grid. cosmic-text lays out ASCII with the
  un-floored advance and (Bevy default `FontHinting::Disabled`) does **not**
  x-snap, so per ASCII cell the caret lags by `frac(advance_phys)`.
- **(b) CJK fallback advance mismatch (dominant for Japanese).** Measured: JBM
  `'0'` advance ≈ `0.600 em`; UDEVGothic35 `'あ'/'に'` advance ≈ `0.998 em`
  (≈ `1.664×` the primary `'0'`, not `2×`). The caret assumes `2 * floor(advance)`
  per fullwidth glyph, so it runs **ahead** of the shaped CJK text.

The caret/clause/background math already matches the terminal grid (same
`floor()` as `material.rs:553`). The text body is the outlier. **Decision:**
align the text body to the cell grid rather than chase proportional shaping with
the caret. This matches a terminal IME's expectation — the preedit should occupy
the same cells the committed text will.

## Decision

Render the preedit as **one leaf `Text` node per grapheme cluster**, each
absolutely positioned at its cell origin (`pos.x + cum_cells * cell_w_logical`),
so the text body, caret, clause highlight, and occluding background all share the
**same cell arithmetic**. Drift cannot accumulate because every visible element is
anchored to the same grid the caret already uses.

**Fidelity scope (chosen): grid-anchor only.** Each grapheme is left-aligned to
its cell origin. A fullwidth glyph whose shaped advance is narrower than its
2-cell span leaves a small **fixed** gap before the caret — this is exactly what
the committed terminal text shows too (the cursor sits at the next cell), so it
is faithful, not a regression. We do **not** scale/center glyphs within cells
(full per-pixel cell parity is out of scope).

## Approach

### Entity structure

The current single `Text` node serves double duty (text + occluding background).
Split it:

| Entity | Role | X / width |
| --- | --- | --- |
| `ImeOverlayNode` (repurposed → background-only) | Occludes the underlying terminal line (`BackgroundColor = pane default_bg`, auto-required by `Node`) | `left = pos.x`, `top = pos.y`, `width = total_cells * cell_w_logical`, `height = line_height_logical` |
| `ImeGraphemeCell` (new, pooled) | One leaf `Text` per grapheme cluster; CJK via fallback font | `left = pos.x + cum_cells * cell_w_logical`, `top = pos.y` |
| `ImeUnderline` (new, single bar) | Continuous underline under the whole preedit (IME convention) | `left = pos.x`, `width = total_cells * cell_w_logical`, `top = pos.y + (ascent_phys − underline_position_phys) / scale`, `height = underline_thickness_phys / scale` (note: `underline_position_phys` is **baseline-relative and negative**, so ascent must be folded in — a bare `underline_position_phys` places the bar above the cell top) |
| `ImeCaretBar` | **Unchanged** — already cell-correct | unchanged |
| `ImeClauseHighlight` | **Unchanged** — already cell-correct | unchanged |

Each `ImeGraphemeCell` is an independent top-level UI entity (not a child of any
other — parenting it under the background node would put the `Text` on the
flex-container branch and collapse its `ComputedNode.size` to 0×0), so it stays a
Taffy leaf and the text measure func (`NodeMeasure::Fixed` under
`LineBreak::NoWrap`, per `bevy_ui/src/widget/text.rs:292-295`) drives its
`ComputedNode.size` — the same constraint the current root `Text`, caret bar, and
clause highlight already satisfy. The `Underline`/`UnderlineColor`/`Text`/
`TextFont`/`TextLayout` components are removed from `ImeOverlayNode`; it keeps only
`Node` + `BackgroundColor`.

### Pure decision helper (unit-testable, no `App`)

```rust
struct CellPlacement { text: String, left: f32 }

fn layout_preedit_cells(text: &str, cell_w_logical: f32, origin_x: f32)
    -> Vec<CellPlacement>
```

- Split `text` into grapheme clusters via `unicode-segmentation`
  (`UnicodeSegmentation::graphemes(true)`). It is currently a **crate-local**
  dependency (`crates/ozma_tty_renderer/Cargo.toml:14`), **not** in
  `[workspace.dependencies]`. Promote it to `[workspace.dependencies]` and
  reference it via `{ workspace = true }` from both the root crate and
  `ozma_tty_renderer` (matching the existing `unicode-width = { workspace = true }`
  policy).
- Per cluster, width = `UnicodeWidthStr::width(cluster)`, clamped to the
  renderer's `runs_to_cells` rule (`crates/ozma_tty_renderer/src/grid.rs:71-72`):
  a `width >= 2` cluster consumes 2 cells, a `width == 0` cluster consumes 0 and
  merges into the previous placement. Accumulate `cum_cells` and emit
  `left = origin_x + cum_cells as f32 * cell_w_logical`.
- A width-0 cluster (lone combining mark) is merged into the previous placement's
  `text` rather than emitted as its own node (it consumes no cell — mirrors the
  renderer's `material.rs:693` "width=0 cells don't consume a column").

This is the same pure-function family as the existing `caret_cell_offsets` and
`compute_overlay_pos`.

### Entity pool

- New resource `ImeGraphemePool(Vec<Entity>)`. `spawn_ime_overlay_once` pre-spawns
  a small initial capacity (16) of hidden `ImeGraphemeCell` nodes and records them.
- `position_ime_overlay` assigns `placements[i] → pool[i]`, growing the pool via
  `Commands` when `placements.len() > pool.len()`. Newly grown nodes are spawned
  **already configured** (text + `left`/`top` + `Display::Flex`) so only the tail
  of an unusually long composition appears one frame late on the growth frame —
  the same one-frame cosmetic latency the existing anchor NOTE already documents.
  No upper cap.
- Unused pool nodes are set to `Display::None` each frame (same default-hide
  pattern the system already uses for caret/clause).

### System (gather → decide → apply)

`position_ime_overlay` stays the single apply system but now:

1. **decide (pure):** `compute_overlay_pos` (origin), `layout_preedit_cells`
   (per-cell placements), `caret_cell_offsets` (caret/clause) — all unchanged or
   new pure helpers.
2. **apply:** set the background rect (`left`, `top`, `width`, `height`,
   `display`), assign placements to pool nodes, position the underline bar, and the
   caret/clause (math unchanged). Grow the pool via `Commands` if short.

Compute `total_width_logical = total_cells as f32 * cell_w_logical` (free — the
per-cell walk already yields `total_cells`) and pass it into `compute_overlay_pos`
as `measured_width_logical` to activate the function's existing-but-dormant
right/left edge clamp. This removes the `measured_width_logical = 0.0` MVP
shortcut and its documented 1-frame clamp-lag NOTE (`ime_overlay.rs:237-247`); the
overlay now clamps to the pane edge the same frame the composition grows.

All component writes stay **equality-guarded** (`if node.left != … { … }`,
`if cell_text.0 != … { … }`, **including the per-node `Display` show/hide** — the
current code writes `Display` unconditionally) so change detection only fires on
real changes, matching the existing `if root_text.0 != comp.text()` and the repo's
"mutate conditionally" rule.

## Tests

- `layout_preedit_cells` unit tests (no `App`): ASCII, fullwidth CJK, mixed
  ASCII+CJK, a combining-mark cluster, and empty/whitespace.
- System-level (`position_ime_overlay`): pool nodes receive `left` values equal to
  the cell arithmetic, and the caret bar's `left` equals the trailing grapheme's
  cell boundary (= suffix) for an ASCII and a CJK composition.
- Update existing tests that assumed `ImeOverlayNode` carries the composition text
  (`overlay_background_matches_pane_default_bg_while_composing` keeps its
  background/`Display::Flex` assertions; the text-content expectation moves to the
  pool).
- Existing `caret_cell_offsets`, `compute_overlay_pos`, and `apply_event` tests
  stay green unchanged.

## Out of scope

- Per-pixel cell parity (scaling/centering glyphs within their cell span).
- The OS candidate-window anchor (`ime_policy_system`) — already cursor-anchored
  and unaffected.
- Any renderer/engine change; the terminal grid path is untouched.

## Risks

- **Pool growth one-frame lag** on compositions longer than the initial capacity —
  cosmetic, bounded to the growth frame's tail, documented with a `// NOTE:`.
- **Grapheme vs. VT-engine clustering divergence:** the overlay clusters the raw
  winit preedit string, while the committed text re-clusters through the VT engine.
  For normal IME input (Japanese kana/kanji, ASCII) the cell counts match; exotic
  ZWJ/emoji sequences could differ by a cell, but those are not produced by CJK IME
  composition and degrade gracefully (still grid-anchored, no accumulation).

## Spec-review notes

Resolved during the parallel Codex + Claude-Code spec review (2026-06-26):

- **Root-cause (a) `FontHinting` mechanism is correct.** A reviewer suggested UI
  `Text` defaults to `FontHinting::Enabled` (which would x-snap and weaken (a)).
  Refuted against source: `bevy_ui-0.18.0/src/widget/text.rs:98-109` declares
  `Text` with `#[require(… FontHinting::Disabled)]`, so the preedit body is **not**
  x-snapped and (a) stands. (`Text2d` is the one that defaults to `Enabled`.)
- **`BackgroundColor` is auto-required by `Node`.** A reviewer flagged that
  `spawn_ime_overlay_once` never spawns `BackgroundColor`. It is injected
  automatically: `bevy_ui-0.18.0/src/ui_node.rs:482-493` declares `Node` with
  `#[require(… BackgroundColor …)]`. The repurposed background node needs no
  explicit `BackgroundColor` insert.
