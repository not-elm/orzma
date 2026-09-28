from __future__ import annotations

import io
import json
import os
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import release_checks as rc

REPO_ROOT = Path(__file__).resolve().parent.parent


def _write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def _make_repo(root: Path, version: str = "0.2.2", lock_version: str | None = None) -> None:
    locked = lock_version or version
    _write(root / "VERSION", f"{version}\n")
    _write(
        root / "Cargo.toml",
        f'[workspace]\nmembers = []\n\n[workspace.package]\nversion = "{version}"\n',
    )
    _write(
        root / "sdk" / "orzma-web" / "package.json",
        json.dumps({"name": "@orzma/web", "version": version}) + "\n",
    )
    _write(
        root / "Cargo.lock",
        "version = 4\n\n"
        f'[[package]]\nname = "orzma"\nversion = "{locked}"\n\n'
        f'[[package]]\nname = "ratatui_orzma"\nversion = "{locked}"\n',
    )


def _run(argv: list[str], stdin: str = "") -> tuple[int, str, str]:
    out, err = io.StringIO(), io.StringIO()
    saved_stdin = sys.stdin
    sys.stdin = io.StringIO(stdin)
    try:
        with redirect_stdout(out), redirect_stderr(err):
            code = rc.main(argv)
    finally:
        sys.stdin = saved_stdin
    return code, out.getvalue(), err.getvalue()


class TagVersion(unittest.TestCase):
    def test_three_part_tag_yields_its_version(self):
        self.assertEqual(rc.tag_version("v0.2.2"), "0.2.2")

    def test_prerelease_tag_is_rejected(self):
        self.assertIsNone(rc.tag_version("v0.3.0-rc.1"))

    def test_four_part_tag_is_rejected(self):
        self.assertIsNone(rc.tag_version("v0.2.2.1"))

    def test_tag_without_v_is_rejected(self):
        self.assertIsNone(rc.tag_version("0.2.2"))


class Versions(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)

    def tearDown(self):
        self.tmp.cleanup()

    def test_agreeing_sources_print_the_version(self):
        _make_repo(self.root)
        result = _run(["versions", "--tag", "v0.2.2", "--root", str(self.root)])
        self.assertEqual(result, (0, "0.2.2\n", ""))

    def test_stale_lockfile_is_reported(self):
        _make_repo(self.root, lock_version="0.2.1")
        problems = rc.version_problems("v0.2.2", rc.read_versions(self.root))
        self.assertEqual(
            problems,
            [
                "Cargo.lock orzma is 0.2.1, but the tag is v0.2.2.",
                "Cargo.lock ratatui_orzma is 0.2.1, but the tag is v0.2.2.",
            ],
        )

    def test_missing_lock_entry_is_reported(self):
        _make_repo(self.root)
        _write(
            self.root / "Cargo.lock",
            'version = 4\n\n[[package]]\nname = "orzma"\nversion = "0.2.2"\n',
        )
        problems = rc.version_problems("v0.2.2", rc.read_versions(self.root))
        self.assertEqual(problems, ["Cargo.lock ratatui_orzma is missing, but the tag is v0.2.2."])

    def test_version_file_mismatch_fails_the_command(self):
        _make_repo(self.root)
        _write(self.root / "VERSION", "0.2.1\n")
        code, out, err = _run(["versions", "--tag", "v0.2.2", "--root", str(self.root)])
        self.assertEqual((code, out), (1, ""))
        self.assertIn("VERSION is 0.2.1, but the tag is v0.2.2.", err)

    def test_prerelease_tag_is_reported_without_comparing(self):
        self.assertEqual(
            rc.version_problems("v0.3.0-rc.1", {"VERSION": "0.3.0-rc.1"}),
            ["Tag v0.3.0-rc.1 is not vMAJOR.MINOR.PATCH."],
        )

    def test_committed_version_sources_agree(self):
        versions = rc.read_versions(REPO_ROOT)
        self.assertEqual(len(set(versions.values())), 1, versions)


if __name__ == "__main__":
    unittest.main()
