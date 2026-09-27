#!/usr/bin/env python3
"""Package the staged Linux tree into an orzma .deb with dpkg-deb."""

from __future__ import annotations

import argparse
import math
import os
import re
import shutil
import stat
import subprocess
from pathlib import Path

from stage_linux import BIN_NAME, COMPANION_BINS, DEFAULT_OUT_DIR, dist_name, normalized_mode, write_sidecar
from stage_windows import cargo_version

PACKAGE = "orzma"
DEB_ARCH = "amd64"
INSTALL_DIR = Path("usr/lib/orzma")
DOC_DIR = Path("usr/share/doc/orzma")
BIN_DIR = Path("usr/bin")
APPLICATIONS_DIR = Path("usr/share/applications")
ICONS_DIR = Path("usr/share/icons")
LAUNCHERS = (BIN_NAME, *COMPANION_BINS)
DESKTOP_FILE = "orzma.desktop"
EXEC_PLACEHOLDER = "@ORZMA_EXEC@"
EXEC_PATH = f"/usr/bin/{BIN_NAME}"
MAINTAINER = "notelm <notelm@users.noreply.github.com>"
HOMEPAGE = "https://github.com/not-elm/orzma"
SUMMARY = "Terminal emulator with in-process webviews"
LONG_DESCRIPTION = "orzma is a GPU-rendered terminal emulator that displays webviews inside the terminal."
DEPENDS = (
    "libc6 (>= 2.35)",
    "libnss3",
    "libnspr4",
    "libatk1.0-0",
    "libatk-bridge2.0-0",
    "libcups2",
    "libdrm2",
    "libgbm1",
    "libxkbcommon0",
    "libxcomposite1",
    "libxdamage1",
    "libxrandr2",
    "libxfixes3",
    "libpango-1.0-0",
    "libcairo2",
    "libgtk-3-0",
    "libasound2t64 | libasound2",
    "libdbus-1-3",
    "libglib2.0-0",
    "libudev1",
    "libwayland-client0",
    "libfontconfig1",
    # winit, wgpu and Bevy dlopen these, so neither ldd nor the ELF NEEDED list shows them.
    "libx11-6",
    "libx11-xcb1",
    "libxcursor1",
    "libxi6",
    "libxkbcommon-x11-0",
    "libvulkan1",
    "libegl1",
)
RECOMMENDS = ("mesa-vulkan-drivers | vulkan-icd",)
STAGE_ONLY_ENTRIES = frozenset({"install.sh", "uninstall.sh", "share"})
LICENSE_FILE = "LICENSE"
DOC_ENTRIES = frozenset({"THIRD-PARTY-LICENSES.md", "chromium"})
DEB_VERSION_RE = re.compile(r"[0-9][0-9A-Za-z.+~]*")
SCRATCH_DIR_NAME = "deb-root"


def deb_version(version: str) -> str:
    """The Debian control version: prerelease `-` becomes `~` so it sorts before the release."""
    mapped = version.replace("-", "~")
    if not DEB_VERSION_RE.fullmatch(mapped):
        raise SystemExit(f"version '{version}' cannot be expressed as a Debian package version")
    return mapped


def deb_file_name(version: str) -> str:
    deb_version(version)
    return f"{PACKAGE}_{version}_{DEB_ARCH}.deb"


def render_control(version: str, installed_size: int) -> str:
    lines = [
        f"Package: {PACKAGE}",
        f"Version: {deb_version(version)}",
        f"Architecture: {DEB_ARCH}",
        "Section: x11",
        "Priority: optional",
        f"Maintainer: {MAINTAINER}",
        f"Homepage: {HOMEPAGE}",
        f"Installed-Size: {installed_size}",
        f"Depends: {', '.join(DEPENDS)}",
        f"Recommends: {', '.join(RECOMMENDS)}",
        f"Description: {SUMMARY}",
        f" {LONG_DESCRIPTION}",
    ]
    return "\n".join(lines) + "\n"


def installed_size_kib(root: Path) -> int:
    total = 0
    for path in root.rglob("*"):
        st = path.lstat()
        if stat.S_ISREG(st.st_mode):
            total += math.ceil(st.st_size / 1024)
        else:
            total += 1
    return total


