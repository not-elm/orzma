#!/usr/bin/env python3
"""Release-gate checks that release.yml and post-release.yml run between their gh calls."""

from __future__ import annotations

import argparse
import json
import re
import sys
import tomllib
from pathlib import Path

from stage_windows import locked_version

TAG_PATTERN = re.compile(r"v(\d+\.\d+\.\d+)")
LOCKED_PACKAGES = ("orzma", "ratatui_orzma")


def tag_version(tag: str) -> str | None:
    """The X.Y.Z of a vX.Y.Z tag, or None for any other tag."""
    match = TAG_PATTERN.fullmatch(tag)
    return match.group(1) if match else None


def read_versions(root: Path) -> dict[str, str | None]:
    """Every place a release version is written, keyed by a label; None marks a missing entry."""
    cargo = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))
    package_json = json.loads(
        (root / "sdk" / "orzma-web" / "package.json").read_text(encoding="utf-8")
    )
    versions: dict[str, str | None] = {
        "VERSION": (root / "VERSION").read_text(encoding="utf-8").strip(),
        "Cargo.toml [workspace.package]": cargo.get("workspace", {})
        .get("package", {})
        .get("version"),
        "sdk/orzma-web/package.json": package_json.get("version"),
    }
    for package in LOCKED_PACKAGES:
        versions[f"Cargo.lock {package}"] = locked_version(package, root / "Cargo.lock")
    return versions


def version_problems(tag: str, versions: dict[str, str | None]) -> list[str]:
    """One message per version source that disagrees with the tag."""
    expected = tag_version(tag)
    if expected is None:
        return [f"Tag {tag} is not vMAJOR.MINOR.PATCH."]
    return [
        f"{label} is {value or 'missing'}, but the tag is {tag}."
        for label, value in versions.items()
        if value != expected
    ]


def build_arg_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)

    versions = commands.add_parser(
        "versions", help="print the tag's version when every version source agrees with it"
    )
    versions.add_argument("--tag", required=True)
    versions.add_argument("--root", type=Path, default=Path("."))
    versions.set_defaults(handler=_versions_command)

    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_arg_parser().parse_args(argv)
    problems, result = args.handler(args)
    for problem in problems:
        print(problem, file=sys.stderr)
    if problems:
        return 1
    if result is not None:
        print(result)
    return 0


def _versions_command(args: argparse.Namespace) -> tuple[list[str], str | None]:
    return version_problems(args.tag, read_versions(args.root)), tag_version(args.tag)


if __name__ == "__main__":
    sys.exit(main())
