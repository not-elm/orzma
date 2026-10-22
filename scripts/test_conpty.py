from __future__ import annotations

import hashlib
import http.client
import os
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import conpty


DLL_MEMBER = "runtimes/win-x64/native/conpty.dll"
EXE_MEMBER = "build/native/runtimes/x64/OpenConsole.exe"


def _package(root: Path) -> tuple[Path, str, tuple[conpty.ConptyFile, ...]]:
    """Write a fake ConPTY nupkg under root.

    Returns the zip path, the zip's SHA-256, and ConptyFiles carrying
    the members' real digests.
    """
    root.mkdir(parents=True, exist_ok=True)
    zip_path = root / "conpty.nupkg"
    with zipfile.ZipFile(zip_path, "w") as zf:
        zf.writestr(DLL_MEMBER, b"dll")
        zf.writestr(EXE_MEMBER, b"exe")
    sha = hashlib.sha256(zip_path.read_bytes()).hexdigest()
    files = (
        conpty.ConptyFile(
            name="conpty.dll",
            member=DLL_MEMBER,
            sha256=hashlib.sha256(b"dll").hexdigest(),
        ),
        conpty.ConptyFile(
            name="OpenConsole.exe",
            member=EXE_MEMBER,
            sha256=hashlib.sha256(b"exe").hexdigest(),
        ),
    )
    return zip_path, sha, files


def _downloader(zip_path: Path):
    """Return a `download` callable that serves the zip's bytes."""

    def download(url: str) -> bytes:
        return zip_path.read_bytes()

    return download


def _sources(root: Path) -> Path:
    """Write source files matching the fake package's pins; return the dir."""
    src = root / "src"
    src.mkdir(parents=True, exist_ok=True)
    (src / "conpty.dll").write_bytes(b"dll")
    (src / "OpenConsole.exe").write_bytes(b"exe")
    return src


