#!/usr/bin/env python3
"""Bundle orzma into a CEF-embedded macOS .app and package it as a release dmg."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import plistlib
import shutil
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass, field
from pathlib import Path

APP_NAME = "orzma"
BIN_NAME = "orzma"
BUNDLE_ID_BASE = "not.elm.orzma"
ARCH = "arm64"
TARGET_TRIPLE = "aarch64-apple-darwin"
CARGO_PROFILE = "dist"
HELPER_SUFFIXES = ("", " (GPU)", " (Renderer)", " (Plugin)")
COMPANION_BINS = ("orzbrowser", "orzmd")
# NOTE: `just setup-cef` drops the debug render process into the shared CEF
# framework's Libraries/ for `cargo run`. The bundle's helpers use the release
# render process instead, so this copy is dead weight (~41 MiB) and, being a
# non-dylib executable, is also skipped by the Libraries/ signing loop.
DEV_ONLY_CEF_LIBRARIES = ("bevy_cef_debug_render_process",)
# NOTE: on macOS CEF resolves its locale from the OUTER app bundle's advertised
# localizations, not from CefSettings.locale -- Chromium's
# OverrideLocaleWithCocoaLocale() takes precedence over the pref, and maps "en"
# to "en-US" whose pack is on disk as en.lproj. Shipping any other pack is dead
# weight, and dropping the one CEF resolves is fatal rather than degraded: the
# empty lookup becomes `CHECK failed: !loaded_locale.empty()` during startup.
SHIPPED_CEF_LOCALES = ("en",)
# NOTE: CEF's macOS redistribution README marks these optional. Dropping
# SwiftShader stays safe only while no switch enables software rendering --
# `disable-gpu` or `--enable-unsafe-swiftshader` would make canvas, 3D CSS and
# WebGL fail outright instead of falling back.
OPTIONAL_CEF_LIBRARIES = ("libvk_swiftshader.dylib", "vk_swiftshader_icd.json")
OPTIONAL_CEF_RESOURCES = ("gpu_shader_cache.bin",)
MIN_MACOS = "11.0"
DMG_FORMAT = "ULMO"
DMG_FILESYSTEM = "HFS+"
# NOTE: on GitHub's macOS runners `hdiutil create` intermittently fails with
# "Resource busy", and `hdiutil verify` with "Resource temporarily unavailable"
# while the disk-image helper still locks the new image; without the retry a
# release build fails at random.
HDIUTIL_ATTEMPTS = 3
HDIUTIL_RETRY_DELAY_SECONDS = 5

REPO_ROOT = Path(__file__).resolve().parent.parent


def dmg_name(app_name: str, version: str, arch: str) -> str:
    return f"{app_name}-{version}-{arch}.dmg"


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


def companion_cargo_build_argv(triple: str, profile: str, names: tuple[str, ...]) -> list[str]:
    argv = ["cargo", "build", "--profile", profile, "--target", triple, "--locked"]
    for name in names:
        argv += ["-p", name]
    return argv


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


def codesign_verify_one_argv(path: Path) -> list[str]:
    return ["codesign", "--verify", str(path)]


def xattr_strip_argv(path: Path) -> list[str]:
    return ["xattr", "-cr", str(path)]


def ditto_zip_argv(app: Path, dest: Path) -> list[str]:
    return ["ditto", "-c", "-k", "--sequesterRsrc", "--keepParent", str(app), str(dest)]


def ditto_copy_argv(src: Path, dest: Path) -> list[str]:
    return ["ditto", str(src), str(dest)]


def hdiutil_create_argv(volname: str, srcfolder: Path, dest: Path) -> list[str]:
    return [
        "hdiutil", "create", "-volname", volname, "-srcfolder", str(srcfolder),
        "-fs", DMG_FILESYSTEM, "-format", DMG_FORMAT, "-ov", str(dest),
    ]


def hdiutil_verify_argv(dmg: Path) -> list[str]:
    return ["hdiutil", "verify", str(dmg)]


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
    p = argparse.ArgumentParser(description="Bundle orzma into a CEF-embedded macOS .app")
    p.add_argument("--version")
    p.add_argument("--bin")
    p.add_argument("--orzbrowser-bin")
    p.add_argument("--orzmd-bin")
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
    companion_bins = {}
    for name in COMPANION_BINS:
        override = getattr(args, f"{name}_bin", None)
        if override:
            companion_bins[name] = Path(override).expanduser()
        else:
            companion_bins[name] = REPO_ROOT / "target" / TARGET_TRIPLE / CARGO_PROFILE / name
    return BundleConfig(
        version=version, app_name=APP_NAME, bin_name=BIN_NAME, bundle_id_base=BUNDLE_ID_BASE,
        arch=ARCH, target_triple=TARGET_TRIPLE, bin_source=bin_source,
        cef_framework=Path(args.cef_framework).expanduser(),
        helper_bin=Path(args.helper_bin).expanduser(),
        out_dir=Path(args.out_dir), sign_identity=sign_identity,
        no_sign=args.no_sign, notarize=notarize,
        companion_bins=companion_bins,
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
    for name, src in cfg.companion_bins.items():
        if not src.is_file():
            raise SystemExit(
                f"companion binary not found: {src} (build first or pass --{name}-bin)"
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


def is_cef_locale_dir(name: str, locales: tuple[str, ...]) -> bool:
    return any(name == f"{locale}.lproj" for locale in locales)


def missing_cef_locale_dirs(names: list[str], locales: tuple[str, ...]) -> list[str]:
    present = set(names)
    return [locale for locale in locales if f"{locale}.lproj" not in present]


def app_advertised_localizations(plist: dict, resource_names: list[str]) -> set[str]:
    advertised = set(plist.get("CFBundleLocalizations") or ())
    advertised |= {
        name.removesuffix(".lproj") for name in resource_names if name.endswith(".lproj")
    }
    # NOTE: the development region is unioned in even when the bundle advertises
    # localizations explicitly, which over-reports what NSBundle would resolve.
    # Narrowing it to the empty-advertisement case reopens the startup abort this
    # guard exists to prevent, and a false build failure is the cheaper error.
    region = plist.get("CFBundleDevelopmentRegion")
    if region:
        advertised.add(str(region))
    return advertised


def dir_entry_names(path: Path) -> list[str]:
    return sorted(p.name for p in path.iterdir()) if path.is_dir() else []


def verify_cef_locales(cfg: BundleConfig, plist: dict) -> None:
    source = cfg.cef_framework / "Resources"
    absent = missing_cef_locale_dirs(dir_entry_names(source), SHIPPED_CEF_LOCALES)
    if absent:
        raise SystemExit(
            f"missing CEF locale packs in {source}: "
            f"{', '.join(f'{name}.lproj' for name in absent)}"
        )
    advertised = app_advertised_localizations(
        plist, dir_entry_names(cfg.app_path / "Contents" / "Resources")
    )
    unshipped = sorted(advertised - set(SHIPPED_CEF_LOCALES))
    if unshipped:
        raise SystemExit(
            f"{cfg.app_name}.app advertises localizations with no CEF locale pack: "
            f"{', '.join(unshipped)} (shipped: {', '.join(SHIPPED_CEF_LOCALES)}); "
            "CEF would abort at startup on a system preferring one of them"
        )


def prune_cef_framework(framework: Path) -> None:
    resources = framework / "Resources"
    for lproj in sorted(resources.glob("*.lproj")):
        if not is_cef_locale_dir(lproj.name, SHIPPED_CEF_LOCALES):
            shutil.rmtree(lproj)
    print(f"  Kept CEF locale packs: {', '.join(SHIPPED_CEF_LOCALES)}")
    for name in OPTIONAL_CEF_LIBRARIES:
        optional = framework / "Libraries" / name
        if optional.exists():
            optional.unlink()
            print(f"  Removed optional {name}")
    for name in OPTIONAL_CEF_RESOURCES:
        optional = resources / name
        if optional.exists():
            optional.unlink()
            print(f"  Removed optional {name}")


def embed_cef(cfg: BundleConfig) -> None:
    contents = cfg.app_path / "Contents"
    plist_path = contents / "Info.plist"
    with open(plist_path, "rb") as f:
        plist = plistlib.load(f)
    merge_cef_keys(plist)
    with open(plist_path, "wb") as f:
        plistlib.dump(plist, f)

    verify_cef_locales(cfg, plist)

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
    for name in DEV_ONLY_CEF_LIBRARIES:
        dev_only = old_cef / "Libraries" / name
        if dev_only.exists():
            dev_only.unlink()
            print(f"  Removed dev-only {name}")
    prune_cef_framework(old_cef)

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


def copy_companions(cfg: BundleConfig) -> None:
    resources = cfg.resources_path
    resources.mkdir(parents=True, exist_ok=True)
    for name, src in cfg.companion_bins.items():
        dest = resources / name
        shutil.copy2(src, dest)
        dest.chmod(0o755)
        print(f"  Embedded companion {name}")


def copy_licenses(cfg: BundleConfig) -> None:
    resources = cfg.resources_path
    resources.mkdir(parents=True, exist_ok=True)
    for src in (
        REPO_ROOT / "licenses" / "THIRD-PARTY-LICENSES.md",
        REPO_ROOT / "licenses" / "chromium" / "CREDITS.html",
    ):
        dest = resources / src.name
        shutil.copy2(src, dest)
        print(f"  Embedded license file {src.name}")


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

    resources = cfg.resources_path
    # NOTE: companions are plain CLIs (no CEF) exposed directly on the user's
    # PATH; sign them with the hardened runtime but WITHOUT entitlements. They
    # must not inherit the CEF grants (JIT, unsigned-executable-memory, disabled
    # library validation) — least privilege. Do not unify this with the helper
    # signing above.
    # NOTE: codesign --verify --deep --strict on the outer app does NOT descend
    # into plain executables in Contents/Resources, so verify each companion
    # explicitly here — otherwise a broken signing loop ships unsigned binaries
    # with no error on the local/ad-hoc path.
    for name in cfg.companion_bins:
        run(codesign_argv(cfg.sign_identity, resources / name,
                          hardened=hardened, entitlements=None))
        run(codesign_verify_one_argv(resources / name))

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


def stage_dmg(app: Path, staging: Path) -> None:
    staging.chmod(0o755)
    run(ditto_copy_argv(app, staging / app.name))
    (staging / "Applications").symlink_to("/Applications")


def run_hdiutil(argv: list[str]) -> None:
    for attempt in range(1, HDIUTIL_ATTEMPTS + 1):
        try:
            run(argv)
            return
        except subprocess.CalledProcessError:
            if attempt == HDIUTIL_ATTEMPTS:
                raise
            print(
                f"==> {' '.join(argv[:2])} failed (attempt {attempt}/{HDIUTIL_ATTEMPTS}); "
                f"retrying in {HDIUTIL_RETRY_DELAY_SECONDS}s"
            )
            time.sleep(HDIUTIL_RETRY_DELAY_SECONDS)


def package(cfg: BundleConfig) -> str:
    dest = cfg.dmg_path
    sidecar = dest.with_name(dest.name + ".sha256")
    dest.unlink(missing_ok=True)
    sidecar.unlink(missing_ok=True)
    try:
        with tempfile.TemporaryDirectory(prefix="dmg-staging-", dir=cfg.out_dir) as tmp:
            staging = Path(tmp)
            stage_dmg(cfg.app_path, staging)
            run_hdiutil(hdiutil_create_argv(cfg.app_name, staging, dest))
        run_hdiutil(hdiutil_verify_argv(dest))
    except BaseException:
        dest.unlink(missing_ok=True)
        raise
    digest = compute_sha256(dest)
    sidecar.write_text(f"{digest}  {dest.name}\n")
    return digest


def verify_orzmd_web_assets(assets_dir: Path | None = None) -> None:
    assets = assets_dir if assets_dir is not None else REPO_ROOT / "apps" / "orzmd" / "assets"
    real = (
        [p for p in assets.glob("*") if p.name not in {".gitignore", ".gitkeep"}]
        if assets.is_dir() else []
    )
    if not real:
        raise SystemExit(
            "orzmd web assets missing: apps/orzmd/assets/ has only placeholders. "
            "Run `pnpm build` (or `just orzmd-web`) before bundling, "
            "or orzmd will ship a blank viewer."
        )


def verify_orzbrowser_web_assets(assets_dir: Path | None = None) -> None:
    assets = assets_dir if assets_dir is not None else REPO_ROOT / "apps" / "orzbrowser" / "assets"
    if not (assets / "chrome.html").is_file():
        raise SystemExit(
            "orzbrowser chrome page missing: apps/orzbrowser/assets/chrome.html was not built. "
            "Run `pnpm build` (or `just orzbrowser-web`) before bundling, "
            "or orzbrowser will fail to start."
        )


def cargo_build(cfg: BundleConfig) -> None:
    run(cargo_build_argv(cfg.target_triple, CARGO_PROFILE))
    verify_orzmd_web_assets()
    verify_orzbrowser_web_assets()
    run(companion_cargo_build_argv(cfg.target_triple, CARGO_PROFILE, COMPANION_BINS))


def main(argv: list[str] | None = None) -> None:
    # NOTE: CI pipes stdout, which Python then block-buffers; without line
    # buffering the "==>" progress lines land far from the subprocess output
    # they describe, including the hdiutil retry notices.
    sys.stdout.reconfigure(line_buffering=True)
    args = build_arg_parser().parse_args(argv)
    cfg = resolve_config(args)
    cfg.out_dir.mkdir(parents=True, exist_ok=True)

    if not args.skip_build:
        cargo_build(cfg)
    verify_prerequisites(cfg)
    assemble_app(cfg)
    embed_cef(cfg)
    copy_companions(cfg)
    copy_licenses(cfg)
    strip_xattrs(cfg)
    if not cfg.no_sign:
        codesign_bundle(cfg)
    if cfg.notarize:
        notarize(cfg)
    digest = package(cfg)

    print(f"version={cfg.version}")
    print(f"sha256={digest}")
    print(f"artifact={cfg.dmg_path}")


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
    companion_bins: dict[str, Path] = field(default_factory=dict)

    @property
    def app_path(self) -> Path:
        return self.out_dir / f"{self.app_name}.app"

    @property
    def dmg_path(self) -> Path:
        return self.out_dir / dmg_name(self.app_name, self.version, self.arch)

    @property
    def resources_path(self) -> Path:
        return self.app_path / "Contents" / "Resources"


if __name__ == "__main__":
    main()
