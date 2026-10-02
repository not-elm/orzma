#!/usr/bin/env python3
"""Stage orzma, its companions, and the CEF runtime into a tar.gz for Linux."""

from __future__ import annotations

import argparse
import gzip
import json
import os
import re
import shutil
import subprocess
import tarfile
from pathlib import Path

from build_linux_icons import ICON_SIZES, icon_path
from stage_windows import (
    cargo_version,
    is_cef_locale_pak,
    locked_version,
    missing_cef_locales,
    verify_orzbrowser_web_assets,
    verify_orzmd_web_assets,
)

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
LINUX_DIR = REPO_ROOT / "build" / "linux"
DEFAULT_OUT_DIR = REPO_ROOT / "target" / "dist"

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

# Chromium on Linux ignores `--lang`, picks the locale from LANGUAGE / LC_ALL /
# LC_MESSAGES / LANG, and falls back to en-US, so ship the fallback and the Japanese pack.
CEF_LOCALES = ("en-US", "ja")

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
    locales_dir = cef_dir / "locales"
    absent = missing_cef_locales([p.name for p in locales_dir.iterdir()], CEF_LOCALES)
    if absent:
        raise SystemExit(f"missing CEF locale packs in {locales_dir}: {', '.join(absent)}")
    for entry in entries:
        dest = tree / entry.name
        if entry.name == "locales":
            shutil.copytree(entry, dest, ignore=_unused_locale_packs)
        elif entry.is_dir():
            shutil.copytree(entry, dest)
        else:
            shutil.copy2(entry, dest)
    return names


def _unused_locale_packs(directory: str, names: list[str]) -> list[str]:
    return [name for name in names if not is_cef_locale_pak(name, CEF_LOCALES)]


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


def dist_name(version: str) -> str:
    return f"{APP_NAME}-{version}-{ARCH}-linux"


def cargo_build_argv(triple: str, profile: str) -> list[str]:
    return ["cargo", "build", "--profile", profile, "--target", triple, "--locked",
            "--no-default-features", "-p", BIN_NAME, "-p", RENDER_PROCESS_PACKAGE]


def companion_cargo_build_argv(triple: str, profile: str, names: tuple[str, ...]) -> list[str]:
    argv = ["cargo", "build", "--profile", profile, "--target", triple, "--locked"]
    for name in names:
        argv += ["-p", name]
    return argv


def stage_binaries(built_dir: Path, tree: Path) -> None:
    for name in (BIN_NAME, *COMPANION_BINS, RENDER_PROCESS_BIN):
        src = built_dir / name
        if not src.is_file():
            raise SystemExit(f"binary not found: {src} (build first, or omit --skip-build)")
        shutil.copy2(src, tree / name)
        print(f"==> staged {name}")


def stage_licenses(repo_root: Path, tree: Path) -> None:
    shutil.copy2(repo_root / "LICENSE", tree / "LICENSE")
    shutil.copy2(repo_root / "licenses" / "THIRD-PARTY-LICENSES.md", tree / "THIRD-PARTY-LICENSES.md")
    (tree / "chromium").mkdir(exist_ok=True)
    shutil.copy2(repo_root / "licenses" / "chromium" / "CREDITS.html", tree / "chromium" / "CREDITS.html")


def stage_desktop_integration(linux_dir: Path, tree: Path) -> None:
    placements = [
        (linux_dir / "install.sh", tree / "install.sh"),
        (linux_dir / "uninstall.sh", tree / "uninstall.sh"),
        (linux_dir / "orzma.desktop", tree / "share" / "applications" / "orzma.desktop"),
    ]
    placements += [
        (icon_path(linux_dir / "icons", size),
         tree / "share" / "icons" / "hicolor" / f"{size}x{size}" / "apps" / "orzma.png")
        for size in ICON_SIZES
    ]
    missing = [str(src) for src, _ in placements if not src.is_file()]
    if missing:
        raise SystemExit(
            f"desktop integration files missing: {', '.join(missing)} "
            "(run `just linux-icons` for the icons)"
        )
    for src, dest in placements:
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(src, dest)
    for script in ("install.sh", "uninstall.sh"):
        (tree / script).chmod(0o755)


def shared_objects(tree: Path) -> list[Path]:
    return sorted(
        path for path in tree.iterdir()
        if path.is_file() and (path.suffix == ".so" or ".so." in path.name)
    )


def strip_argv(paths: list[Path]) -> list[str]:
    return ["strip", "--strip-debug", *(str(path) for path in paths)]


