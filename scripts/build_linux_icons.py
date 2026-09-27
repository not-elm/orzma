#!/usr/bin/env python3
"""Regenerate the orzma Linux hicolor icons (build/linux/icons/) from the master SVG."""

from __future__ import annotations

import argparse
import shutil
from pathlib import Path

from build_icon import DEFAULT_FONT, DEFAULT_SVG, png_dimensions, resvg_argv, run

REPO_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_OUT_DIR = REPO_ROOT / "build" / "linux" / "icons"
ICON_SIZES = (48, 128, 256, 512)


def icon_path(out_dir: Path, size: int) -> Path:
    return out_dir / f"orzma-{size}.png"


def render_argv(svg: Path, out_dir: Path, size: int, font: Path) -> list[str]:
    return resvg_argv(svg, icon_path(out_dir, size), size, font)


def build_linux_icons(svg: Path, out_dir: Path, font: Path) -> None:
    if shutil.which("resvg") is None:
        raise SystemExit("resvg not found on PATH (install it with `cargo install resvg`)")
    if not svg.is_file():
        raise SystemExit(f"master SVG not found: {svg}")
    if not font.is_file():
        raise SystemExit(f"icon font not found: {font}")
    out_dir.mkdir(parents=True, exist_ok=True)
    for size in ICON_SIZES:
        png = icon_path(out_dir, size)
        run(render_argv(svg, out_dir, size, font))
        dims = png_dimensions(png)
        if dims != (size, size):
            raise SystemExit(f"resvg produced {dims}, expected ({size}, {size}): {png}")
    print(f"==> wrote {len(ICON_SIZES)} icons to {out_dir}")


def main(argv: list[str] | None = None) -> None:
    p = argparse.ArgumentParser(description="Build the Linux hicolor icons from the master SVG")
    p.add_argument("--svg", default=str(DEFAULT_SVG))
    p.add_argument("--out-dir", default=str(DEFAULT_OUT_DIR))
    p.add_argument("--font", default=str(DEFAULT_FONT))
    args = p.parse_args(argv)
    build_linux_icons(Path(args.svg), Path(args.out_dir), Path(args.font))


if __name__ == "__main__":
    main()