def assemble_deb_root(tree: Path, root: Path) -> None:
    missing = [name for name in (*LAUNCHERS, LICENSE_FILE) if not (tree / name).is_file()]
    if not (tree / "share" / "icons").is_dir():
        missing.append("share/icons")
    if missing:
        raise SystemExit(f"stage tree {tree} lacks {', '.join(missing)}; rerun `just stage`")
    for entry in sorted(tree.iterdir()):
        if entry.name in STAGE_ONLY_ENTRIES or entry.name == LICENSE_FILE:
            continue
        dest_dir = DOC_DIR if entry.name in DOC_ENTRIES else INSTALL_DIR
        _copy_entry(entry, root / dest_dir / entry.name)
    _write_copyright(tree / LICENSE_FILE, root / DOC_DIR / "copyright")
    for name in LAUNCHERS:
        link = root / BIN_DIR / name
        link.parent.mkdir(parents=True, exist_ok=True)
        os.symlink(f"../lib/{PACKAGE}/{name}", link)
    _install_desktop_entry(
        tree / "share" / "applications" / DESKTOP_FILE, root / APPLICATIONS_DIR / DESKTOP_FILE
    )
    shutil.copytree(tree / "share" / "icons", root / ICONS_DIR, dirs_exist_ok=True)


def normalize_modes(root: Path) -> None:
    for path in (root, *root.rglob("*")):
        if stat.S_ISLNK(path.lstat().st_mode):
            continue
        path.chmod(normalized_mode(path))


def dpkg_deb_argv(root: Path, out: Path) -> list[str]:
    return ["dpkg-deb", "--root-owner-group", "-Zxz", "--build", str(root), str(out)]


def build_deb(tree: Path, version: str, out_dir: Path) -> Path:
    if shutil.which("dpkg-deb") is None:
        raise SystemExit("dpkg-deb not found; the .deb can only be built on a Debian/Ubuntu host")
    out_dir.mkdir(parents=True, exist_ok=True)
    out = out_dir / deb_file_name(version)
    root = out_dir / SCRATCH_DIR_NAME
    if root.exists():
        shutil.rmtree(root)
    root.mkdir()
    try:
        assemble_deb_root(tree, root)
        normalize_modes(root)
        control = render_control(version, installed_size_kib(root))
        debian = root / "DEBIAN"
        debian.mkdir()
        debian.chmod(0o755)
        (debian / "control").write_text(control, encoding="utf-8", newline="\n")
        (debian / "control").chmod(0o644)
        env = {**os.environ, "SOURCE_DATE_EPOCH": os.environ.get("SOURCE_DATE_EPOCH", "0")}
        subprocess.run(dpkg_deb_argv(root, out), check=True, env=env)
    finally:
        shutil.rmtree(root, ignore_errors=True)
    sidecar = write_sidecar(out)
    print(f"==> wrote {out} and {sidecar.name}")
    return out


def package_deb(stage_root: Path, version: str, out_dir: Path) -> Path:
    tree = stage_root / dist_name(version)
    if not tree.is_dir():
        raise SystemExit(
            f"stage tree not found: {tree}; run `just stage` first with the same --version"
        )
    return build_deb(tree, version, out_dir)


def build_arg_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(description="Package the staged Linux tree into an orzma .deb")
    p.add_argument("--version", help="package version; defaults to the Cargo version")
    p.add_argument("--out-dir", default=str(DEFAULT_OUT_DIR))
    return p


def main(argv: list[str] | None = None) -> None:
    args = build_arg_parser().parse_args(argv)
    version = args.version or cargo_version(BIN_NAME)
    out_dir = Path(args.out_dir).expanduser()
    package_deb(out_dir / "stage", version, out_dir)


def _copy_entry(src: Path, dest: Path) -> None:
    dest.parent.mkdir(parents=True, exist_ok=True)
    if src.is_dir() and not src.is_symlink():
        shutil.copytree(src, dest, symlinks=True)
    else:
        shutil.copy2(src, dest, follow_symlinks=False)


def _write_copyright(license_file: Path, dest: Path) -> None:
    dest.parent.mkdir(parents=True, exist_ok=True)
    header = f"orzma is distributed under the license below.\nSource: {HOMEPAGE}\n\n"
    dest.write_text(header + license_file.read_text(encoding="utf-8"), encoding="utf-8")


def _install_desktop_entry(template: Path, dest: Path) -> None:
    if not template.is_file():
        raise SystemExit(f"desktop entry template not found: {template}; rerun `just stage`")
    text = template.read_text(encoding="utf-8")
    if EXEC_PLACEHOLDER not in text:
        raise SystemExit(f"{template} has no {EXEC_PLACEHOLDER} placeholder to substitute")
    dest.parent.mkdir(parents=True, exist_ok=True)
    dest.write_text(text.replace(EXEC_PLACEHOLDER, EXEC_PATH), encoding="utf-8")


if __name__ == "__main__":
    main()
