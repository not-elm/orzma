# CJK Fallback Glyph Sizing + `font.size` Config Wiring — Design

- **Date:** 2026-06-24
- **Branch:** `cjk-font`
- **Status:** Approved (brainstorming) — pending spec review → implementation plan
- **Scope:** Two independent fixes in `crates/ozma_tty_renderer` (+ binary wiring in `src/`):
  1. **Part 1** — CJK fallback glyphs render ~13.7% too large/bold; fix the rasterization scale.
  2. **Part 2** — `font.size` config value is ignored; the renderer hardcodes `12.0`. Wire it through.

The two parts share this spec but are implemented as separate, individually-revertable steps.

---

## Part 1 — CJK fallback em-match fix

### Problem

Japanese (CJK) text renders slightly **bolder** and **larger** than the Latin
alphabet in the same grid (reported visually; `bug.png`). The "bold" appearance
is a side effect of the oversize, not a separate weight problem.

### Root cause

The glyph atlas rasterizes **every** glyph — primary and fallback — at a single
`ab_glyph::PxScale` derived **only** from the primary regular face (JetBrains
Mono):

- `GlyphAtlas::get_or_insert` computes `scale = PxScale::from(fonts.px_scale_value(key.size_px))`
  (`crates/ozma_tty_renderer/src/glyph/atlas.rs:123`).
- `px_scale_value` = `phys_size_px × (ascender − descender) / units_per_em` of
  `self.regular` only (`crates/ozma_tty_renderer/src/glyph/font.rs:360-372`).
- `resolve_glyph` hands that **same** primary-derived `scale` to the fallback
  face (UDEVGothic35) when the primary lacks the glyph
  (`crates/ozma_tty_renderer/src/glyph/atlas.rs:72-89`), under the now-false
  assertion "UDEVGothic35 is JBM-metric-compatible by design"
  (`atlas.rs:58-66`).

`ab_glyph::PxScale` is interpreted as the **text height**, which ab_glyph
normalizes by each font's *own* `height_unscaled = (ascent − descent)` in font
units. Handing the primary's `PxScale` to a fallback face whose
`(ascent−descent)/units_per_em` ratio differs therefore resizes the fallback's
em-square.

**Verified font metrics** (parsed directly from the bundled TTF `head`/`hhea`
tables, 2026-06-24):

| Font | upem | ascender | descender | `em_scale = (asc−desc)/upem` |
| --- | --- | --- | --- | --- |
| JetBrainsMonoNerdFontMono-Regular | 1000 | 1020 | −300 | **1.320000** |
| UDEVGothic35-Regular | 2048 | 1962 | −416 | **1.161133** |

- At 12 px, the atlas uses `PxScale = 12 × 1.320000 = 15.84`.
- ab_glyph renders the JBM em-square at exactly `12.0 px` (self-consistent).
- ab_glyph renders the UDEVGothic35 em-square at
  `2048 × 15.84 / 2378 = 13.6419 px` — an inflation of **+13.68%**
  (`1.320000 / 1.161133 = 1.136821`).

The renderer draws atlas bitmaps 1:1 (the shader samples each glyph at its native
rasterized size; there is no fit-to-cell rescale), so the oversized bitmap
reaches the screen at full size. Wide (width = 2) CJK cells get a two-cell budget,
so the oversize manifests as apparent size/weight rather than clipping.

### Decision

**Em-match (standard).** Rasterize the fallback face at its **own** em-matched
scale so its em-square equals the same physical pixel size as the primary's. This
is the Alacritty / WezTerm / Ghostty default. CJK ideographs will remain
naturally somewhat larger than Latin *lowercase* (ideographs fill the em-square;
Latin x-height is ~half the em) — that is correct and expected. The fix removes
only the spurious +13.68%.

Rejected alternatives:
- **Tunable per-fallback scale knob** (WezTerm-style) — YAGNI for now; em-match
  is expected to be sufficient. Can be added later if the eye disagrees.
- **Shrink CJK to Latin cap-height** — non-standard; risks CJK looking too small.

### Approach (chosen: derive scale from the resolved face)

The rasterization scale must come from the face being rasterized. Refinement:
all four *fallback* faces share **`fallback_regular`'s** `em_scale` (not each its
own), mirroring how all four *primary* faces already share `regular`'s scale.
This keeps CJK regular/bold the same size and is robust if UDEV's bold metrics
differ slightly. Only **two** scale values are needed, not eight.

Rejected alternatives:
- **Single precomputed correction factor** (`fallback_em / primary_em ≈ 0.8796`)
  — smallest diff, but assumes all four fallback faces share `fallback_regular`'s
  metrics and must be recomputed when the primary is overridden; it is just the
  chosen approach hard-coded to two faces.
- **Per-glyph TTF parse of the resolved face** — re-parses on every glyph
  cache-miss; the chosen approach reuses the cheap on-demand parse pattern that
  already exists for the primary.

