#!/usr/bin/env python3
"""Regenerate the orzma Windows icon (build/windows/orzma.ico) from the master SVG."""

from __future__ import annotations

import argparse
import shutil
import struct
import tempfile
from pathlib import Path

from build_icon import DEFAULT_FONT, DEFAULT_SVG, png_dimensions, resvg_argv, run

REPO_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_ICO = REPO_ROOT / "build" / "windows" / "orzma.ico"
ICO_SIZES = (16, 32, 48, 256)


def ico_bytes(images: list[tuple[int, bytes]]) -> bytes:
    header = struct.pack("<HHH", 0, 1, len(images))
    offset = 6 + 16 * len(images)
    entries, blobs = b"", b""
    for size, data in images:
        entries += struct.pack(
            "<BBBBHHII", size % 256, size % 256, 0, 0, 1, 32, len(data), offset
        )
        blobs += data
        offset += len(data)
    return header + entries + blobs


def build_ico(svg: Path, out: Path, font: Path) -> None:
    # NOTE: build_icon.verify_prerequisites also requires iconutil, which is macOS-only,
    # so the prerequisites this script actually needs are checked here instead.
    if shutil.which("resvg") is None:
        raise SystemExit("resvg not found on PATH (install it with `cargo install resvg`)")
    if not svg.is_file():
        raise SystemExit(f"master SVG not found: {svg}")
    if not font.is_file():
        raise SystemExit(f"icon font not found: {font}")
    images = []
    with tempfile.TemporaryDirectory() as tmp:
        for size in ICO_SIZES:
            png = Path(tmp) / f"icon_{size}.png"
            run(resvg_argv(svg, png, size, font))
            dims = png_dimensions(png)
            if dims != (size, size):
                raise SystemExit(f"resvg produced {dims}, expected ({size}, {size}): {png}")
            images.append((size, png.read_bytes()))
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_bytes(ico_bytes(images))
    print(f"==> wrote {out} ({len(images)} images)")


def main(argv: list[str] | None = None) -> None:
    p = argparse.ArgumentParser(description="Build the Windows .ico from the master SVG")
    p.add_argument("--svg", default=str(DEFAULT_SVG))
    p.add_argument("--out", default=str(DEFAULT_ICO))
    p.add_argument("--font", default=str(DEFAULT_FONT))
    args = p.parse_args(argv)
    build_ico(Path(args.svg), Path(args.out), Path(args.font))


if __name__ == "__main__":
    main()