def strip_debug_info(tree: Path) -> None:
    libraries = shared_objects(tree)
    subprocess.run(strip_argv(libraries), check=True)
    print(f"==> stripped debug info from {len(libraries)} shared libraries")


def normalized_mode(path: Path) -> int:
    if path.is_dir() or path.stat().st_mode & 0o111:
        return 0o755
    return 0o644


def write_archive(tree: Path, out: Path) -> None:
    rels = sorted(path.relative_to(tree).as_posix() for path in tree.rglob("*"))
    members = [(tree, tree.name)] + [(tree / rel, f"{tree.name}/{rel}") for rel in rels]
    # NOTE: tarfile's "w:gz" mode stamps the current time and the output file name into
    # the gzip header, so the archive would differ on every run; wrap it explicitly.
    with open(out, "wb") as raw, \
            gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0) as compressed, \
            tarfile.open(fileobj=compressed, mode="w", format=tarfile.PAX_FORMAT) as tar:
        for path, arcname in members:
            info = _tar_info(path, arcname)
            if info.isdir():
                tar.addfile(info)
            else:
                with open(path, "rb") as f:
                    tar.addfile(info, f)


def package(stage_root: Path, version: str, out_dir: Path) -> Path:
    tree = stage_root / dist_name(version)
    if not tree.is_dir():
        raise SystemExit(
            f"stage tree not found: {tree}; run `just stage` first with the same --version"
        )
    out_dir.mkdir(parents=True, exist_ok=True)
    archive = out_dir / f"{dist_name(version)}.tar.gz"
    write_archive(tree, archive)
    print(f"==> wrote {archive}")
    return archive


def build_arg_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(description="Stage and package orzma for Linux")
    p.add_argument("--version", help="version in the archive name; defaults to the Cargo version")
    p.add_argument("--out-dir", default=str(DEFAULT_OUT_DIR))
    p.add_argument("--skip-build", action="store_true",
                   help="reuse already-built binaries instead of running cargo build")
    p.add_argument("--package-only", action="store_true",
                   help="only archive an existing stage tree; builds and stages nothing")
    p.add_argument("--check-deps", action="store_true",
                   help="fail when ldd reports an unresolved shared library in the stage tree")
    return p


def main(argv: list[str] | None = None) -> None:
    args = build_arg_parser().parse_args(argv)
    version = args.version or cargo_version(BIN_NAME)
    out_dir = Path(args.out_dir).expanduser()
    stage_root = out_dir / "stage"
    if args.package_only:
        package(stage_root, version, out_dir)
        return
    cef_path = os.environ.get("CEF_PATH")
    if not cef_path:
        raise SystemExit(
            "CEF_PATH is not set; run through `just stage`, which points it at ~/.cache/orzma/cef"
        )
    verify_orzmd_web_assets()
    verify_orzbrowser_web_assets()
    tree = stage_root / dist_name(version)
    if tree.exists():
        shutil.rmtree(tree)
    tree.mkdir(parents=True)
    if not args.skip_build:
        _cargo_build()
    stage_cef(Path(cef_path).expanduser(), tree)
    strip_debug_info(tree)
    stage_binaries(REPO_ROOT / "target" / TARGET_TRIPLE / CARGO_PROFILE, tree)
    stage_licenses(REPO_ROOT, tree)
    stage_desktop_integration(LINUX_DIR, tree)
    verify_runpaths(tree)
    if args.check_deps:
        verify_deps(tree)
    print(f"version={version}")
    print(f"stage={tree}")


def _capture(argv: list[str]) -> str:
    return subprocess.run(
        argv, capture_output=True, text=True, check=True, env={**os.environ, "LC_ALL": "C"}
    ).stdout


def _cargo_build() -> None:
    for argv in (
        cargo_build_argv(TARGET_TRIPLE, CARGO_PROFILE),
        companion_cargo_build_argv(TARGET_TRIPLE, CARGO_PROFILE, COMPANION_BINS),
    ):
        print(f"==> {' '.join(argv)}")
        subprocess.run(argv, check=True, cwd=str(REPO_ROOT))


def _tar_info(path: Path, arcname: str) -> tarfile.TarInfo:
    info = tarfile.TarInfo(arcname)
    info.mode = normalized_mode(path)
    info.mtime = 0
    info.uid = info.gid = 0
    info.uname = info.gname = ""
    if path.is_dir():
        info.type = tarfile.DIRTYPE
    else:
        info.size = path.stat().st_size
    return info


if __name__ == "__main__":
    main()
