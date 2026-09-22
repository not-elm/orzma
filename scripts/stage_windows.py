#!/usr/bin/env python3
"""Stage orzma, its companions, and the CEF runtime into a flat tree for the Windows MSI."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import struct
import subprocess
from dataclasses import dataclass
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


@dataclass
class StageConfig:
    version: str
    cef_dir: Path
    out_dir: Path
    render_process_bin: Path | None
    skip_build: bool

    @property
    def stage_dir(self) -> Path:
        return self.out_dir / "stage"

    @property
    def tools_dir(self) -> Path:
        return self.out_dir / "tools"


def build_arg_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(description="Stage orzma for the Windows MSI")
    p.add_argument("--version")
    p.add_argument("--cef-dir", default="~/.local/share/cef")
    p.add_argument("--render-process-bin")
    p.add_argument("--skip-build", action="store_true")
    p.add_argument("--out-dir", default=str(REPO_ROOT / "target" / "dist"))
    p.add_argument("--refresh-inventory", action="store_true",
                   help="rewrite build/windows/cef-inventory.json from --cef-dir and exit")
    p.add_argument("--cef-version", help="required with --refresh-inventory")
    return p


def resolve_config(args: argparse.Namespace) -> StageConfig:
    return StageConfig(
        version=args.version or cargo_version(BIN_NAME),
        cef_dir=Path(args.cef_dir).expanduser(),
        out_dir=Path(args.out_dir).expanduser(),
        render_process_bin=(
            Path(args.render_process_bin).expanduser() if args.render_process_bin else None
        ),
        skip_build=args.skip_build,
    )


def run(argv: list[str], env: dict[str, str] | None = None) -> None:
    print(f"==> {' '.join(argv)}")
    subprocess.run(argv, check=True, cwd=str(REPO_ROOT), env=env)


def verify_orzmd_web_assets(assets_dir: Path | None = None) -> None:
    assets = assets_dir if assets_dir is not None else REPO_ROOT / "apps" / "orzmd" / "assets"
    real = (
        [p for p in assets.glob("*") if p.name not in {".gitignore", ".gitkeep"}]
        if assets.is_dir() else []
    )
    if not real:
        raise SystemExit(
            "orzmd web assets missing: apps/orzmd/assets/ has only placeholders. "
            "Run `pnpm build` (or `just orzmd-web`) before staging, "
            "or orzmd will ship a blank viewer."
        )


def cargo_build(cfg: StageConfig) -> None:
    env = cargo_env(dict(os.environ))
    run(cargo_build_argv(TARGET_TRIPLE, CARGO_PROFILE), env=env)
    verify_orzmd_web_assets()
    run(companion_cargo_build_argv(TARGET_TRIPLE, CARGO_PROFILE, COMPANION_BINS), env=env)
    run(render_process_install_argv(RENDER_PROCESS_VERSION, TARGET_TRIPLE, cfg.tools_dir), env=env)


def assert_inventory_clean(missing: list[str], unclassified: list[str], mismatched: list[str]) -> None:
    problems = []
    if missing:
        problems.append(f"missing required CEF files: {', '.join(missing)}")
    if unclassified:
        problems.append(
            "unclassified CEF files (add them to required/optional/build_only in "
            f"{INVENTORY_PATH.name}): {', '.join(unclassified)}"
        )
    if mismatched:
        problems.append(
            "sha256 mismatch, the CEF directory does not match the inventory "
            f"(run `just cef-inventory-refresh` after a cef_version bump): {', '.join(mismatched)}"
        )
    if problems:
        raise SystemExit("; ".join(problems))


def copy_cef_entries(cef_dir: Path, stage_dir: Path, staged: list[str]) -> None:
    for rel in staged:
        dest = stage_dir / rel
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(cef_dir / rel, dest)


def stage_cef(cfg: StageConfig) -> None:
    inventory = load_inventory(INVENTORY_PATH)
    build_only = set(inventory["build_only"])
    required, optional = inventory["required"], inventory["optional"]
    entries = iter_cef_files(cfg.cef_dir, build_only)
    staged, missing, unclassified = classify_entries(entries, required, optional)
    digests = {**required, **optional}
    assert_inventory_clean(missing, unclassified, digest_mismatches(cfg.cef_dir, staged, digests))
    copy_cef_entries(cfg.cef_dir, cfg.stage_dir, staged)
    print(f"==> staged {len(staged)} CEF files")


def binary_sources(cfg: StageConfig) -> dict[str, Path]:
    built = REPO_ROOT / "target" / TARGET_TRIPLE / CARGO_PROFILE
    sources = {f"{name}.exe": built / f"{name}.exe" for name in (BIN_NAME, *COMPANION_BINS)}
    sources[f"{RENDER_PROCESS_BIN}.exe"] = (
        cfg.render_process_bin
        if cfg.render_process_bin
        else cfg.tools_dir / "bin" / f"{RENDER_PROCESS_BIN}.exe"
    )
    return sources


def stage_binaries(cfg: StageConfig) -> None:
    for name, src in binary_sources(cfg).items():
        if not src.is_file():
            raise SystemExit(f"binary not found: {src} (build first or pass --skip-build off)")
        shutil.copy2(src, cfg.stage_dir / name)
        print(f"==> staged {name}")


def stage_licenses(cfg: StageConfig) -> None:
    for src in (REPO_ROOT / "licenses" / "THIRD-PARTY-LICENSES.md",):
        shutil.copy2(src, cfg.stage_dir / src.name)
    rtf = license_rtf((REPO_ROOT / "LICENSE").read_text(encoding="utf-8"))
    (cfg.stage_dir / "license.rtf").write_text(rtf, encoding="ascii", newline="\r\n")


def verify_crt(cfg: StageConfig) -> None:
    offenders = {}
    for path in sorted(cfg.stage_dir.rglob("*")):
        if path.suffix.lower() not in (".exe", ".dll"):
            continue
        try:
            imported = pe_imported_dlls(path)
        except ValueError as error:
            raise SystemExit(f"cannot read the import table of {path.name}: {error}") from error
        found = forbidden_crt_imports(imported)
        if found:
            offenders[path.name] = found
    if offenders:
        raise SystemExit(
            "dynamic CRT dependency found; the build did not use "
            f"{CRT_STATIC_RUSTFLAGS}: {offenders}"
        )
    print("==> CRT check passed (no vcruntime140/msvcp140 imports)")


def main(argv: list[str] | None = None) -> None:
    args = build_arg_parser().parse_args(argv)
    if args.refresh_inventory:
        if not args.cef_version:
            raise SystemExit("--refresh-inventory requires --cef-version")
        refresh_inventory(Path(args.cef_dir).expanduser(), args.cef_version, INVENTORY_PATH)
        return
    cfg = resolve_config(args)
    if cfg.stage_dir.exists():
        shutil.rmtree(cfg.stage_dir)
    cfg.stage_dir.mkdir(parents=True)
    if not cfg.skip_build:
        cargo_build(cfg)
    stage_cef(cfg)
    stage_binaries(cfg)
    stage_licenses(cfg)
    verify_crt(cfg)
    print(f"version={cfg.version}")
    print(f"stage={cfg.stage_dir}")


if __name__ == "__main__":
    main()
