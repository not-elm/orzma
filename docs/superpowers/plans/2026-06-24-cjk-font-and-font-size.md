# CJK Fallback Sizing + `font.size` Config Wiring Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop CJK fallback glyphs from rendering ~13.7% too large/bold by rasterizing the fallback face at its own em-matched scale, and wire the ignored `font.size` config value into the renderer.

**Architecture:** Two independent fixes in `crates/ozma_tty_renderer` plus binary wiring in `src/`. Part 1 makes the glyph atlas pick the rasterization `PxScale` from the *resolved* face (primary keeps the primary-regular scale; fallback uses the fallback-regular scale). Part 2 introduces a `TerminalFontSize` resource that `FontBridgePlugin` fills from `config.font.size` and the renderer reads instead of the hardcoded `FONT_SIZE_PX`. The parts are separate commits so either can be reverted alone.

**Tech Stack:** Rust 2024, Bevy 0.18 ECS, `ab_glyph` 0.2 (glyph rasterization), `ttf_parser` (OpenType metrics).

**Spec:** `docs/superpowers/specs/2026-06-24-cjk-font-and-font-size-design.md`

## Global Constraints

- Rust edition 2024, toolchain 1.95; Bevy 0.18 (copied from `CLAUDE.md` / `rust-toolchain.toml`).
- All in-code comments in English. Only `// TODO:` / `// NOTE:` / `// SAFETY:` line-comment forms (`.claude/rules/rust.md`).
- Every externally `pub` item gets a `///` doc comment; module files keep their `//!`.
- Visibility minimization: any item used only inside its defining module MUST be private.
- Mutable function/system params declared before immutable ones (`self`/`On<E>` excepted).
- `Plugin::build` bodies are a single `app.` method chain (an `if` guard may precede it).
- Systems/observers are registered by the `Plugin` defined in the same file.
- No `mod.rs`. No manual `set_changed()` / `bypass_change_detection()`.
- TDD: failing test first, minimal impl, green, commit. Conventional-commit messages ending with:
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- Verified font metrics (do not re-derive): JBM-Regular `em_scale = (1020 − −300)/1000 = 1.320000`; UDEVGothic35-Regular `em_scale = (1962 − −416)/2048 = 1.161133`; fallback/primary ratio `= 0.879646`.

---

## File Structure

| File | Responsibility | Tasks |
| --- | --- | --- |
| `crates/ozma_tty_renderer/src/glyph/font.rs` | `em_scale_of`, `px_scale_value`, new `fallback_px_scale_value`, new `TerminalFontSize` resource, `TerminalFontPlugin`, `init_cell_metrics_from_primary_window` | 2, 3, 5 |
| `crates/ozma_tty_renderer/src/glyph/atlas.rs` | `resolve_glyph` + `get_or_insert` pick per-face scale | 2 |
| `crates/ozma_tty_renderer/src/material.rs` | `update_terminal_material` reads `TerminalFontSize` | 3 |
| `crates/ozma_tty_renderer/src/lib.rs` | export `TerminalFontSize`; drop `FONT_SIZE_PX` export | 3, 5 |
| `src/font.rs` | `bridge_font_config` sets `TerminalFontSize` from config | 4 |
| `src/ui/ime_overlay.rs` | IME overlay reads `TerminalFontSize` | 5 |

---

## Task 1: Commit design artifacts

Put the approved spec and this plan under version control before touching code, so the design is recoverable independently of the implementation.

**Files:**
- Add: `docs/superpowers/specs/2026-06-24-cjk-font-and-font-size-design.md`
- Add: `docs/superpowers/plans/2026-06-24-cjk-font-and-font-size.md`

- [ ] **Step 1: Confirm on the feature branch**

Run: `git rev-parse --abbrev-ref HEAD`
Expected: `cjk-font`

- [ ] **Step 2: Commit the docs**

`docs/` is gitignored, but ~101 design docs are tracked here via force-add (repo convention; see `CLAUDE.md` "docs/ — tracked in git"). Use `-f`:

