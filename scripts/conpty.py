"""Pin, fetch and install Microsoft's ConPTY binaries (Windows x64).

The ConPTY pair (``conpty.dll`` + ``OpenConsole.exe``) always travels together:
both files are extracted from the same ``Microsoft.Windows.Console.ConPTY``
NuGet package and installed side by side in the same directory.

Stdlib only. Unit tests inject a fake ``download`` callable and never touch
the network.
"""

from __future__ import annotations

import argparse
import hashlib
import http.client
import io
import os
import shutil
import urllib.request
import zipfile
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path

CONPTY_VERSION: str = "1.25.260930003"
NUPKG_SHA256: str = (
    "02b07b349af66d801159bdf9e440d4a1ce78bb951f37fc8609731665afdae7ee"
)
NUPKG_URL: str = (
    "https://www.nuget.org/api/v2/package/"
    "Microsoft.Windows.Console.ConPTY/{version}"
)


@dataclass(frozen=True)
class ConptyFile:
    name: str
    member: str
    sha256: str


FILES: tuple[ConptyFile, ...] = (
    ConptyFile(
        name="conpty.dll",
        member="runtimes/win-x64/native/conpty.dll",
        sha256=(
            "feeef341d891643c62d30b6b07800bc70f0bc148f44cb8c3bee84aa557ae805a"
        ),
    ),
    ConptyFile(
        name="OpenConsole.exe",
        member="build/native/runtimes/x64/OpenConsole.exe",
        sha256=(
            "3d66b23d0a71bb8eed2b77edc8b9df9bf54ce6c8fb74c863a30e760997f80586"
        ),
    ),
)

DEFAULT_CACHE_DIR: Path = Path.home() / ".cache" / "orzma" / "conpty"


def _sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _file_digest(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(65536), b""):
            digest.update(chunk)
    return digest.hexdigest()


def download_url(url: str) -> bytes:
    """Download ``url`` and return the response body."""
    with urllib.request.urlopen(url, timeout=30) as response:
        return response.read()


def fetch(
    cache_dir: Path,
    *,
    version: str = CONPTY_VERSION,
    nupkg_sha256: str = NUPKG_SHA256,
    files: tuple[ConptyFile, ...] = FILES,
    download: Callable[[str], bytes] = download_url,
) -> Path:
    """Fetch the ConPTY pair into ``cache_dir / version`` and return that dir.

    Downloads the package only when a cached file is missing or its digest
    differs from its pin. Exits nonzero on a failed download or any digest
    mismatch, without leaving a partially written file behind.
    """
    dest = cache_dir / version
    if all(
        (dest / entry.name).is_file()
        and _file_digest(dest / entry.name) == entry.sha256
        for entry in files
    ):
        return dest

    try:
        data = download(NUPKG_URL.format(version=version))
    except (OSError, http.client.HTTPException) as exc:
        raise SystemExit(
            f"conpty: failed to download ConPTY {version}: {exc}; "
            "run `python scripts/conpty.py install --dest target/debug` "
            "once the network is available; "
            "panes use an older ConPTY until it succeeds"
        ) from exc

    actual_package = _sha256(data)
    if actual_package != nupkg_sha256:
        raise SystemExit(
            "conpty: package sha256 mismatch: "
            f"expected {nupkg_sha256}, got {actual_package}"
        )

    with zipfile.ZipFile(io.BytesIO(data)) as archive:
        try:
            members = {entry.member: archive.read(entry.member) for entry in files}
        except KeyError as exc:
            raise SystemExit(
                f"conpty: package is missing member {exc}"
            ) from exc
    for entry in files:
        actual_member = _sha256(members[entry.member])
        if actual_member != entry.sha256:
            raise SystemExit(
                f"conpty: {entry.name} sha256 mismatch: "
                f"expected {entry.sha256}, got {actual_member}"
            )

    dest.mkdir(parents=True, exist_ok=True)
    for entry in files:
        tmp = dest / (entry.name + ".tmp")
        tmp.write_bytes(members[entry.member])
        os.replace(tmp, dest / entry.name)
    return dest


def install(
    src_dir: Path, dest_dir: Path, files: tuple[ConptyFile, ...] = FILES
) -> list[str]:
    """Copy the ConPTY pair from ``src_dir`` into ``dest_dir``.

    Checks every source file's digest before copying any. Returns the names
    it copied, in ``files`` order.
    """
    for entry in files:
        if not (src_dir / entry.name).is_file():
            raise SystemExit(f"conpty: {entry.name} is missing from {src_dir}")
        actual = _file_digest(src_dir / entry.name)
        if actual != entry.sha256:
            raise SystemExit(
                f"conpty: {entry.name} sha256 mismatch: "
                f"expected {entry.sha256}, got {actual}"
            )
    dest_dir.mkdir(parents=True, exist_ok=True)
    copied: list[str] = []
    for entry in files:
        target = dest_dir / entry.name
        if target.is_file() and _file_digest(target) == entry.sha256:
            continue
        shutil.copy2(src_dir / entry.name, target)
        copied.append(entry.name)
    return copied


def main(argv: list[str] | None = None) -> None:
    parser = argparse.ArgumentParser(
        description="Fetch and install Microsoft's ConPTY binaries (Windows x64)."
    )
    sub = parser.add_subparsers(dest="command", required=True)

    fetch_parser = sub.add_parser(
        "fetch", help="Download the ConPTY pair into the cache."
    )
    fetch_parser.add_argument(
        "--cache-dir", type=Path, default=DEFAULT_CACHE_DIR
    )

    install_parser = sub.add_parser(
        "install", help="Fetch the ConPTY pair and copy it into --dest."
    )
    install_parser.add_argument("--dest", type=Path, required=True)
    install_parser.add_argument(
        "--cache-dir", type=Path, default=DEFAULT_CACHE_DIR
    )

    args = parser.parse_args(argv)
    if args.command == "fetch":
        print(fetch(args.cache_dir))
    elif args.command == "install":
        for name in install(fetch(args.cache_dir), args.dest):
            print(name)


if __name__ == "__main__":
    main()
