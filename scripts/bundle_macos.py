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