```bash
git add -f docs/superpowers/specs/2026-06-24-cjk-font-and-font-size-design.md \
           docs/superpowers/plans/2026-06-24-cjk-font-and-font-size.md
git commit -m "docs: CJK fallback sizing + font.size wiring design and plan

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Part 1 — CJK fallback em-match fix

Rasterize fallback (CJK) glyphs at the fallback-regular face's own em-matched scale instead of the primary's, removing the +13.68% inflation. The primary path and all cell metrics are unchanged.

**Files:**
- Modify: `crates/ozma_tty_renderer/src/glyph/font.rs` (add `em_scale_of`, refactor `px_scale_value`, add `fallback_px_scale_value`)
- Modify: `crates/ozma_tty_renderer/src/glyph/atlas.rs` (`resolve_glyph` signature + `get_or_insert` scale selection + imports + doc)
- Test: same two files (`#[cfg(test)] mod tests`)

**Interfaces:**
- Produces: `TerminalFonts::fallback_px_scale_value(&self, phys_size_px: u16) -> f32` (`pub(crate)`); free fn `em_scale_of(font: &FontArc) -> f32` (module-private); `resolve_glyph(...) -> Option<(&FontArc, ab_glyph::GlyphId, bool)>` where the `bool` is `used_fallback`.

- [ ] **Step 1: Write the failing unit test for `fallback_px_scale_value`**

Add to `crates/ozma_tty_renderer/src/glyph/font.rs` inside `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn fallback_px_scale_value_is_em_matched_and_smaller_than_primary() {
        let fonts = TerminalFonts::default();
        let phys = 12u16;
        let primary = fonts.px_scale_value(phys);
        let fallback = fonts.fallback_px_scale_value(phys);
        // UDEVGothic35 em_scale (1.161133) is ~12% smaller than JBM (1.320000);
        // the fallback must rasterize correspondingly smaller, not at the
        // primary's scale (which inflated CJK by +13.68%).
        let ratio = fallback / primary;
        assert!(
            (ratio - 0.879646).abs() < 0.001,
            "fallback/primary px_scale ratio = {ratio} (expected ~0.879646)"
        );
    }
```

- [ ] **Step 2: Run it to verify it fails to compile**

Run: `cargo test -p ozma_tty_renderer fallback_px_scale_value_is_em_matched -- --nocapture`
Expected: FAIL — `no method named fallback_px_scale_value`.

- [ ] **Step 3: Extract `em_scale_of` and add `fallback_px_scale_value`**

In `crates/ozma_tty_renderer/src/glyph/font.rs`, add this module-private free function next to `max_ascii_overflow_for_face` (above the `impl TerminalFonts` block):

```rust
/// Returns a face's em-scale `(ascender − descender) / units_per_em` — the
/// factor that maps a physical font size in pixels to the `ab_glyph::PxScale`
/// whose em-square renders at exactly that pixel size.
fn em_scale_of(font: &FontArc) -> f32 {
    let face = TtfFace::parse(font.font_data(), 0)
        .expect("ttf-parser parse failed for a face ab_glyph already accepted");
    let asc = i32::from(face.ascender());
    let desc = i32::from(face.descender());
    let upem = f32::from(face.units_per_em());
    (asc - desc) as f32 / upem
}
```

Replace the body of `px_scale_value` (currently parses `self.regular` inline) with the extracted helper, and add `fallback_px_scale_value` directly after it:

```rust
    pub(crate) fn px_scale_value(&self, phys_size_px: u16) -> f32 {
        f32::from(phys_size_px) * em_scale_of(&self.regular)
    }

    /// Returns the `PxScale` value for the CJK fallback face so its em-square
    /// renders at the same physical pixel size as the primary's, preventing the
    /// fallback from rasterizing larger than the grid expects. Mirrors
    /// [`Self::px_scale_value`] but reads `self.fallback_regular`'s metrics.
    pub(crate) fn fallback_px_scale_value(&self, phys_size_px: u16) -> f32 {
        f32::from(phys_size_px) * em_scale_of(&self.fallback_regular)
    }
```

Delete the old doc comment block above `px_scale_value` that describes the inline parse, replacing it with a one-line summary (the detailed em-scale explanation now lives on `em_scale_of`):

```rust
    /// Returns the `ab_glyph::PxScale` value for the primary regular face at the
    /// given physical pixel size. Used by `cell_metrics_px` and `glyph/atlas.rs`.
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p ozma_tty_renderer fallback_px_scale_value_is_em_matched`
Expected: PASS.

- [ ] **Step 5: Write the failing atlas regression test**

