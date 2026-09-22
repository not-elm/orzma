#!/usr/bin/env python3
"""Stage orzma, its companions, and the CEF runtime into a flat tree for the Windows MSI."""

from __future__ import annotations

import hashlib
import json
import struct
import subprocess
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

CRT_STATIC_RUSTFLAGS = "-Ctarget-feature=+crt-static"

RTF_HEADER = r"{\rtf1\ansi\ansicpg1252\deff0{\fonttbl{\f0\fnil Segoe UI;}}\fs18"


def cargo_build_argv(triple: str, profile: str) -> list[str]:
    return ["cargo", "build", "--profile", profile, "--target", triple,
            "--locked", "--no-default-features"]


def companion_cargo_build_argv(triple: str, profile: str, names: tuple[str, ...]) -> list[str]:
    argv = ["cargo", "build", "--profile", profile, "--target", triple, "--locked"]
    for name in names:
        argv += ["-p", name]
    return argv


def render_process_install_argv(version: str, triple: str, root: Path) -> list[str]:
    # NOTE: --target is mandatory. Without it RUSTFLAGS reaches build scripts and
    # build dependencies, and the cc build of ring is compiled against the static CRT.
    # --force keeps a second staging run from failing on the already-installed binary.
    return ["cargo", "install", f"{RENDER_PROCESS_BIN}@{version}",
            "--target", triple, "--root", str(root), "--locked", "--force"]


def cargo_env(base: dict[str, str]) -> dict[str, str]:
    env = dict(base)
    flags = env.get("RUSTFLAGS", "")
    if CRT_STATIC_RUSTFLAGS not in flags.split():
        env["RUSTFLAGS"] = f"{flags} {CRT_STATIC_RUSTFLAGS}".strip()
    return env


def license_rtf(text: str) -> str:
    body = text.replace("\\", "\\\\").replace("{", "\\{").replace("}", "\\}")
    body = body.replace("\n", "\\par\n")
    return f"{RTF_HEADER}\n{body}\n}}"


def cargo_version(package: str) -> str:
    out = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--no-deps"],
        cwd=str(REPO_ROOT), capture_output=True, text=True, check=True,
    ).stdout
    for pkg in json.loads(out)["packages"]:
        if pkg["name"] == package:
            return pkg["version"]
    raise SystemExit(f"package {package} not found in cargo metadata")


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


FORBIDDEN_CRT_IMPORTS = ("vcruntime140", "msvcp140")

IMPORT_DIRECTORY_INDEX = 1
DELAY_IMPORT_DIRECTORY_INDEX = 13
IMPORT_DESCRIPTOR = (IMPORT_DIRECTORY_INDEX, 20, 12)
DELAY_IMPORT_DESCRIPTOR = (DELAY_IMPORT_DIRECTORY_INDEX, 32, 4)


def pe_imported_dlls(path: Path) -> list[str]:
    """DLL names a PE image imports, including delay-loaded imports."""
    with open(path, "rb") as f:
        if _read_at(f, 0, 2) != b"MZ":
            raise ValueError(f"not a PE image: {path}")
        pe_offset = _u32(_read_at(f, 0x3C, 4), 0)
        coff = _read_at(f, pe_offset, 24)
        if coff[:4] != b"PE\0\0":
            raise ValueError(f"missing PE signature: {path}")
        section_count = _u16(coff, 6)
        optional_size = _u16(coff, 20)
        magic = _u16(_read_at(f, pe_offset + 24, 2), 0)
        if magic not in (0x10B, 0x20B):
            raise ValueError(f"unknown optional header magic {magic:#x}: {path}")
        directories_offset = pe_offset + 24 + (96 if magic == 0x10B else 112)
        # NumberOfRvaAndSizes sits immediately before the data directories. Reading a
        # fixed 16 entries would run into the section table on an image that declares
        # fewer, and index 13 would then yield a bogus RVA.
        directory_count = _u32(_read_at(f, directories_offset - 4, 4), 0)
        directories = _read_at(f, directories_offset, 8 * min(directory_count, 16))
        sections = _read_at(f, pe_offset + 24 + optional_size, 40 * section_count)
        names: list[str] = []
        for index, stride, name_field in (IMPORT_DESCRIPTOR, DELAY_IMPORT_DESCRIPTOR):
            if index >= directory_count:
                continue
            rva = _u32(directories, index * 8)
            if rva:
                names += _descriptor_names(f, sections, rva, stride, name_field)
    return sorted({name.lower() for name in names})


def forbidden_crt_imports(names: list[str]) -> list[str]:
    return sorted(
        name for name in names
        if any(name.startswith(prefix) for prefix in FORBIDDEN_CRT_IMPORTS)
    )


def _u16(buf: bytes, offset: int) -> int:
    return struct.unpack_from("<H", buf, offset)[0]


def _u32(buf: bytes, offset: int) -> int:
    return struct.unpack_from("<I", buf, offset)[0]


def _read_at(f, offset: int, size: int) -> bytes:
    f.seek(offset)
    data = f.read(size)
    if len(data) != size:
        raise ValueError(f"unexpected end of file at offset {offset:#x}")
    return data


def _rva_to_offset(sections: bytes, rva: int) -> int:
    for start in range(0, len(sections), 40):
        virtual_address = _u32(sections, start + 12)
        raw_size = _u32(sections, start + 16)
        raw_pointer = _u32(sections, start + 20)
        # NOTE: bound the match by SizeOfRawData, never by VirtualSize. An RVA inside a
        # section's uninitialized tail has no bytes on disk, and a legacy VA-based
        # delay-import descriptor lands outside every section. Both must raise here
        # rather than yield an offset that reads unrelated bytes.
        if raw_pointer and virtual_address <= rva < virtual_address + raw_size:
            return raw_pointer + (rva - virtual_address)
    raise ValueError(f"RVA {rva:#x} has no file-backed section")


def _read_cstring(f, offset: int) -> str:
    f.seek(offset)
    out = bytearray()
    while True:
        chunk = f.read(64)
        if not chunk:
            raise ValueError("unterminated string in PE image")
        end = chunk.find(b"\0")
        if end >= 0:
            out += chunk[:end]
            return out.decode("ascii", "replace")
        out += chunk


def _descriptor_names(f, sections: bytes, rva: int, stride: int, name_field: int) -> list[str]:
    names = []
    offset = _rva_to_offset(sections, rva)
    while True:
        entry = _read_at(f, offset, stride)
        if entry == b"\0" * stride:
            return names
        name_rva = _u32(entry, name_field)
        if name_rva:
            names.append(_read_cstring(f, _rva_to_offset(sections, name_rva)))
        offset += stride


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