class Fetch(unittest.TestCase):
    def test_fetch_extracts_both_files_into_a_version_dir(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            zip_path, sha, files = _package(root)
            cache = root / "cache"
            out = conpty.fetch(
                cache,
                version="9.9.9",
                nupkg_sha256=sha,
                files=files,
                download=_downloader(zip_path),
            )
            self.assertEqual(out, cache / "9.9.9")
            self.assertEqual((out / "conpty.dll").read_bytes(), b"dll")
            self.assertEqual((out / "OpenConsole.exe").read_bytes(), b"exe")

    def test_a_complete_cache_is_reused_without_downloading(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            zip_path, sha, files = _package(root)
            cache = root / "cache"
            conpty.fetch(
                cache,
                version="9.9.9",
                nupkg_sha256=sha,
                files=files,
                download=_downloader(zip_path),
            )

            def raising(url: str) -> bytes:
                raise AssertionError("download must not be called for a complete cache")

            self.assertEqual(
                conpty.fetch(
                    cache,
                    version="9.9.9",
                    nupkg_sha256=sha,
                    files=files,
                    download=raising,
                ),
                cache / "9.9.9",
            )

    def test_a_package_with_the_wrong_digest_is_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            zip_path, _, files = _package(root)
            cache = root / "cache"
            with self.assertRaises(SystemExit) as raised:
                conpty.fetch(
                    cache,
                    version="9.9.9",
                    nupkg_sha256="0" * 64,
                    files=files,
                    download=_downloader(zip_path),
                )
            self.assertIn("sha256", str(raised.exception))
            self.assertFalse((cache / "9.9.9").exists())

    def test_a_member_with_the_wrong_digest_is_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            zip_path, sha, files = _package(root)
            cache = root / "cache"
            bad = (
                files[0],
                conpty.ConptyFile(
                    name=files[1].name, member=files[1].member, sha256="0" * 64
                ),
            )
            with self.assertRaises(SystemExit) as raised:
                conpty.fetch(
                    cache,
                    version="9.9.9",
                    nupkg_sha256=sha,
                    files=bad,
                    download=_downloader(zip_path),
                )
            self.assertIn("OpenConsole.exe", str(raised.exception))
            self.assertFalse((cache / "9.9.9").exists())

    def test_a_corrupted_cached_file_is_downloaded_again(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            zip_path, sha, files = _package(root)
            cache = root / "cache"
            calls: list[str] = []
            data = zip_path.read_bytes()

            def download(url: str) -> bytes:
                calls.append(url)
                return data

            out = conpty.fetch(
                cache,
                version="9.9.9",
                nupkg_sha256=sha,
                files=files,
                download=download,
            )
            (out / "conpty.dll").write_bytes(b"bad")
            self.assertEqual(
                conpty.fetch(
                    cache,
                    version="9.9.9",
                    nupkg_sha256=sha,
                    files=files,
                    download=download,
                ),
                out,
            )
            self.assertEqual(len(calls), 2)
            self.assertEqual((out / "conpty.dll").read_bytes(), b"dll")

    def test_a_failed_download_names_the_install_command(self):
        for error in (OSError("offline"), http.client.IncompleteRead(b"")):
            with self.subTest(error=type(error).__name__):
                with tempfile.TemporaryDirectory() as tmp:
                    root = Path(tmp)
                    _, sha, files = _package(root)
                    cache = root / "cache"

                    def download(url: str, _error=error) -> bytes:
                        raise _error

                    with self.assertRaises(SystemExit) as raised:
                        conpty.fetch(
                            cache,
                            version="9.9.9",
                            nupkg_sha256=sha,
                            files=files,
                            download=download,
                        )
                    self.assertIn("scripts/conpty.py install", str(raised.exception))


class Install(unittest.TestCase):
    def test_install_copies_both_files_into_the_destination(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            _, _, files = _package(root)
            src = _sources(root)
            dest = root / "dest"
            self.assertEqual(
                conpty.install(src, dest, files), ["conpty.dll", "OpenConsole.exe"]
            )
            self.assertEqual((dest / "conpty.dll").read_bytes(), b"dll")
            self.assertEqual((dest / "OpenConsole.exe").read_bytes(), b"exe")

    def test_install_skips_files_that_already_match(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            _, _, files = _package(root)
            src = _sources(root)
            dest = root / "dest"
            conpty.install(src, dest, files)
            self.assertEqual(conpty.install(src, dest, files), [])

    def test_install_rejects_a_source_whose_digest_differs(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            _, _, files = _package(root)
            src = _sources(root)
            (src / "conpty.dll").write_bytes(b"bad")
            with self.assertRaises(SystemExit) as raised:
                conpty.install(src, root / "dest", files)
            self.assertIn("conpty.dll", str(raised.exception))

    def test_install_copies_nothing_when_the_host_is_corrupt(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            _, _, files = _package(root)
            src = _sources(root)
            (src / "OpenConsole.exe").write_bytes(b"bad")
            dest = root / "dest"
            with self.assertRaises(SystemExit) as raised:
                conpty.install(src, dest, files)
            self.assertIn("OpenConsole.exe", str(raised.exception))
            self.assertFalse((dest / "conpty.dll").exists())


class Pins(unittest.TestCase):
    def test_the_pins_name_the_x64_members_of_the_package(self):
        self.assertEqual(conpty.CONPTY_VERSION, "1.25.260930003")
        self.assertEqual(
            [(f.name, f.member) for f in conpty.FILES],
            [
                ("conpty.dll", "runtimes/win-x64/native/conpty.dll"),
                ("OpenConsole.exe", "build/native/runtimes/x64/OpenConsole.exe"),
            ],
        )
        self.assertEqual(
            conpty.NUPKG_URL.format(version=conpty.CONPTY_VERSION),
            "https://www.nuget.org/api/v2/package/Microsoft.Windows.Console.ConPTY/1.25.260930003",
        )


if __name__ == "__main__":
    unittest.main()
