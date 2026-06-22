# ozmux App Icon — Design Spec

Date: 2026-06-22
Status: Approved (visual direction locked via the brainstorming visual companion)
Scope: macOS application icon set for the `ozmux.app` bundle.

## 1. Goal

Give `ozmux` a real application icon. Today `build/macos/Info.plist` ships no
`CFBundleIconFile`, so the bundled `.app` falls back to the generic macOS icon.
`scripts/bundle_macos.py` already looks for `build/macos/AppIcon.icns`
(lines ~217–229): if that file exists it sets `CFBundleIconFile = AppIcon.icns`
and copies it into `Contents/Resources/`. The deliverable is therefore the
artwork plus a reproducible pipeline that produces `build/macos/AppIcon.icns`.

## 2. The mark

A monogram: lowercase **`oz`** followed by a rounded **block cursor**. The
`oz` carries the brand (the `ozma`/`ozmux` family); the cursor signals
"terminal" without a literal prompt or pane drawing. Chosen over a literal
terminal glyph, an Oz-themed gem, and a split-pane illustration during visual
brainstorming.

- Letters: `oz`, lowercase, set in **JetBrains Mono Bold (weight 700)** — the
  typeface the app already ships
  (`assets/fonts/jetbrainsmono/JetBrainsMonoNerdFontMono-Bold.ttf`). Slight
  negative letter-spacing so the two glyphs read as one unit.
- Cursor: a cyan rounded rectangle to the right of the `z`, sized like a
  terminal block cursor sitting on the text baseline/x-height band.

## 3. Color tokens

| Role | Hex | Notes |
| --- | --- | --- |
| Background gradient start (top-left) | `#7c3aed` | violet |
| Background gradient end (bottom-right) | `#2563eb` | blue; linear at 135° |
| Glyphs `oz` | `#ffffff` | white |
| Block cursor | `#22d3ee` | cyan |

Finish is **flat**: the gradient only. No baked drop shadow, no top sheen,
no inner bevel, no cursor glow. (Depth and vivid/glow finishes were considered
and rejected in favor of the flat option.)

## 4. Geometry — macOS Big Sur grid

- Canvas: **1024×1024**, transparent background.
- Icon body: a **824×824** continuous-corner squircle (superellipse),
  centered, leaving a **100px** transparent margin on every side. This matches
  the Big Sur+ icon template so `ozmux` shares the common silhouette in the
  Dock.
- Corner: true continuous-corner squircle preferred. A plain rounded rect with
  `rx ≈ 185` (≈ 0.2237 × 824) is an acceptable v1 approximation if generating
  the exact superellipse path is inconvenient. The committed master resolves to
  exactly **one** corner treatment (a fixed superellipse path or the fixed
  rect) — there is no runtime branch in the pipeline.
- No baked shadow (macOS does not require one for the squircle; the flat finish
  omits it deliberately).

## 5. Master artwork

A single self-contained SVG is the source of truth:
`build/macos/icon/appicon.svg` (1024×1024).

Reference composition (coordinates approximate; tune for optical centering):

```svg
<svg viewBox="0 0 1024 1024" xmlns="http://www.w3.org/2000/svg">
  <defs>
    <linearGradient id="bg" x1="0" y1="0" x2="1" y2="1">
      <stop offset="0" stop-color="#7c3aed"/>
      <stop offset="1" stop-color="#2563eb"/>
    </linearGradient>
  </defs>
  <!-- 824x824 body, 100px margin; swap for a true squircle path if available -->
  <rect x="100" y="100" width="824" height="824" rx="185" fill="url(#bg)"/>
  <text x="452" y="512"
        font-family="JetBrainsMono Nerd Font Mono" font-weight="700" font-size="392"
        fill="#ffffff" text-anchor="middle" dominant-baseline="central"
        letter-spacing="-6">oz</text>
  <rect x="706" y="520" width="92" height="104" rx="12" fill="#22d3ee"/>
</svg>
```

Font handling: keep `oz` as a `<text>` element and resolve the glyphs from the
bundled JetBrains Mono Bold TTF at rasterization time (see §6). This reproduces
the exact shipped letterforms without requiring the font to be installed
system-wide. Outlining the glyphs to `<path>` data is an optional hardening
step (fully font-independent master) and is not required for v1.

Gotcha: the SVG `font-family` must match the bundled font's **internal family
name**, not the filename. Verified via `fc-scan` on the shipped TTF, that name
is `JetBrainsMono Nerd Font Mono` (alias `JetBrainsMono NFM`) — the value used
in the reference SVG above. Re-verify with `fc-scan` / `resvg --list-fonts` if
the bundled font is ever updated. Do not rely on resvg's `--font-family` to
correct a wrong literal: it only takes effect when the element has no
`font-family` at all. Outlining the glyphs to `<path>` sidesteps the
family-name dependency entirely.

## 6. Production pipeline

A new script `scripts/build_icon.py`, invoked by a `just icon` recipe (mirroring
the existing `bundle-macos` recipe), regenerates the icon from the master SVG:

