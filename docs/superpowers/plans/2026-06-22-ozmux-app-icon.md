# ozmux App Icon Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a real macOS app icon for `ozmux` — a `oz`+cursor monogram — generated reproducibly from a master SVG into `build/macos/AppIcon.icns`, which the existing bundler already wires into `ozmux.app`.

**Architecture:** A self-contained master SVG (`build/macos/icon/appicon.svg`) is rasterized by a new Python script (`scripts/build_icon.py`, mirroring `scripts/bundle_macos.py`'s pure-`*_argv`-helpers style) using `resvg` (pinned to the bundled JetBrains Mono TTF) into the 7 unique iconset sizes, packed by `iconutil` into `AppIcon.icns`. A `just icon` recipe drives it. The generated `.icns` and a 1024px PNG are committed, so the release path gains no new dependency.

**Tech Stack:** Python 3 (stdlib only), `resvg` (Rust CLI, dev-only), `iconutil` (ships with macOS), `just`, SVG.

Spec: `docs/superpowers/specs/2026-06-22-ozmux-app-icon-design.md` (read it).

## Global Constraints

- Platform: macOS only (Apple Silicon). `LSMinimumSystemVersion` = `11.0`. Do not add non-macOS handling.
- Canvas `1024×1024`, transparent. Icon body `824×824`, centered, `100px` transparent margin. Exactly ONE corner treatment in the committed master — v1 uses a rounded rect `rx="185"` (spec §4 sanctions this; a true superellipse path is a future upgrade, no runtime branch).
- Colors (verbatim): background linear gradient `#7c3aed` (top-left) → `#2563eb` (bottom-right, 135°); glyphs `#ffffff`; block cursor `#22d3ee`. Finish is **flat** — no shadow, sheen, bevel, or glow.
- Font: JetBrains Mono Bold (`font-weight="700"`). SVG `font-family` MUST be the internal family name `JetBrainsMono Nerd Font Mono` (verified via `fc-scan`), NOT `JetBrains Mono`. File: `assets/fonts/jetbrainsmono/JetBrainsMonoNerdFontMono-Bold.ttf`.
- Rasterizer flags (verbatim): `resvg --skip-system-fonts --use-font-file <ttf> --width <N> --height <N> <in.svg> <out.png>`. The flag is `--use-font-file` (Rust resvg CLI from `cargo install resvg`), NOT `--font-file` (that belongs to the unrelated `@resvg/resvg-js-cli` npm package). `--skip-system-fonts` is mandatory (JetBrains Mono is not installed system-wide; without it resvg silently falls back to a system mono → non-reproducible output).
- Render the **7 unique sizes** (16, 32, 64, 128, 256, 512, 1024) once each, then copy the shared renders into the 10 iconset filenames. Treat `AppIcon.iconset/` and intermediate PNGs as temporary scratch.
- Committed outputs: `build/macos/icon/appicon.svg`, `build/macos/AppIcon.icns`, `build/macos/AppIcon-1024.png`. (`build/` is NOT gitignored.)
- Do NOT modify `scripts/bundle_macos.py` or `build/macos/Info.plist` — the bundler already injects `CFBundleIconFile` and copies the icns when `build/macos/AppIcon.icns` exists (`scripts/bundle_macos.py:217-229`).
- All in-code comments in English. Python: stdlib only; mirror `bundle_macos.py` (pure `*_argv` helpers returning `list[str]`, an orchestrating `main()`).
- Commit message trailer (every commit): two `-m` trailers —
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>` and
  `Claude-Session: https://claude.ai/code/session_01VCLgegxqr1voAD4m39srXN`.

---

### Task 1: Master SVG artwork

**Files:**
- Create: `build/macos/icon/appicon.svg`
- Create: `scripts/test_icon_svg.py`

**Interfaces:**
- Consumes: nothing.
- Produces: `build/macos/icon/appicon.svg` — the single source-of-truth master consumed by `scripts/build_icon.py` (Task 2/3).

- [ ] **Step 1: Write the failing test**

Create `scripts/test_icon_svg.py`:

```python
"""Structure checks for the ozmux app-icon master SVG."""

import unittest
import xml.etree.ElementTree as ET
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
SVG = REPO_ROOT / "build" / "macos" / "icon" / "appicon.svg"


class TestMasterSvg(unittest.TestCase):
    def setUp(self):
        self.assertTrue(SVG.is_file(), f"missing master SVG: {SVG}")
        self.text = SVG.read_text(encoding="utf-8")

    def test_well_formed_xml(self):
        ET.fromstring(self.text)

    def test_canvas_is_1024(self):
        self.assertIn('viewBox="0 0 1024 1024"', self.text)

    def test_gradient_colors(self):
        self.assertIn('stop-color="#7c3aed"', self.text)
        self.assertIn('stop-color="#2563eb"', self.text)

    def test_glyphs_white_oz(self):
        self.assertIn('fill="#ffffff"', self.text)
        self.assertIn(">oz<", self.text)

    def test_cursor_cyan(self):
        self.assertIn('fill="#22d3ee"', self.text)

    def test_font_family_is_internal_name(self):
        self.assertIn('font-family="JetBrainsMono Nerd Font Mono"', self.text)
        self.assertIn('font-weight="700"', self.text)


if __name__ == "__main__":
    unittest.main()
```

- [ ] **Step 2: Run test to verify it fails**

Run: `python3 -m unittest scripts.test_icon_svg -v`
Expected: FAIL — `missing master SVG: .../build/macos/icon/appicon.svg` (file not created yet).

- [ ] **Step 3: Create the master SVG**

Create `build/macos/icon/appicon.svg`:

```svg
<svg viewBox="0 0 1024 1024" xmlns="http://www.w3.org/2000/svg">
  <defs>
    <linearGradient id="bg" x1="0" y1="0" x2="1" y2="1">
      <stop offset="0" stop-color="#7c3aed"/>
      <stop offset="1" stop-color="#2563eb"/>
    </linearGradient>
  </defs>
  <rect x="100" y="100" width="824" height="824" rx="185" fill="url(#bg)"/>
  <text x="452" y="512"
        font-family="JetBrainsMono Nerd Font Mono" font-weight="700" font-size="392"
        fill="#ffffff" text-anchor="middle" dominant-baseline="central"
        letter-spacing="-6">oz</text>
  <rect x="706" y="520" width="92" height="104" rx="12" fill="#22d3ee"/>
</svg>
```

- [ ] **Step 4: Run test to verify it passes**

Run: `python3 -m unittest scripts.test_icon_svg -v`
Expected: PASS (6 tests OK).

- [ ] **Step 5: Commit**

```bash
git add build/macos/icon/appicon.svg scripts/test_icon_svg.py
git add -f docs/superpowers/specs/2026-06-22-ozmux-app-icon-design.md \
          docs/superpowers/plans/2026-06-22-ozmux-app-icon.md
git commit \
  -m "feat(icon): add master SVG for the ozmux app icon" \
  -m "Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>" \
  -m "Claude-Session: https://claude.ai/code/session_01VCLgegxqr1voAD4m39srXN"
```

(`docs/` is gitignored but the repo tracks specs/plans — `-f` matches the 42 existing tracked specs.)

---

### Task 2: Icon build script (`scripts/build_icon.py`) + unit tests

**Files:**
- Create: `scripts/build_icon.py`
- Create: `scripts/test_build_icon.py`

**Interfaces:**
- Consumes: `build/macos/icon/appicon.svg` (Task 1); the bundled font TTF.
- Produces (the public surface later tasks/`just icon` rely on):
  - `ICONSET_ENTRIES: list[tuple[str, int]]` and `iconset_entries() -> list[tuple[str, int]]` — the 10 Apple iconset (filename, px) pairs.
  - `unique_sizes(entries: list[tuple[str, int]]) -> list[int]` — sorted distinct sizes.
  - `resvg_argv(svg: Path, png: Path, size: int, font: Path) -> list[str]`.
  - `iconutil_argv(iconset: Path, out: Path) -> list[str]`.
  - `parse_png_dimensions(header: bytes) -> tuple[int, int]` and `png_dimensions(path: Path) -> tuple[int, int]`.
  - `build_icon(svg: Path, out: Path, png_1024: Path, font: Path) -> None` and `main(argv: list[str] | None = None) -> None`.
  - Module constants `DEFAULT_SVG`, `DEFAULT_ICNS`, `DEFAULT_PNG_1024`, `DEFAULT_FONT` (all `Path`).

- [ ] **Step 1: Write the failing tests**

Create `scripts/test_build_icon.py`:

```python
"""Unit tests for the pure helpers in scripts/build_icon.py."""

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import build_icon as bi


class TestIconsetPlan(unittest.TestCase):
    def test_entries_are_apples_ten(self):
        self.assertEqual(
            bi.iconset_entries(),
            [
                ("icon_16x16.png", 16),
                ("icon_16x16@2x.png", 32),
                ("icon_32x32.png", 32),
                ("icon_32x32@2x.png", 64),
                ("icon_128x128.png", 128),
                ("icon_128x128@2x.png", 256),
                ("icon_256x256.png", 256),
                ("icon_256x256@2x.png", 512),
                ("icon_512x512.png", 512),
                ("icon_512x512@2x.png", 1024),
            ],
        )

    def test_unique_sizes_are_seven(self):
        self.assertEqual(
            bi.unique_sizes(bi.iconset_entries()),
            [16, 32, 64, 128, 256, 512, 1024],
        )


class TestArgv(unittest.TestCase):
    def test_resvg_argv_pins_font_and_skips_system(self):
        argv = bi.resvg_argv(Path("/a/in.svg"), Path("/b/out.png"), 128, Path("/f/Bold.ttf"))
        self.assertEqual(
            argv,
            [
                "resvg",
                "--skip-system-fonts",
                "--use-font-file", "/f/Bold.ttf",
                "--width", "128",
                "--height", "128",
                "/a/in.svg",
                "/b/out.png",
            ],
        )

    def test_iconutil_argv(self):
        argv = bi.iconutil_argv(Path("/t/AppIcon.iconset"), Path("/o/AppIcon.icns"))
        self.assertEqual(
            argv,
            ["iconutil", "-c", "icns", "/t/AppIcon.iconset", "-o", "/o/AppIcon.icns"],
        )


class TestPngDimensions(unittest.TestCase):
    def _png_header(self, width: int, height: int) -> bytes:
        return (
            b"\x89PNG\r\n\x1a\n"
            + b"\x00\x00\x00\x0dIHDR"
            + width.to_bytes(4, "big")
            + height.to_bytes(4, "big")
        )

    def test_parse_valid_header(self):
        self.assertEqual(bi.parse_png_dimensions(self._png_header(512, 512)), (512, 512))

    def test_parse_rejects_non_png(self):
        with self.assertRaises(ValueError):
            bi.parse_png_dimensions(b"not-a-png" + b"\x00" * 16)


if __name__ == "__main__":
    unittest.main()
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `python3 -m unittest scripts.test_build_icon -v`
Expected: FAIL — `ModuleNotFoundError: No module named 'build_icon'` (script not created yet).

- [ ] **Step 3: Write the implementation**

Create `scripts/build_icon.py`:

```python
#!/usr/bin/env python3
"""Regenerate the ozmux macOS app icon (AppIcon.icns) from the master SVG."""

from __future__ import annotations

import argparse
import shutil
import subprocess
import tempfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_SVG = REPO_ROOT / "build" / "macos" / "icon" / "appicon.svg"
DEFAULT_ICNS = REPO_ROOT / "build" / "macos" / "AppIcon.icns"
DEFAULT_PNG_1024 = REPO_ROOT / "build" / "macos" / "AppIcon-1024.png"
DEFAULT_FONT = (
    REPO_ROOT / "assets" / "fonts" / "jetbrainsmono" / "JetBrainsMonoNerdFontMono-Bold.ttf"
)

PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"

ICONSET_ENTRIES: list[tuple[str, int]] = [
    ("icon_16x16.png", 16),
    ("icon_16x16@2x.png", 32),
    ("icon_32x32.png", 32),
    ("icon_32x32@2x.png", 64),
    ("icon_128x128.png", 128),
    ("icon_128x128@2x.png", 256),
    ("icon_256x256.png", 256),
    ("icon_256x256@2x.png", 512),
    ("icon_512x512.png", 512),
    ("icon_512x512@2x.png", 1024),
]


def iconset_entries() -> list[tuple[str, int]]:
    return list(ICONSET_ENTRIES)


def unique_sizes(entries: list[tuple[str, int]]) -> list[int]:
    return sorted({size for _, size in entries})


def resvg_argv(svg: Path, png: Path, size: int, font: Path) -> list[str]:
    return [
        "resvg",
        "--skip-system-fonts",
        "--use-font-file", str(font),
        "--width", str(size),
        "--height", str(size),
        str(svg),
        str(png),
    ]


def iconutil_argv(iconset: Path, out: Path) -> list[str]:
    return ["iconutil", "-c", "icns", str(iconset), "-o", str(out)]


def parse_png_dimensions(header: bytes) -> tuple[int, int]:
    if header[:8] != PNG_SIGNATURE:
        raise ValueError("not a PNG file")
    width = int.from_bytes(header[16:20], "big")
    height = int.from_bytes(header[20:24], "big")
    return width, height


def png_dimensions(path: Path) -> tuple[int, int]:
    with open(path, "rb") as f:
        return parse_png_dimensions(f.read(24))


def run(argv: list[str]) -> None:
    print("==> " + " ".join(argv))
    subprocess.run(argv, check=True)


def verify_prerequisites(svg: Path, font: Path) -> None:
    for tool in ("resvg", "iconutil"):
        if shutil.which(tool) is None:
            raise SystemExit(
                f"required tool not found on PATH: {tool} "
                "(install resvg with `cargo install resvg`)"
            )
    if not svg.is_file():
        raise SystemExit(f"master SVG not found: {svg}")
    if not font.is_file():
        raise SystemExit(f"bundled font not found: {font}")


def build_icon(svg: Path, out: Path, png_1024: Path, font: Path) -> None:
    verify_prerequisites(svg, font)
    entries = iconset_entries()
    with tempfile.TemporaryDirectory() as tmp_name:
        tmp = Path(tmp_name)
        renders: dict[int, Path] = {}
        for size in unique_sizes(entries):
            png = tmp / f"render_{size}.png"
            run(resvg_argv(svg, png, size, font))
            dims = png_dimensions(png)
            if dims != (size, size):
                raise SystemExit(f"resvg produced {dims}, expected ({size}, {size}): {png}")
            renders[size] = png
        iconset = tmp / "AppIcon.iconset"
        iconset.mkdir()
        for name, size in entries:
            shutil.copy2(renders[size], iconset / name)
        out.parent.mkdir(parents=True, exist_ok=True)
        run(iconutil_argv(iconset, out))
        shutil.copy2(renders[1024], png_1024)
    print(f"wrote {out}")
    print(f"wrote {png_1024}")


def build_arg_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(description="Regenerate the ozmux macOS app icon")
    p.add_argument("--svg", default=str(DEFAULT_SVG))
    p.add_argument("--out", default=str(DEFAULT_ICNS))
    p.add_argument("--png-1024", default=str(DEFAULT_PNG_1024))
    p.add_argument("--font", default=str(DEFAULT_FONT))
    return p


def main(argv: list[str] | None = None) -> None:
    args = build_arg_parser().parse_args(argv)
    build_icon(Path(args.svg), Path(args.out), Path(args.png_1024), Path(args.font))


if __name__ == "__main__":
    main()
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `python3 -m unittest scripts.test_build_icon -v`
Expected: PASS (6 tests OK).

- [ ] **Step 5: Commit**

```bash
git add scripts/build_icon.py scripts/test_build_icon.py
git commit \
  -m "feat(icon): add build_icon.py icon-generation pipeline with unit tests" \
  -m "Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>" \
  -m "Claude-Session: https://claude.ai/code/session_01VCLgegxqr1voAD4m39srXN"
```

---

### Task 3: `just icon` recipe + generate & commit the icon artifacts

**Files:**
- Modify: `justfile` (add an `icon` recipe near the `bundle-macos` recipe, lines ~69-77)
- Create (generated, committed): `build/macos/AppIcon.icns`, `build/macos/AppIcon-1024.png`

**Interfaces:**
- Consumes: `scripts/build_icon.py` (Task 2), `build/macos/icon/appicon.svg` (Task 1), `resvg`.
- Produces: `build/macos/AppIcon.icns` (consumed unchanged by `scripts/bundle_macos.py:217-229`) and `build/macos/AppIcon-1024.png`.

- [ ] **Step 1: Install resvg and confirm its flags**

```bash
cargo install resvg
```
Expected: installs the `resvg` binary to `~/.cargo/bin` (this compiles from source — allow a few minutes).

Confirm the flags this plan depends on actually exist:
```bash
resvg --help | grep -E -- '--use-font-file|--skip-system-fonts|--width|--height'
```
Expected: all four flags listed. If `--width`/`--height` are spelled differently in the installed version, update both `resvg_argv` in `scripts/build_icon.py` AND `test_resvg_argv_pins_font_and_skips_system` in `scripts/test_build_icon.py` to match, then re-run `python3 -m unittest scripts.test_build_icon`.

- [ ] **Step 2: Add the `just icon` recipe**

In `justfile`, immediately before the `# build and package the ozmux .app` comment (the `bundle-macos` recipe at ~line 69), insert:

```just
# regenerate the macOS app icon (build/macos/AppIcon.icns) from the master SVG
[macos]
icon *args:
    python3 scripts/build_icon.py {{ args }}

```

- [ ] **Step 3: Generate the icon**

Run: `just icon`
Expected: prints seven `==> resvg ...` lines, one `==> iconutil -c icns ...`, then `wrote .../AppIcon.icns` and `wrote .../AppIcon-1024.png`.

- [ ] **Step 4: Verify the generated artifacts**

```bash
file build/macos/AppIcon.icns
sips -g pixelWidth -g pixelHeight build/macos/AppIcon-1024.png
iconutil -c iconset build/macos/AppIcon.icns -o /tmp/verify.iconset && ls /tmp/verify.iconset | wc -l
rm -rf /tmp/verify.iconset
```
Expected: `file` reports a `Mac OS X icon` (icns); `sips` reports `pixelWidth: 1024` and `pixelHeight: 1024`; the round-trip iconset lists `10` files. Confirm the iconset/intermediate PNGs were NOT left in the repo: `git status --short build/macos/` shows only `AppIcon.icns` and `AppIcon-1024.png` as new (plus the already-committed `appicon.svg`).

- [ ] **Step 5: Commit**

```bash
git add justfile build/macos/AppIcon.icns build/macos/AppIcon-1024.png
git commit \
  -m "feat(icon): generate AppIcon.icns + 1024px export via just icon" \
  -m "Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>" \
  -m "Claude-Session: https://claude.ai/code/session_01VCLgegxqr1voAD4m39srXN"
```

---

### Task 4: Bundle-wiring verification + small-size visual QA

**Files:**
- Create: `scripts/test_icon_bundle.py`

**Interfaces:**
- Consumes: `scripts/bundle_macos.py` (`build_arg_parser`, `resolve_config`, `assemble_app`, `REPO_ROOT`); the committed `build/macos/AppIcon.icns` (Task 3).
- Produces: an automated proof that `assemble_app` injects `CFBundleIconFile` and copies the icns, satisfying spec §10 without a full release build.

- [ ] **Step 1: Write the failing test**

Create `scripts/test_icon_bundle.py`:

```python
"""Verifies the bundler wires the committed AppIcon.icns into the .app."""

import plistlib
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import bundle_macos as bm

ICNS = bm.REPO_ROOT / "build" / "macos" / "AppIcon.icns"


class TestBundleIconWiring(unittest.TestCase):
    def test_committed_icns_exists(self):
        self.assertTrue(ICNS.is_file(), f"missing committed icns: {ICNS}")

    def test_assemble_app_sets_icon_file_and_copies_icns(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            fake_bin = root / "ozmux"
            fake_bin.write_bytes(b"\x7fELF fake binary")
            args = bm.build_arg_parser().parse_args(
                [
                    "--version", "9.9.9",
                    "--bin", str(fake_bin),
                    "--no-sign",
                    "--out-dir", str(root / "out"),
                ]
            )
            cfg = bm.resolve_config(args)
            bm.assemble_app(cfg)
            info = plistlib.loads((cfg.app_path / "Contents" / "Info.plist").read_bytes())
            self.assertEqual(info.get("CFBundleIconFile"), "AppIcon.icns")
            self.assertTrue(
                (cfg.app_path / "Contents" / "Resources" / "AppIcon.icns").is_file()
            )


if __name__ == "__main__":
    unittest.main()
```

- [ ] **Step 2: Run the full icon suite to verify wiring passes**

Run: `python3 -m unittest scripts.test_icon_svg scripts.test_build_icon scripts.test_icon_bundle -v`
Expected: PASS. (`test_committed_icns_exists` and the wiring test pass because Task 3 committed the icns. If the icns is missing, run `just icon` first.)

- [ ] **Step 3: Confirm no pre-existing tests regressed**

Run: `python3 -m unittest discover -s scripts -p 'test_*.py'`
Expected: OK — the 30 `test_bundle_macos` tests plus the new icon tests all pass.

- [ ] **Step 4: Small-size visual QA**

Render preview PNGs to a temp dir and visually inspect them (open/Read the images):
```bash
F=assets/fonts/jetbrainsmono/JetBrainsMonoNerdFontMono-Bold.ttf
for s in 16 32 64; do
  resvg --skip-system-fonts --use-font-file "$F" --width $s --height $s \
    build/macos/icon/appicon.svg /tmp/oz_$s.png
done
```
Then inspect `/tmp/oz_16.png`, `/tmp/oz_32.png`, `/tmp/oz_64.png`, and the committed `build/macos/AppIcon-1024.png`. Confirm: gradient renders, `oz` is white and legible, the cyan cursor is visible, nothing is clipped by the 100px margin. If 16/32px is illegible, apply spec §8's lightest contingency (nudge glyph size/margin in `appicon.svg`, re-run `just icon`, re-commit) and note what changed. `rm -f /tmp/oz_*.png` when done.

- [ ] **Step 5: Commit**

```bash
git add scripts/test_icon_bundle.py
git commit \
  -m "test(icon): verify bundler wires AppIcon.icns into the .app" \
  -m "Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>" \
  -m "Claude-Session: https://claude.ai/code/session_01VCLgegxqr1voAD4m39srXN"
```

---

## Manual smoke (out of automated scope)

A full `just bundle-macos` builds the entire Bevy+CEF release binary (multi-minute, needs `just setup-cef-release`) and is the spec §10 end-to-end check ("`ozmux.app` shows the new icon in Finder/Dock"). The wiring is already proven by Task 4; run the full bundle manually when convenient and confirm the Dock icon visually.

## Self-Review (completed by plan author)

- **Spec coverage:** §2/§3/§4 mark+colors+geometry → Task 1 SVG (+asset test). §5 master artwork + font-family gotcha → Task 1 (test asserts internal family name). §6 production pipeline (resvg flags, 7 unique renders, iconutil, 1024 export, fail-fast) → Task 2 (helpers/tests) + Task 3 (recipe/generate). §7 bundle wiring (no bundler edit) → Task 4 wiring test. §8 small-size legibility → Task 4 visual QA. §9 deliverables → Tasks 1+3 committed artifacts; intermediates kept temporary (Task 3 Step 4 check). §10 success criteria → Task 3 Step 4 (reproducible generate + dims) and Task 4 (wiring + recognizability). No gaps.
- **Placeholder scan:** none — every step has concrete code/commands and expected output.
- **Type consistency:** `iconset_entries()`/`unique_sizes()`/`resvg_argv()`/`iconutil_argv()`/`parse_png_dimensions()`/`build_icon()`/`main()` names and signatures match between Task 2 implementation, Task 2 tests, and the Interfaces blocks. `assemble_app`/`resolve_config`/`build_arg_parser`/`REPO_ROOT` match `bundle_macos.py` as read.