### Implementation

**`crates/ozma_tty_renderer/src/glyph/font.rs`**
- Extract a private `em_scale_of(font: &FontArc) -> f32` (the `(asc−desc)/upem`
  computation currently inlined in both `px_scale_value` and `cell_metrics_px`)
  and use it for both `px_scale_value` and the new `fallback_px_scale_value`,
  removing the duplicated formula and its drift risk. Keep the existing
  `ttf_parser` source so the primary path stays numerically identical.
  *Optional refinement (evaluate at implementation time):* compute `em_scale_of`
  from ab_glyph's own `font.height_unscaled() / font.units_per_em()` instead —
  `height_unscaled()` is exactly `ascent − descent` in font units and is the
  value ab_glyph's `v_scale_factor` itself uses, so it is tautologically
  consistent with the rasterizer and needs no second parse. Adopt it only behind
  a unit test asserting `em_scale_of(face)` equals the `ttf_parser`
  `(asc−desc)/upem` the primary metrics path uses (guards against any
  ab_glyph/ttf-parser metric-source divergence, e.g. OS/2 typo vs hhea, and the
  `Option` from `units_per_em()`).
- Add `pub(crate) fn fallback_px_scale_value(&self, phys_size_px: u16) -> f32`
  → `phys × em_scale_of(&self.fallback_regular)`, mirroring `px_scale_value`
  (which keeps reading `self.regular`). No struct-field caching: `em_scale_of`
  is cheap (`ttf_parser::Face::parse` reads the table directory lazily — not a
  full 13 MB parse), and these helpers run on glyph **cache-misses** *and*
  during cell-metrics recomputation — note `px_scale_value` is **also** called
  by `cell_metrics_px` (Startup metrics init and DPR-change material rebuilds),
  so it is **not** invoked only on glyph cache-misses. Caching would force
  touching `cell_metrics_px`, which is out of scope.

**`crates/ozma_tty_renderer/src/glyph/atlas.rs`**
- `resolve_glyph`: drop the `scale` parameter (glyph-id lookup is
  scale-independent — use `ab_glyph::Font::glyph_id` directly, already in
  scope), and return `(&FontArc, ab_glyph::GlyphId, bool /* used_fallback */)`.
- `get_or_insert`: resolve first, then pick the scale —
  `fonts.fallback_px_scale_value(key.size_px)` when `used_fallback`, else
  `fonts.px_scale_value(key.size_px)` — build the `PxScale`, and outline as
  before.
- Rewrite the now-false doc comment on `resolve_glyph` (`atlas.rs:58-66`): the
  "UDEVGothic35 is JBM-metric-compatible by design / both faces scaled at the
  primary's PxScale" claim becomes "each face is scaled at its own em-matched
  PxScale; the fallback uses `fallback_px_scale_value` so its em-square matches
  the primary's physical size."

### What stays byte-identical

Cell metrics (pitch, ascent/descent, line-height, underline), `max_overflow_phys`
(ASCII / primary only), and the entire primary glyph path. Baseline alignment is
preserved automatically: `offset_y` comes from the same outline, now at the
correct smaller scale, so CJK sits on the same baseline at its true size.

### Tests (Part 1)

- `fallback_px_scale_value(N)` ≈ `N × 1.161133`, and is ~12% **smaller** than
  `px_scale_value(N)` (≈ `N × 1.320000`) — the direct proof the inflation is
  gone. (Assert the ratio `≈ 0.8796`, not absolute pixels, so a font re-vendor
  updates one tolerance.)
- Regression: 'あ' (U+3042) rasterizes at the fallback scale — assert its rect
  height matches the fallback-scaled outline and is strictly smaller than the
  old primary-scaled height.
- Guard: a primary Latin glyph still rasterizes via `px_scale_value` (unchanged
  rect), and U+E0B0 (Nerd Font PUA) stays on the primary path
  (`used_fallback == false`).
- Existing `cjk_renders_through_fallback`, `latin_renders_through_primary`,
  `nerd_font_pua_stays_on_primary`, and `unknown_codepoint_returns_none` all
  keep passing (the CJK rect is simply smaller).

---

## Part 2 — Wire `font.size` into the renderer

### Problem

The config exposes `font.size` (default `11.25`,
`crates/ozmux_configs/src/font.rs:6,11-13`) but the renderer ignores it and
hardcodes `FONT_SIZE_PX = 12.0` (`crates/ozma_tty_renderer/src/glyph/font.rs:28`),
with an explicit `// TODO: load font size from config`
(`crates/ozma_tty_renderer/src/material.rs:487`). Changing `font.size` in config
has no effect today.

### Decision

- **Honor the config default (11.25).** Because the renderer currently hardcodes
  `12.0`, the moment config drives size the default rendered text becomes ~6%
  smaller. This is accepted — it is the config working as documented; users set
  their own size to taste.
