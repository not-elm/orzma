#!/usr/bin/env python3
"""Stage orzma, its companions, and the CEF runtime into a flat tree for the Windows MSI."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path

APP_NAME = "orzma"
BIN_NAME = "orzma"
ARCH = "x64"
TARGET_TRIPLE = "x86_64-pc-windows-msvc"
CARGO_PROFILE = "dist"
COMPANION_BINS = ("orzbrowser", "orzmd")
RENDER_PROCESS_BIN = "bevy_cef_render_process"
RENDER_PROCESS_VERSION = "0.13.0"

REPO_ROOT = Path(__file__).resolve().parent.parent
INVENTORY_PATH = REPO_ROOT / "build" / "windows" / "cef-inventory.json"


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(65536), b""):
            digest.update(chunk)
    return digest.hexdigest()


def iter_cef_files(cef_dir: Path, build_only: set[str]) -> list[str]:
    entries = []
    for path in sorted(cef_dir.rglob("*")):
        if not path.is_file():
            continue
        rel = path.relative_to(cef_dir).as_posix()
        if rel.split("/")[0] in build_only:
            continue
        entries.append(rel)
    return sorted(entries)


def classify_entries(
    entries: list[str], required: dict[str, str], optional: dict[str, str]
) -> tuple[list[str], list[str], list[str]]:
    present = set(entries)
    known = set(required) | set(optional)
    staged = [rel for rel in entries if rel in known]
    missing = sorted(set(required) - present)
    unclassified = sorted(present - known)
    return staged, missing, unclassified


def digest_mismatches(cef_dir: Path, staged: list[str], digests: dict[str, str]) -> list[str]:
    return [
        rel for rel in staged
        if digests.get(rel) and sha256_file(cef_dir / rel) != digests[rel]
    ]


def load_inventory(path: Path) -> dict:
    with open(path, encoding="utf-8") as f:
        return json.load(f)


def build_inventory(
    cef_dir: Path, cef_version: str, build_only: set[str], optional_names: set[str]
) -> dict:
    required: dict[str, str] = {}
    # NOTE: seed every optional name, present on this host or not. An entry that CEF
    # ships only on some hosts would otherwise be missing from the inventory, and would
    # then count as unclassified -- a hard staging failure -- wherever it does appear.
    optional: dict[str, str | None] = {name: None for name in optional_names}
    for rel in iter_cef_files(cef_dir, build_only):
        if rel in optional_names:
            optional[rel] = sha256_file(cef_dir / rel)
        else:
            required[rel] = sha256_file(cef_dir / rel)
    return {
        "cef_version": cef_version,
        "build_only": sorted(build_only),
        "required": required,
        "optional": optional,
    }


BUILD_ONLY_ENTRIES = {
    "include",
    "libcef_dll",
    "cmake",
    "CMakeLists.txt",
    "libcef.lib",
    "archive.json",
    "bootstrap.exe",
    "bootstrapc.exe",
    "bevy_cef_render_process.exe",
}

OPTIONAL_ENTRIES = {
    "vk_swiftshader.dll",
    "vk_swiftshader_icd.json",
    "vulkan-1.dll",
    "dxil.dll",
    "dxcompiler.dll",
}


def refresh_inventory(cef_dir: Path, cef_version: str, out_path: Path) -> None:
    inventory = build_inventory(cef_dir, cef_version, BUILD_ONLY_ENTRIES, OPTIONAL_ENTRIES)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    with open(out_path, "w", encoding="utf-8", newline="\n") as f:
        json.dump(inventory, f, indent=2, sort_keys=True)
        f.write("\n")
    print(f"==> wrote {out_path} ({len(inventory['required'])} required files)")


if __name__ == "__main__":
    import argparse

    # NOTE: this option surface is the one Task 4 keeps. Only the body is replaced there,
    # so the just recipe below does not need rewriting.
    parser = argparse.ArgumentParser(description="Stage orzma for the Windows MSI")
    parser.add_argument("--refresh-inventory", action="store_true")
    parser.add_argument("--cef-dir", default="~/.local/share/cef")
    parser.add_argument("--cef-version")
    args = parser.parse_args()
    if not args.refresh_inventory:
        raise SystemExit("staging is implemented in a later task; pass --refresh-inventory")
    if not args.cef_version:
        raise SystemExit("--refresh-inventory requires --cef-version")
    refresh_inventory(Path(args.cef_dir).expanduser(), args.cef_version, INVENTORY_PATH)
