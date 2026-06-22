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
