#!/usr/bin/env python3
"""Package the staged Linux tree into an orzma .deb with dpkg-deb."""

from __future__ import annotations

import math
import os
import re
import shutil
import stat
from pathlib import Path

from stage_linux import BIN_NAME, COMPANION_BINS, normalized_mode

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


def _copy_entry(src: Path, dest: Path) -> None:
    dest.parent.mkdir(parents=True, exist_ok=True)
    if src.is_dir():
        shutil.copytree(src, dest, symlinks=True)
    else:
        shutil.copy2(src, dest)


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
