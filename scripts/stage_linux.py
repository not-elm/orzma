#!/usr/bin/env python3
"""Stage orzma, its companions, and the CEF runtime into a tar.gz for Linux."""

from __future__ import annotations

import json
import re
import shutil
import subprocess
from pathlib import Path

from stage_windows import cargo_version, locked_version, sha256_file, verify_orzmd_web_assets

APP_NAME = "orzma"
BIN_NAME = "orzma"
ARCH = "x86_64"
TARGET_TRIPLE = "x86_64-unknown-linux-gnu"
CARGO_PROFILE = "dist"
COMPANION_BINS = ("orzbrowser", "orzmd")
RENDER_PROCESS_PACKAGE = "cef_render_process"
RENDER_PROCESS_BIN = "bevy_cef_render_process"
RPATH_BINS = (BIN_NAME, RENDER_PROCESS_BIN)
CEF_SYS_CRATE = "cef-dll-sys"
CEF_PLATFORM_DIR = "cef_linux_x86_64"

REPO_ROOT = Path(__file__).resolve().parent.parent

CEF_EXCLUDED_ENTRIES = frozenset({
    "include",
    "libcef_dll",
    "cmake",
    "CMakeLists.txt",
    "archive.json",
    "CREDITS.html",
    "chrome-sandbox",
})
CEF_REQUIRED_ENTRIES = (
    "libcef.so",
    "icudtl.dat",
    "resources.pak",
    "v8_context_snapshot.bin",
    "locales",
)
ARCHIVE_NAME_RE = re.compile(r"^cef_binary_([^+]+)\+.*_linux64_minimal\.tar\.bz2$")
DYNAMIC_PATH_RE = re.compile(r"\((?:RUNPATH|RPATH)\)\s+Library (?:runpath|rpath): \[(.*)\]")


def cef_build_meta(locked: str | None) -> str:
    """The CEF build metadata (the part after `+`) of the locked cef-dll-sys version."""
    if locked is None:
        raise SystemExit(f"{CEF_SYS_CRATE} is not in Cargo.lock; cannot locate the CEF runtime")
    _, sep, meta = locked.partition("+")
    if not sep or not meta:
        raise SystemExit(
            f"{CEF_SYS_CRATE} {locked} carries no +<cef version> build metadata; "
            "cannot locate the CEF runtime it downloads"
        )
    return meta


def cef_runtime_dir(cef_path: Path, build_meta: str) -> Path:
    return cef_path / build_meta / CEF_PLATFORM_DIR


def archive_build_meta(archive_json: str) -> str | None:
    """The CEF version a Linux archive.json names, or None when it names no Linux archive."""
    try:
        data = json.loads(archive_json)
    except ValueError:
        return None
    name = data.get("name") if isinstance(data, dict) else None
    if not isinstance(name, str):
        return None
    match = ARCHIVE_NAME_RE.match(name)
    return match.group(1) if match else None


def verify_cef_archive(cef_dir: Path, build_meta: str) -> None:
    path = cef_dir / "archive.json"
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as error:
        raise SystemExit(f"cannot read {path}: {error}") from error
    found = archive_build_meta(text)
    if found != build_meta:
        raise SystemExit(
            f"{path} describes CEF {found or 'of an unknown platform'}, but orzma links "
            f"CEF {build_meta}; delete {cef_dir} and rebuild so cef-dll-sys downloads it again"
        )


def select_cef_entries(cef_dir: Path) -> list[Path]:
    return sorted(
        (entry for entry in cef_dir.iterdir() if entry.name not in CEF_EXCLUDED_ENTRIES),
        key=lambda entry: entry.name,
    )


def copy_cef_runtime(cef_dir: Path, tree: Path) -> list[str]:
    entries = select_cef_entries(cef_dir)
    names = [entry.name for entry in entries]
    missing = [name for name in CEF_REQUIRED_ENTRIES if name not in names]
    if missing:
        raise SystemExit(f"missing required CEF entries in {cef_dir}: {', '.join(missing)}")
    for entry in entries:
        dest = tree / entry.name
        if entry.is_dir():
            shutil.copytree(entry, dest)
        else:
            shutil.copy2(entry, dest)
    return names


def stage_cef(cef_path: Path, tree: Path) -> None:
    build_meta = cef_build_meta(locked_version(CEF_SYS_CRATE))
    cef_dir = cef_runtime_dir(cef_path, build_meta)
    if not cef_dir.is_dir():
        raise SystemExit(
            f"CEF runtime not found at {cef_dir}; build orzma with CEF_PATH={cef_path} "
            "first (omit --skip-build) so cef-dll-sys downloads it"
        )
    verify_cef_archive(cef_dir, build_meta)
    staged = copy_cef_runtime(cef_dir, tree)
    print(f"==> staged {len(staged)} CEF entries from {cef_dir}")


def runpath_entries(readelf_output: str) -> list[str]:
    entries: list[str] = []
    for match in DYNAMIC_PATH_RE.finditer(readelf_output):
        entries += match.group(1).split(":")
    return entries


def unresolved_libraries(ldd_output: str) -> list[str]:
    return sorted({
        line.split("=>", 1)[0].strip()
        for line in ldd_output.splitlines()
        if "=> not found" in line
    })


def verify_runpaths(tree: Path) -> None:
    missing = [
        name for name in RPATH_BINS
        if "$ORIGIN" not in runpath_entries(_capture(["readelf", "-d", str(tree / name)]))
    ]
    if missing:
        raise SystemExit(
            f"no $ORIGIN RUNPATH on {', '.join(missing)}; they would not find the "
            "libcef.so staged beside them (check the rustc-link-arg-bins in build.rs)"
        )
    print(f"==> RUNPATH check passed ({', '.join(RPATH_BINS)})")


def verify_deps(tree: Path) -> None:
    unresolved = {}
    for name in ("libcef.so", *RPATH_BINS):
        found = unresolved_libraries(_capture(["ldd", str(tree / name)]))
        if found:
            unresolved[name] = found
    if unresolved:
        raise SystemExit(f"unresolved shared libraries: {unresolved}")
    print("==> dependency check passed (ldd reports nothing missing)")


def _capture(argv: list[str]) -> str:
    return subprocess.run(argv, capture_output=True, text=True, check=True).stdout