- **Treat `size` as logical px (Alacritty model).** In this codebase
  `size × scale_factor = device px`, which *is* Alacritty's logical-px model. No
  `×96/72` point→px conversion (that would double-count DPI and inflate text to
  ~15 px). The `FontConfig.size` doc comment is clarified accordingly; the
  default `11.25` and its test are unchanged.

### Implementation

- **New resource `TerminalFontSize(pub f32)`** (logical px), defined in
  `crates/ozma_tty_renderer/src/glyph/font.rs` next to `FONT_SIZE_PX`,
  `TerminalFonts`, and `TerminalFontPlugin` (registration locality). It needs a
  `///` doc comment (it is externally `pub`). Its `Default` returns
  `Self(FONT_SIZE_PX)` = **12.0**, so the renderer standalone and all of its
  existing tests are unaffected. `TerminalFontPlugin` `init_resource`s it.
- **`bridge_font_config`** (`src/font.rs:112-175`) takes
  `ResMut<TerminalFontSize>` and sets `font_size.0 = configs.font.size`. Placed
  **before** the `no_override` early-return (`src/font.rs:127-134`) so size is
  honored even with no font-path override. Direct `ResMut` mutation (like the
  existing `*terminal_fonts = …`) makes it immediately visible to
  `init_cell_metrics_from_primary_window`, which already runs `.after` via the
  `TerminalFontInitSet::InitCellMetrics` ordering (`bridge_font_config` is
  registered `.before(InitCellMetrics)`). Keep mutable params first:
  `ResMut<TerminalFontSize>` joins the existing `ResMut` group
  (`commands`/`fonts_assets`/`terminal_fonts`) ahead of the immutable
  `configs: Res<…>` (`.claude/rules/rust.md` mutable-params-first).
- **`init_cell_metrics_from_primary_window`** (`font.rs:69-81`) and
  **`update_terminal_material`** (`material.rs:515`) read
  `Res<TerminalFontSize>` instead of the constant; remove the
  `// TODO: load font size from config` (`material.rs:487`).
- **`src/ui/ime_overlay.rs:368`** reads `TerminalFontSize` instead of
  `FONT_SIZE_PX`, so the IME preedit overlay matches terminal text size (the
  only other live reader of the constant). `COPY_MODE_INDICATOR_FONT_SIZE_PX`
  (`src/theme.rs:55`) is independent UI chrome and is left alone.
- **`crates/ozmux_configs/src/font.rs`**: clarify the `size` doc comment (logical
  px scaled by DPR — Alacritty's model — not literal typographic points).
  Default stays `11.25`; its test stays.
- Export `TerminalFontSize` from `crates/ozma_tty_renderer/src/lib.rs`, and
  **demote `FONT_SIZE_PX`** from the crate's public re-export to a `pub(crate)`
  backing constant once its three readers migrate to the resource — after
  migration nothing outside the crate reads it, so the visibility-minimization
  rule applies. `TerminalFontSize` becomes the public size API.

### Why no hot-reload concern

Font size is set once at Startup, exactly like font *face* overrides today (the
`FontBridgePlugin` module doc already states font changes require a restart).
`update_terminal_material`'s existing `phys_size_changed` path still correctly
handles DPR changes (window moved between displays). "Startup-only" refers to
*writes*: `update_terminal_material` **reads** `Res<TerminalFontSize>` every
frame to recompute `phys_font_size`, so the resource must live for the whole app
lifetime — it is `init_resource`-d by `TerminalFontPlugin` and never removed.

### Tests (Part 2)

- `TerminalFontSize` default is `12.0` (renderer crate, no bridge).
- Bridge honors `size`: config `size = 14.0` → `TerminalFontSize.0 == 14.0` and
  `phys_font_size == 14` at DPR 1.
- The early-return (no font override) path **still** sets size: config with only
  `size = 16.0` and no font paths → `TerminalFontSize.0 == 16.0`.
- Default config → `TerminalFontSize.0 == 11.25`.

---

## Out of scope

- A user-configurable per-fallback scale knob (WezTerm-style).
- Changing `cell_metrics_px` to cache em-scales (would touch the primary metrics
  path for no behavior change).
- Caching em-scales as struct fields (the on-demand parse is cheap enough).
- Any change to wide-cell layout, baseline policy, or the shader.

## Risks

- **Default text size shifts ~6% smaller** (12.0 → 11.25) once Part 2 lands —
  intended, documented here so it is not mistaken for a regression.
- **Font re-vendor changes the metrics.** Part 1 tests assert the
  fallback/primary scale *ratio* (≈ 0.8796) rather than absolute pixels, so a
  re-vendor updates one tolerance, not a hardcoded constant.
- The two parts are independent (separate files, separate failure modes); the
  implementation plan keeps them as separate steps so either can be reverted
  alone.
