#!/usr/bin/env python3
"""Bundle ozmux into a CEF-embedded macOS .app and package it for Homebrew."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import plistlib
import shutil
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

APP_NAME = "ozmux"
BIN_NAME = "ozmux"
BUNDLE_ID_BASE = "not.elm.ozmux"
ARCH = "arm64"
TARGET_TRIPLE = "aarch64-apple-darwin"
CARGO_PROFILE = "dist"
HELPER_SUFFIXES = ("", " (GPU)", " (Renderer)", " (Plugin)")
MIN_MACOS = "11.0"

REPO_ROOT = Path(__file__).resolve().parent.parent


def zip_name(app_name: str, version: str, arch: str) -> str:
    return f"{app_name}-{version}-{arch}.zip"


def version_less_than(a: str, b: str) -> bool:
    parse = lambda s: [int(p) for p in s.split(".") if p.isdigit()]
    return parse(a) < parse(b)


def helper_bundle_id(base: str, suffix: str) -> str:
    if not suffix:
        return f"{base}.helper"
    raw = suffix.lower().replace(" ", "").replace("(", "").replace(")", "")
    return f"{base}.helper.{raw}"


def merge_cef_keys(plist: dict) -> dict:
    env = plist.get("LSEnvironment")
    if env is None:
        env = {}
    elif not isinstance(env, dict):
        raise ValueError("LSEnvironment exists but is not a dictionary")
    env["MallocNanoZone"] = "0"
    plist["LSEnvironment"] = env

    existing = plist.get("LSMinimumSystemVersion")
    if existing is None or version_less_than(str(existing), MIN_MACOS):
        plist["LSMinimumSystemVersion"] = MIN_MACOS

    plist.setdefault("NSSupportsAutomaticGraphicsSwitching", True)
    return plist


def build_helper_plist(name: str, bundle_id: str) -> dict:
    return {
        "CFBundleExecutable": name,
        "CFBundleName": name,
        "CFBundleIdentifier": bundle_id,
        "CFBundleInfoDictionaryVersion": "6.0",
        "CFBundlePackageType": "APPL",
        "LSEnvironment": {"MallocNanoZone": "0"},
        "LSUIElement": True,
    }


def cargo_build_argv(triple: str, profile: str) -> list[str]:
    return ["cargo", "build", "--profile", profile, "--target", triple,
            "--locked", "--no-default-features"]


def lipo_archs_argv(path: Path) -> list[str]:
    return ["lipo", "-archs", str(path)]


def parse_lipo_archs(output: str) -> set[str]:
    return set(output.split())


def codesign_argv(identity: str, path: Path, *, hardened: bool, entitlements: Path | None) -> list[str]:
    argv = ["codesign", "--force", "--sign", identity]
    if hardened:
        argv += ["--options", "runtime"]
    if entitlements is not None:
        argv += ["--entitlements", str(entitlements)]
    argv.append(str(path))
    return argv


def codesign_verify_argv(path: Path) -> list[str]:
    return ["codesign", "--verify", "--deep", "--strict", str(path)]


def xattr_strip_argv(path: Path) -> list[str]:
    return ["xattr", "-cr", str(path)]


def ditto_zip_argv(app: Path, dest: Path) -> list[str]:
    return ["ditto", "-c", "-k", "--sequesterRsrc", "--keepParent", str(app), str(dest)]


def notarytool_submit_argv(zip_path: Path, apple_id: str, team_id: str, password: str) -> list[str]:
    return [
        "xcrun", "notarytool", "submit", str(zip_path),
        "--apple-id", apple_id, "--team-id", team_id, "--password", password, "--wait",
    ]


def stapler_argv(app: Path) -> list[str]:
    return ["xcrun", "stapler", "staple", str(app)]


def compute_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(65536), b""):
            digest.update(chunk)
    return digest.hexdigest()


def cargo_version(bin_name: str) -> str:
    out = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--no-deps"],
        cwd=str(REPO_ROOT), capture_output=True, text=True, check=True,
    ).stdout
    meta = json.loads(out)
    for pkg in meta["packages"]:
        if pkg["name"] == bin_name:
            return pkg["version"]
    raise SystemExit(f"package {bin_name} not found in cargo metadata")


def build_arg_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(description="Bundle ozmux into a CEF-embedded macOS .app")
    p.add_argument("--version")
    p.add_argument("--bin")
    p.add_argument("--skip-build", action="store_true")
    p.add_argument("--no-sign", action="store_true")
    p.add_argument("--sign-identity")
    p.add_argument("--notarize", action="store_true")
    p.add_argument("--cef-framework",
                   default="~/.local/share/cef/Chromium Embedded Framework.framework")
    p.add_argument("--helper-bin", default="~/.cargo/bin/bevy_cef_render_process")
    p.add_argument("--out-dir", default=str(REPO_ROOT / "target" / "bundle"))
    return p


def resolve_config(args: argparse.Namespace) -> BundleConfig:
    version = args.version or cargo_version(BIN_NAME)
    bin_source = (
        Path(args.bin) if args.bin
        else REPO_ROOT / "target" / TARGET_TRIPLE / CARGO_PROFILE / BIN_NAME
    )
    sign_identity = args.sign_identity or os.environ.get("MACOS_SIGN_IDENTITY") or "-"
    notarize = args.notarize
    if notarize and sign_identity == "-":
        print("==> WARNING: --notarize requires a Developer ID identity; disabling notarization")
        notarize = False
    if notarize and args.no_sign:
        print("==> WARNING: --no-sign skips signing; disabling notarization")
        notarize = False
    return BundleConfig(
        version=version, app_name=APP_NAME, bin_name=BIN_NAME, bundle_id_base=BUNDLE_ID_BASE,
        arch=ARCH, target_triple=TARGET_TRIPLE, bin_source=bin_source,
        cef_framework=Path(args.cef_framework).expanduser(),
        helper_bin=Path(args.helper_bin).expanduser(),
        out_dir=Path(args.out_dir), sign_identity=sign_identity,
        no_sign=args.no_sign, notarize=notarize,
    )


def verify_prerequisites(cfg: BundleConfig) -> None:
    if not cfg.bin_source.is_file():
        raise SystemExit(f"binary not found: {cfg.bin_source} (build first or pass --bin)")
    if not cfg.cef_framework.is_dir():
        raise SystemExit(
            f"CEF framework not found: {cfg.cef_framework} (run `just setup-cef-release`)"
        )
    if not cfg.helper_bin.is_file():
        raise SystemExit(
            f"render-process helper not found: {cfg.helper_bin} (run `just setup-cef-release`)"
        )


def run(argv: list[str], redact: tuple[str, ...] = ()) -> None:
    shown = " ".join("***" if arg in redact else arg for arg in argv)
    print(f"==> {shown}")
    subprocess.run(argv, check=True)


def lipo_archs(path: Path) -> set[str]:
    out = subprocess.run(lipo_archs_argv(path), capture_output=True, text=True, check=True).stdout
    return parse_lipo_archs(out)


def assemble_app(cfg: BundleConfig) -> None:
    app = cfg.app_path
    if app.exists():
        shutil.rmtree(app)
    contents = app / "Contents"
    (contents / "MacOS").mkdir(parents=True)
    (contents / "Resources").mkdir(parents=True)
    (contents / "Frameworks").mkdir(parents=True)

    template = REPO_ROOT / "build" / "macos" / "Info.plist"
    with open(template, "rb") as f:
        plist = plistlib.load(f)
    plist["CFBundleShortVersionString"] = cfg.version
    plist["CFBundleVersion"] = cfg.version
    icns = REPO_ROOT / "build" / "macos" / "AppIcon.icns"
    has_icns = icns.is_file()
    if has_icns:
        plist["CFBundleIconFile"] = "AppIcon.icns"
    with open(contents / "Info.plist", "wb") as f:
        plistlib.dump(plist, f)

    dest_bin = contents / "MacOS" / cfg.bin_name
    shutil.copy2(cfg.bin_source, dest_bin)
    dest_bin.chmod(0o755)

    if has_icns:
        shutil.copy2(icns, contents / "Resources" / "AppIcon.icns")

    print(f"Assembled {app}")


def embed_cef(cfg: BundleConfig) -> None:
    contents = cfg.app_path / "Contents"
    plist_path = contents / "Info.plist"
    with open(plist_path, "rb") as f:
        plist = plistlib.load(f)
    merge_cef_keys(plist)
    with open(plist_path, "wb") as f:
        plistlib.dump(plist, f)

    main_bin = contents / "MacOS" / cfg.bin_name
    cef_bin = cfg.cef_framework / "Chromium Embedded Framework"
    common = lipo_archs(main_bin) & lipo_archs(cfg.helper_bin) & lipo_archs(cef_bin)
    if not common:
        raise SystemExit(
            "architecture mismatch: no common arch among main/helper/CEF binaries"
        )
    print(f"Architecture check passed (common: {', '.join(sorted(common))})")

    frameworks = contents / "Frameworks"
    old_cef = frameworks / "Chromium Embedded Framework.framework"
    if old_cef.exists():
        shutil.rmtree(old_cef)
    for suffix in HELPER_SUFFIXES:
        helper_app = frameworks / f"{cfg.bin_name} Helper{suffix}.app"
        if helper_app.exists():
            shutil.rmtree(helper_app)

    run(["cp", "-R", str(cfg.cef_framework), str(frameworks)])

    for suffix in HELPER_SUFFIXES:
        helper_name = f"{cfg.bin_name} Helper{suffix}"
        helper_app = frameworks / f"{helper_name}.app"
        macos = helper_app / "Contents" / "MacOS"
        macos.mkdir(parents=True)
        dest = macos / helper_name
        shutil.copy2(cfg.helper_bin, dest)
        dest.chmod(0o755)
        hp = build_helper_plist(helper_name, helper_bundle_id(cfg.bundle_id_base, suffix))
        with open(helper_app / "Contents" / "Info.plist", "wb") as f:
            plistlib.dump(hp, f)
        print(f"  Created {helper_name}.app")


def strip_xattrs(cfg: BundleConfig) -> None:
    run(xattr_strip_argv(cfg.app_path))


def codesign_bundle(cfg: BundleConfig) -> None:
    hardened = cfg.sign_identity != "-"
    entitlements = (REPO_ROOT / "build" / "macos" / "Entitlements.plist") if hardened else None
    cef_fw = cfg.app_path / "Contents" / "Frameworks" / "Chromium Embedded Framework.framework"

    libs = cef_fw / "Libraries"
    if libs.is_dir():
        for dylib in sorted(libs.glob("*.dylib")):
            run(codesign_argv(cfg.sign_identity, dylib, hardened=hardened, entitlements=entitlements))
    run(codesign_argv(cfg.sign_identity, cef_fw / "Chromium Embedded Framework",
                      hardened=hardened, entitlements=entitlements))
    run(codesign_argv(cfg.sign_identity, cef_fw, hardened=hardened, entitlements=entitlements))

    frameworks = cfg.app_path / "Contents" / "Frameworks"
    for suffix in HELPER_SUFFIXES:
        helper_app = frameworks / f"{cfg.bin_name} Helper{suffix}.app"
        run(codesign_argv(cfg.sign_identity, helper_app, hardened=hardened, entitlements=entitlements))

    run(codesign_argv(cfg.sign_identity, cfg.app_path, hardened=hardened, entitlements=entitlements))
    run(codesign_verify_argv(cfg.app_path))


def notarize(cfg: BundleConfig) -> None:
    apple_id = os.environ.get("APPLE_ID")
    team_id = os.environ.get("APPLE_TEAM_ID")
    password = os.environ.get("APPLE_APP_PASSWORD")
    missing = [name for name, value in (
        ("APPLE_ID", apple_id),
        ("APPLE_TEAM_ID", team_id),
        ("APPLE_APP_PASSWORD", password),
    ) if not value]
    if missing:
        raise SystemExit(
            "--notarize requires these environment variables: " + ", ".join(missing)
        )
    tmp_zip = cfg.out_dir / "notarize-upload.zip"
    if tmp_zip.exists():
        tmp_zip.unlink()
    run(ditto_zip_argv(cfg.app_path, tmp_zip))
    run(notarytool_submit_argv(tmp_zip, apple_id, team_id, password), redact=(password,))
    run(stapler_argv(cfg.app_path))
    tmp_zip.unlink(missing_ok=True)


def package(cfg: BundleConfig) -> str:
    dest = cfg.zip_path
    if dest.exists():
        dest.unlink()
    run(ditto_zip_argv(cfg.app_path, dest))
    digest = compute_sha256(dest)
    (dest.parent / (dest.name + ".sha256")).write_text(f"{digest}  {dest.name}\n")
    return digest


def cargo_build(cfg: BundleConfig) -> None:
    run(cargo_build_argv(cfg.target_triple, CARGO_PROFILE))


def main(argv: list[str] | None = None) -> None:
    args = build_arg_parser().parse_args(argv)
    cfg = resolve_config(args)
    cfg.out_dir.mkdir(parents=True, exist_ok=True)

    if not args.skip_build:
        cargo_build(cfg)
    verify_prerequisites(cfg)
    assemble_app(cfg)
    embed_cef(cfg)
    strip_xattrs(cfg)
    if not cfg.no_sign:
        codesign_bundle(cfg)
    if cfg.notarize:
        notarize(cfg)
    digest = package(cfg)

    print(f"version={cfg.version}")
    print(f"sha256={digest}")
    print(f"artifact={cfg.zip_path}")


@dataclass
class BundleConfig:
    version: str
    app_name: str
    bin_name: str
    bundle_id_base: str
    arch: str
    target_triple: str
    bin_source: Path
    cef_framework: Path
    helper_bin: Path
    out_dir: Path
    sign_identity: str
    no_sign: bool
    notarize: bool

    @property
    def app_path(self) -> Path:
        return self.out_dir / f"{self.app_name}.app"

    @property
    def zip_path(self) -> Path:
        return self.out_dir / zip_name(self.app_name, self.version, self.arch)


if __name__ == "__main__":
    main()