Add to `crates/ozma_tty_renderer/src/glyph/atlas.rs` inside `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn cjk_rasterizes_at_fallback_scale_not_primary_scale() {
        use ab_glyph::Font as _;
        let fonts = TerminalFonts::default();
        let mut atlas = GlyphAtlas::default();
        let size = 24u16;
        let key = make_key(FontFace::Regular, 0x3042, size); // 'あ'
        let rect = atlas
            .get_or_insert(key, &fonts)
            .expect("'あ' must rasterize via fallback");

        // Height the buggy code produced: the fallback outline at the PRIMARY
        // scale. The fix must rasterize 'あ' strictly smaller than this.
        let fb = fonts.fallback_choice(&FontFace::Regular);
        let primary_scale = ab_glyph::PxScale::from(fonts.px_scale_value(size));
        let gid = fb.glyph_id('あ');
        let buggy_h = fb
            .outline_glyph(gid.with_scale(primary_scale))
            .expect("'あ' outline at primary scale")
            .px_bounds()
            .height();

        assert!(
            (rect.h as f32) < buggy_h - 0.5,
            "'あ' rect height {} must be smaller than primary-scaled height {buggy_h}",
            rect.h
        );
    }
```

- [ ] **Step 6: Run it to verify it fails**

Run: `cargo test -p ozma_tty_renderer cjk_rasterizes_at_fallback_scale_not_primary_scale`
Expected: FAIL — at HEAD the atlas still scales the fallback at the primary scale, so `rect.h == buggy_h` (assertion fails). (It will also fail to compile if Step 7's signature change is partially applied — run after Step 5 only.)

- [ ] **Step 7: Rewire `resolve_glyph` and `get_or_insert`**

In `crates/ozma_tty_renderer/src/glyph/atlas.rs`, change the import line 1 (drop now-unused `ScaleFont`):

```rust
use ab_glyph::{Font, FontArc, OutlinedGlyph};
```

Replace `resolve_glyph` (drop the `scale` param, return the `used_fallback` flag, look up glyph ids unscaled), and rewrite its doc comment:

```rust
/// Resolves a glyph for the requested codepoint, preferring the primary face
/// and falling back to `fallback_choice` when the primary's `glyph_id` is 0
/// (notdef).
///
/// Returns `(font, glyph_id, used_fallback)` for the resolved face, or `None`
/// when neither primary nor fallback contains the glyph. The `used_fallback`
/// flag tells `get_or_insert` which em-matched `PxScale` to rasterize at: the
/// primary's (`px_scale_value`) or the fallback's (`fallback_px_scale_value`),
/// so each face renders its em-square at the same physical pixel size.
///
/// `glyph_id` lookup is scale-independent, so this resolves before any scale is
/// chosen.
///
/// NOTE: retries on `glyph_id == 0` only — NOT on degenerate outline
/// (`w == 0 || h == 0`), which `get_or_insert` still short-circuits after
/// outlining. PUA Nerd Font icons (U+E000–U+F8FF) resolve non-zero on the
/// primary, so they never reach the fallback.
fn resolve_glyph<'a>(
    fonts: &'a TerminalFonts,
    face: &FontFace,
    ch: char,
) -> Option<(&'a FontArc, ab_glyph::GlyphId, bool)> {
    let primary = fonts.choice(face);
    let id = primary.glyph_id(ch);
    if id.0 != 0 {
        return Some((primary, id, false));
    }
    let fallback = fonts.fallback_choice(face);
    let id = fallback.glyph_id(ch);
    if id.0 != 0 {
        return Some((fallback, id, true));
    }
    None
}
```

In `get_or_insert`, replace the three lines that compute `scale` then resolve:

```rust
        let ch = char::from_u32(key.codepoint)?;
        let (font, glyph_id, used_fallback) = resolve_glyph(fonts, &key.face, ch)?;
        let scale_value = if used_fallback {
            fonts.fallback_px_scale_value(key.size_px)
        } else {
            fonts.px_scale_value(key.size_px)
        };
        let scale = ab_glyph::PxScale::from(scale_value);

        let outlined = font.outline_glyph(glyph_id.with_scale(scale))?;
```

- [ ] **Step 8: Run the full renderer test suite**

Run: `cargo test -p ozma_tty_renderer`
Expected: PASS — including `cjk_rasterizes_at_fallback_scale_not_primary_scale`, `fallback_px_scale_value_is_em_matched_and_smaller_than_primary`, and the existing `cjk_renders_through_fallback`, `latin_renders_through_primary`, `nerd_font_pua_stays_on_primary`, `unknown_codepoint_returns_none`, and the `cell_metrics_px` tests (cell metrics are untouched).

- [ ] **Step 9: Lint**

Run: `cargo clippy -p ozma_tty_renderer`
Expected: no warnings (confirms `ScaleFont` is no longer imported unused).

- [ ] **Step 10: Commit**

```bash
git add crates/ozma_tty_renderer/src/glyph/font.rs \
        crates/ozma_tty_renderer/src/glyph/atlas.rs
git commit -m "fix(renderer): rasterize CJK fallback glyphs at the fallback's em-matched scale

CJK glyphs rendered ~13.7% too large/bold because the fallback face was
rasterized with the primary font's PxScale. ab_glyph normalizes PxScale by
each font's own (ascent - descent), so reuse inflated UDEVGothic35's em.
resolve_glyph now reports whether it fell back, and get_or_insert picks
fallback_px_scale_value vs px_scale_value accordingly.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Part 2a — `TerminalFontSize` resource; renderer reads it

Introduce a `TerminalFontSize` resource (default `12.0`) that the renderer reads instead of the `FONT_SIZE_PX` constant. `FONT_SIZE_PX` stays `pub` for now (the binary's IME overlay still references it until Task 5).

**Files:**
- Modify: `crates/ozma_tty_renderer/src/glyph/font.rs` (resource + `Default` + plugin `init_resource` + `init_cell_metrics_from_primary_window`)
- Modify: `crates/ozma_tty_renderer/src/material.rs` (`update_terminal_material` param + usage + import)
- Modify: `crates/ozma_tty_renderer/src/lib.rs` (export `TerminalFontSize`)
- Test: `crates/ozma_tty_renderer/src/glyph/font.rs`

**Interfaces:**
- Produces: `pub struct TerminalFontSize(pub f32)` with `Default` = `Self(FONT_SIZE_PX)`; init by `TerminalFontPlugin`; read by `init_cell_metrics_from_primary_window` and `update_terminal_material`.

- [ ] **Step 1: Write the failing test**

Add to `crates/ozma_tty_renderer/src/glyph/font.rs` inside `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn init_cell_metrics_honors_terminal_font_size_resource() {
        use bevy::window::{PrimaryWindow, Window, WindowResolution};

        let mut app = App::new();
        let mut window = Window {
            resolution: WindowResolution::new(800, 600),
            ..default()
        };
        window.resolution.set_scale_factor(1.0);
        app.world_mut().spawn((window, PrimaryWindow));
        // Pre-insert so the plugin's init_resource keeps this value.
        app.insert_resource(TerminalFontSize(10.0));
        app.add_plugins(TerminalFontPlugin);
        app.update();

        let res = app
            .world()
            .get_resource::<TerminalCellMetricsResource>()
            .expect("Startup system should insert TerminalCellMetricsResource");
        assert_eq!(
            res.phys_font_size, 10,
            "phys_font_size must follow TerminalFontSize (10.0) at DPR 1.0"
        );
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p ozma_tty_renderer init_cell_metrics_honors_terminal_font_size_resource`
Expected: FAIL — `cannot find ... TerminalFontSize`.

- [ ] **Step 3: Add the `TerminalFontSize` resource**

In `crates/ozma_tty_renderer/src/glyph/font.rs`, immediately after the `FONT_SIZE_PX` constant (line 28):

```rust
/// Logical (CSS) pixel font size for the terminal grid. Multiplied by the
/// PrimaryWindow's `scale_factor` to obtain the physical pixel size fed to
/// `cell_metrics_px` and the glyph atlas.
///
/// Defaults to [`FONT_SIZE_PX`]; the app's `FontBridgePlugin` overwrites it
/// from `config.font.size` at Startup, before cell metrics are computed.
#[derive(Resource, Clone, Copy, Debug)]
pub struct TerminalFontSize(pub f32);

impl Default for TerminalFontSize {
    fn default() -> Self {
        Self(FONT_SIZE_PX)
    }
}
```

- [ ] **Step 4: Init the resource in `TerminalFontPlugin::build` and read it in `init_cell_metrics_from_primary_window`**

Change `TerminalFontPlugin::build` so the `add_systems` call chains off `init_resource`:

```rust
        app.init_resource::<TerminalFontSize>().add_systems(
            Startup,
            init_cell_metrics_from_primary_window.in_set(TerminalFontInitSet::InitCellMetrics),
        );
```

Update `init_cell_metrics_from_primary_window`'s signature and `phys_font_size`:

```rust
fn init_cell_metrics_from_primary_window(
    mut commands: Commands,
    fonts: Res<TerminalFonts>,
    font_size: Res<TerminalFontSize>,
    window: Single<&Window, With<PrimaryWindow>>,
) {
    let dpr = window.scale_factor();
    let phys_font_size = (font_size.0 * dpr).round() as u16;
    let metrics = fonts.cell_metrics_px(phys_font_size);
    commands.insert_resource(TerminalCellMetricsResource {
        metrics,
        phys_font_size,
    });
}
```

- [ ] **Step 5: Run the new test and the existing metrics test**

Run: `cargo test -p ozma_tty_renderer init_cell_metrics`
Expected: PASS — both `init_cell_metrics_honors_terminal_font_size_resource` and the existing `init_cell_metrics_from_primary_window_uses_window_scale_factor` (default `TerminalFontSize` = 12.0 → phys 24 at DPR 2.0).

- [ ] **Step 6: Read `TerminalFontSize` in `update_terminal_material`**

In `crates/ozma_tty_renderer/src/material.rs`, update the import (line 4) to drop `FONT_SIZE_PX` and add `TerminalFontSize`:

```rust
        font::{FontFace, GlyphKey, TerminalCellMetricsResource, TerminalFontSize, TerminalFonts},
```

Add the `font_size` param to `update_terminal_material` directly after `fonts`:

```rust
    fonts: Res<TerminalFonts>,
    font_size: Res<TerminalFontSize>,
```

Delete the `// TODO: load font size from config.` line, and change the `phys_font_size` computation (line 515):

```rust
    let phys_font_size = (font_size.0 * dpr).round() as u16;
```

- [ ] **Step 7: Export `TerminalFontSize`**

In `crates/ozma_tty_renderer/src/lib.rs`, add `TerminalFontSize` to the re-export (keep `FONT_SIZE_PX` for now):

```rust
pub use crate::glyph::font::{
    CellMetrics, FONT_SIZE_PX, FontFace, FontLoadError, TerminalCellMetricsResource,
    TerminalFontInitSet, TerminalFontPlugin, TerminalFontSize, TerminalFonts,
};
```

- [ ] **Step 8: Build, test, lint the crate**

Run: `cargo test -p ozma_tty_renderer && cargo clippy -p ozma_tty_renderer`
Expected: PASS, no warnings (confirms `material.rs` no longer imports `FONT_SIZE_PX` unused).

- [ ] **Step 9: Commit**

```bash
git add crates/ozma_tty_renderer/src/glyph/font.rs \
        crates/ozma_tty_renderer/src/material.rs \
        crates/ozma_tty_renderer/src/lib.rs
git commit -m "feat(renderer): add TerminalFontSize resource read in place of FONT_SIZE_PX

The renderer reads a TerminalFontSize resource (default 12.0) for the logical
font size instead of the hardcoded constant, so the app can drive it from
config. No behavior change yet — nothing overwrites the default.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Part 2b — `bridge_font_config` fills `TerminalFontSize` from config

Set `TerminalFontSize` from `config.font.size` at Startup, before the no-override early-return so size is honored even when no font face is overridden. Default config (`11.25`) now reaches the renderer.

**Files:**
- Modify: `src/font.rs` (`bridge_font_config` import, signature, body)
- Modify: `crates/ozmux_configs/src/font.rs` (clarify the `size` doc comment)
- Test: `src/font.rs`

**Interfaces:**
- Consumes: `TerminalFontSize` (from Task 3, re-exported by `ozma_tty_renderer`).

- [ ] **Step 1: Write the failing tests**

Add to `src/font.rs` inside `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn bridge_sets_terminal_font_size_from_default_config() {
        let (mut app, _guard, _env) = make_test_app();
        app.update();
        let size = app.world().resource::<TerminalFontSize>();
        assert_eq!(
            size.0, 11.25,
            "default config font.size (11.25) must reach TerminalFontSize"
        );
    }

    #[test]
    fn bridge_sets_terminal_font_size_from_config_override_without_font_paths() {
        let tmp = std::env::temp_dir().join("ozmux_font_size_override.toml");
        std::fs::write(&tmp, "[font]\nsize = 16.0\n").expect("write toml");

        let _guard = crate::configs::env_guard();
        let _env = EnvVarGuard::set("OZMUX_CONFIG", &tmp);
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(AssetPlugin::default())
            .add_plugins(TextPlugin)
            .init_asset::<Font>();
        let mut window = Window {
            resolution: WindowResolution::new(800, 600),
            ..default()
        };
        window.resolution.set_scale_factor(1.0);
        app.world_mut().spawn((window, PrimaryWindow));
        app.add_plugins(TerminalFontPlugin);
        app.add_plugins(OzmuxConfigsPlugin);
        app.add_plugins(FontBridgePlugin);
        app.update();

        let size = app.world().resource::<TerminalFontSize>();
        assert_eq!(
            size.0, 16.0,
            "config size=16 must reach TerminalFontSize even with no font override (cold path)"
        );

        let _ = std::fs::remove_file(&tmp);
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p ozmux --bin ozmux bridge_sets_terminal_font_size`
Expected: FAIL — `cannot find ... TerminalFontSize` / resource not present.

- [ ] **Step 3: Import `TerminalFontSize` in `src/font.rs`**

Change the renderer import line (currently `use ozma_tty_renderer::{FontFace, TerminalFontInitSet, TerminalFonts, bundled};`):

```rust
use ozma_tty_renderer::{FontFace, TerminalFontInitSet, TerminalFontSize, TerminalFonts, bundled};
```

- [ ] **Step 4: Add the `ResMut` param (mutable-first) and set the size**

Change `bridge_font_config`'s signature so all mutable params precede the immutable `configs`:

```rust
fn bridge_font_config(
    mut commands: Commands,
    mut fonts_assets: ResMut<Assets<Font>>,
    mut terminal_fonts: ResMut<TerminalFonts>,
    mut font_size: ResMut<TerminalFontSize>,
    configs: Res<OzmuxConfigsResource>,
) {
    font_size.0 = configs.font.size;
    let font: &FontConfig = &configs.font;
```

(The `font_size.0 = configs.font.size;` line is the first statement, ahead of the `no_override` early-return, so the cold path still applies it.)

- [ ] **Step 5: Clarify the `FontConfig.size` doc comment**

In `crates/ozmux_configs/src/font.rs`, replace the `size` field doc (line 12, currently `/// Terminal font size in points, matching Alacritty.`):

```rust
    /// Terminal font size in logical (CSS) pixels, scaled by the display's
    /// `scale_factor` to device pixels — Alacritty's model (not literal
    /// typographic points; no 96/72 conversion is applied).
```

The `DEFAULT_SIZE = 11.25` constant and the `default_size_matches_alacritty` test are unchanged.

- [ ] **Step 6: Run the tests**

Run: `cargo test -p ozmux --bin ozmux bridge_sets_terminal_font_size && cargo test -p ozmux_configs font`
Expected: PASS (both new bridge tests, and `ozmux_configs` font tests including `default_size_matches_alacritty`).

- [ ] **Step 7: Run the existing font-bridge suite**

Run: `cargo test -p ozmux --bin ozmux font::`
Expected: PASS — existing bridge tests (`default_config_keeps_bundled_jbm_in_terminal_fonts`, the override/corrupt-face tests, `cjk_fallback_registered_in_cosmic_fontdb`) still green.

- [ ] **Step 8: Commit**

```bash
git add src/font.rs crates/ozmux_configs/src/font.rs
git commit -m "feat: drive terminal font size from config.font.size

bridge_font_config writes config.font.size into TerminalFontSize before the
no-override early return, so the default 11.25 (and any user size) reaches the
renderer. Default rendered text is now 11.25 logical px (was a hardcoded 12.0).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Part 2c — IME overlay reads `TerminalFontSize`; demote `FONT_SIZE_PX`

Migrate the last `FONT_SIZE_PX` reader (the IME preedit overlay) to the resource so it matches terminal text size, then demote `FONT_SIZE_PX` to a module-private constant.

**Files:**
- Modify: `src/ui/ime_overlay.rs` (import, `spawn_ime_overlay_once` param + `font_size` field)
- Modify: `crates/ozma_tty_renderer/src/glyph/font.rs` (`pub const FONT_SIZE_PX` → private `const`)
- Modify: `crates/ozma_tty_renderer/src/lib.rs` (drop `FONT_SIZE_PX` from the re-export)
- Test: `src/ui/ime_overlay.rs`

**Interfaces:**
- Consumes: `TerminalFontSize`; `TerminalUiFont` (existing, `crate::font::TerminalUiFont`).

- [ ] **Step 1: Write the failing test**

Add to `src/ui/ime_overlay.rs` inside its existing `#[cfg(test)] mod tests` (line 426 — it already has `use super::*;` and `use bevy::prelude::MinimalPlugins;`):

```rust
    #[test]
    fn ime_overlay_uses_terminal_font_size() {
        use bevy::asset::Handle;
        use bevy::text::TextFont;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(crate::font::TerminalUiFont(Handle::default()));
        app.insert_resource(TerminalFontSize(9.0));
        app.add_systems(Startup, spawn_ime_overlay_once);
        app.update();

        let mut query = app.world_mut().query::<&TextFont>();
        let matched = query
            .iter(app.world())
            .any(|tf| (tf.font_size - 9.0).abs() < f32::EPSILON);
        assert!(
            matched,
            "IME overlay TextFont must use TerminalFontSize (9.0), not the constant"
        );
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p ozmux --bin ozmux ime_overlay_uses_terminal_font_size`
Expected: FAIL — the overlay still uses `ozma_tty_renderer::FONT_SIZE_PX` (12.0), so no `TextFont` has `font_size == 9.0`; or `TerminalFontSize` is unresolved in the test until Step 3's import.

- [ ] **Step 3: Migrate `spawn_ime_overlay_once`**

In `src/ui/ime_overlay.rs`, add to the import block (next to the other `use ozma_tty_renderer::...` lines):

```rust
use ozma_tty_renderer::TerminalFontSize;
```

Change `spawn_ime_overlay_once` to take the resource and use it:

```rust
fn spawn_ime_overlay_once(
    mut commands: Commands,
    ui_font: Res<TerminalUiFont>,
    font_size: Res<TerminalFontSize>,
) {
    let text_font = TextFont {
        font: ui_font.0.clone(),
        font_size: font_size.0,
        ..default()
    };
```

- [ ] **Step 4: Run the IME overlay test**

Run: `cargo test -p ozmux --bin ozmux ime_overlay_uses_terminal_font_size`
Expected: PASS.

- [ ] **Step 5: Demote `FONT_SIZE_PX` to module-private**

In `crates/ozma_tty_renderer/src/glyph/font.rs`, change line 28:

```rust
const FONT_SIZE_PX: f32 = 12.0;
```

(It is now referenced only by `TerminalFontSize::default()` in this same file, so the visibility-minimization rule requires it be private.)

In `crates/ozma_tty_renderer/src/lib.rs`, remove `FONT_SIZE_PX` from the re-export:

```rust
pub use crate::glyph::font::{
    CellMetrics, FontFace, FontLoadError, TerminalCellMetricsResource, TerminalFontInitSet,
    TerminalFontPlugin, TerminalFontSize, TerminalFonts,
};
```

- [ ] **Step 6: Build the whole workspace and run all tests**

Run: `cargo build && cargo test`
Expected: PASS — no unresolved `FONT_SIZE_PX` references anywhere (the only remaining mentions are the doc-comment / assertion-message strings in `font.rs` tests, which are plain text, not symbols).

- [ ] **Step 7: Lint the workspace**

Run: `cargo clippy --workspace`
Expected: no warnings (no unused imports; `FONT_SIZE_PX` private and used by `Default`).

- [ ] **Step 8: Commit**

```bash
git add src/ui/ime_overlay.rs \
        crates/ozma_tty_renderer/src/glyph/font.rs \
        crates/ozma_tty_renderer/src/lib.rs
git commit -m "refactor: IME overlay reads TerminalFontSize; make FONT_SIZE_PX private

The IME preedit overlay now sizes to TerminalFontSize so it matches the
terminal grid. With its last external reader migrated, FONT_SIZE_PX is demoted
to a module-private backing constant for TerminalFontSize::default().

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Final verification

- [ ] **Whole-workspace gate**

Run: `cargo test && cargo clippy --workspace && cargo fmt --check`
Expected: all green. (If `cargo fmt --check` reports diffs, run `cargo fmt` and amend the most recent commit.)

- [ ] **Manual visual check (optional, recommended)**

Run: `cargo run`
Expected: Japanese text in the grid is no longer noticeably larger/bolder than Latin; overall text is at the configured `font.size` (default 11.25 logical px).