1. Rasterize `build/macos/icon/appicon.svg` to PNGs at 16, 32, 64, 128, 256,
   512, and 1024 px using **resvg**, pinning the bundled font and disabling
   system-font fallback so output is deterministic on any machine:
   `resvg --skip-system-fonts --use-font-file assets/fonts/jetbrainsmono/JetBrainsMonoNerdFontMono-Bold.ttf <in.svg> <out.png>`
   (the directory-form equivalent is `--use-fonts-dir assets/fonts/jetbrainsmono`).
   The Rust resvg CLI (the binary `cargo install resvg` provides) spells these
   `--use-font-file` / `--use-fonts-dir` — NOT `--font-file` / `--font-dir`,
   which belong to the unrelated `@resvg/resvg-js-cli` npm package.
   `--skip-system-fonts` is mandatory: JetBrains Mono is not installed
   system-wide, and without it resvg silently falls back to a system mono,
   breaking §10's reproducibility criterion. (resvg's `--font-family` only
   applies when the SVG has no `font-family`, so the master SVG must itself
   carry the correct family name — see §5.) `rsvg-convert` (librsvg) is a
   fallback but pins a specific TTF only via fontconfig indirection, so prefer
   resvg.
2. Lay the PNGs out as a standard `AppIcon.iconset/` directory with Apple's
   required names:

   | File | px |
   | --- | --- |
   | `icon_16x16.png` | 16 |
   | `icon_16x16@2x.png` | 32 |
   | `icon_32x32.png` | 32 |
   | `icon_32x32@2x.png` | 64 |
   | `icon_128x128.png` | 128 |
   | `icon_128x128@2x.png` | 256 |
   | `icon_256x256.png` | 256 |
   | `icon_256x256@2x.png` | 512 |
   | `icon_512x512.png` | 512 |
   | `icon_512x512@2x.png` | 1024 |

3. Run `iconutil -c icns AppIcon.iconset -o build/macos/AppIcon.icns`.
4. Copy the 1024px render to a committed path for README/social reuse
   (`build/macos/AppIcon-1024.png`).

The generated `build/macos/AppIcon.icns` **and** `build/macos/AppIcon-1024.png`
are committed. Consequence: the release path (`bundle_macos.py` /
`release-macos`) needs **no new dependency** — it just consumes the committed
`.icns`. Only regenerating the icon needs `resvg` + `iconutil` (`iconutil`
ships with macOS; `resvg` via `cargo install resvg`, documented as a dev tool).

Note: `build/` is NOT gitignored (only `debug`/`target`/`dist`/`docs` are), so
committing `build/macos/AppIcon.icns` and the source SVG works as written.

Pipeline notes (for `scripts/build_icon.py`):

- Rasterize the **7 unique pixel sizes** (16, 32, 64, 128, 256, 512, 1024) once
  each, then copy the shared sizes into both iconset names that need them (e.g.
  the 32px render fills both `icon_16x16@2x.png` and `icon_32x32.png`).
- Treat `AppIcon.iconset/` and all intermediate PNGs as **temporary** scratch;
  only `appicon.svg`, `AppIcon.icns`, and `AppIcon-1024.png` are committed.
- **Fail fast** before calling `iconutil`: verify `resvg` and `iconutil` are on
  `PATH`, the bundled font file exists, and each rasterized PNG has the expected
  dimensions. Surface errors during regeneration, not during the release bundle.

## 7. Wiring into the bundle

No change to `scripts/bundle_macos.py` is required for the icon to take effect —
it already conditionally wires `AppIcon.icns`. Once the file exists at
`build/macos/AppIcon.icns`, the next `just bundle-macos` / `release-macos`
produces an `.app` whose `Info.plist` carries `CFBundleIconFile = AppIcon.icns`
and whose `Contents/Resources/AppIcon.icns` is the new artwork.

`build/macos/Info.plist` (the source plist) does not need a `CFBundleIconFile`
key added — the bundler injects it. (Adding it there is harmless but redundant;
leave it out to keep one source of truth.)

The generated CEF helper apps ("ozmux Helper.app", etc.) keep the default icon;
they are not user-facing and are out of scope.

## 8. Small-size legibility

The single master serves every size. The 16/32px previews during brainstorming
showed `oz` + cursor as borderline-but-readable. Implementation includes a
visual QA pass at 16 and 32px. If either is illegible, apply the lightest
sufficient fix (in order of preference) and document it:

1. Nudge glyph size / margin for small renders.
2. Provide an alternate small SVG (≤32px) that drops the cursor.
3. Use a heavier glyph treatment at small sizes.

This is a contingency, not planned work — ship the single master first.

## 9. Deliverables (approved scope)

In scope:

- `build/macos/icon/appicon.svg` — master artwork (source of truth).
- `scripts/build_icon.py` + `just icon` — regeneration pipeline.
- `build/macos/AppIcon.icns` — generated, committed (consumed by the bundler).
- `build/macos/AppIcon-1024.png` — 1024px export for README/social.

Out of scope (future, if needed):

- Favicon / website assets.
- Non-macOS targets (Linux `.png`, Windows `.ico`).
- Light/dark Dock variants or a `.icon` (Icon Composer) liquid-glass version.

## 10. Success criteria

- `just icon` regenerates `build/macos/AppIcon.icns` reproducibly from the
  master SVG with no manual steps.
- After `just bundle-macos`, `ozmux.app` shows the new icon in Finder and the
  Dock (not the generic placeholder), and `Info.plist` contains
  `CFBundleIconFile = AppIcon.icns`.
- The mark is recognizable at 16, 32, 128, 256, 512, and 1024px.
- Colors and geometry match §3–§4.
```
