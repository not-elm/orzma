#!/usr/bin/env python3
"""Release-gate checks that release.yml and post-release.yml run between their gh calls."""

from __future__ import annotations

import argparse
import json
import re
import sys
import tomllib
from pathlib import Path

from stage_windows import locked_version, sha256_file

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


def expected_assets(version: str) -> list[str]:
    """The eight files every release carries: four packages and their .sha256 sidecars."""
    packages = [
        f"orzma-{version}-arm64.zip",
        f"orzma-{version}-x64.msi",
        f"orzma-{version}-x86_64-linux.tar.gz",
        f"orzma_{version}_amd64.deb",
    ]
    return sorted(packages + [f"{package}.sha256" for package in packages])


def dist_problems(dist: Path, version: str) -> list[str]:
    """Checks that dist holds exactly the expected files and that each sidecar names and hashes its file."""
    expected = set(expected_assets(version))
    present = {path.name for path in dist.iterdir()} if dist.is_dir() else set()
    problems = [f"Missing {name}." for name in sorted(expected - present)]
    problems += [f"Unexpected {name}." for name in sorted(present - expected)]
    for sidecar in sorted(name for name in expected & present if name.endswith(".sha256")):
        package = sidecar.removesuffix(".sha256")
        if package not in present:
            continue
        recorded = (dist / sidecar).read_text(encoding="utf-8")
        if recorded != f"{sha256_file(dist / package)}  {package}\n":
            problems.append(f"{sidecar} does not match {package}.")
    return problems


def uploaded_problems(assets: list[dict], version: str, dist: Path | None) -> list[str]:
    """Checks a release's assets: exactly the expected names, each fully uploaded, and,
    when dist is given, each digest equal to the built file's SHA-256."""
    expected = set(expected_assets(version))
    by_name = {asset["name"]: asset for asset in assets}
    problems = [f"Asset {name} is missing." for name in sorted(expected - by_name.keys())]
    problems += [f"Asset {name} is unexpected." for name in sorted(by_name.keys() - expected)]
    for name in sorted(expected & by_name.keys()):
        asset = by_name[name]
        if asset.get("state") != "uploaded":
            problems.append(f"Asset {name} is {asset.get('state')}, not uploaded.")
        elif dist is not None and asset.get("digest") != f"sha256:{sha256_file(dist / name)}":
            problems.append(
                f"Asset {name} has digest {asset.get('digest')}, which does not match the built file."
            )
    return problems


def build_arg_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)

    versions = commands.add_parser(
        "versions", help="print the tag's version when every version source agrees with it"
    )
    versions.add_argument("--tag", required=True)
    versions.add_argument("--root", type=Path, default=Path("."))
    versions.set_defaults(handler=_versions_command)

    dist = commands.add_parser(
        "dist", help="check the downloaded release files and their .sha256 sidecars"
    )
    dist.add_argument("--version", required=True)
    dist.add_argument("--dir", type=Path, required=True)
    dist.set_defaults(handler=_dist_command)

    uploaded = commands.add_parser(
        "uploaded",
        help="check a draft's assets against the built files (reads gh release view --json assets on stdin)",
    )
    uploaded.add_argument("--version", required=True)
    uploaded.add_argument("--dir", type=Path, required=True)
    uploaded.set_defaults(handler=_uploaded_command)

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


def _dist_command(args: argparse.Namespace) -> tuple[list[str], str | None]:
    return dist_problems(args.dir, args.version), None


def _uploaded_command(args: argparse.Namespace) -> tuple[list[str], str | None]:
    release = json.load(sys.stdin)
    return uploaded_problems(release.get("assets", []), args.version, args.dir), None


if __name__ == "__main__":
    sys.exit(main())
