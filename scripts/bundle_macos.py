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
BIN_NAME = "ozmux-gui"
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
    return ["cargo", "build", "--profile", profile, "--target", triple, "--locked"]


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
