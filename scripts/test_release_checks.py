from __future__ import annotations

import hashlib
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


def _make_dist(dist: Path, version: str = "0.2.2") -> None:
    dist.mkdir(parents=True, exist_ok=True)
    for name in rc.expected_assets(version):
        if name.endswith(".sha256"):
            continue
        (dist / name).write_bytes(name.encode())
        digest = hashlib.sha256(name.encode()).hexdigest()
        (dist / f"{name}.sha256").write_text(f"{digest}  {name}\n", encoding="utf-8")


def _uploaded_assets(dist: Path, version: str = "0.2.2") -> list[dict]:
    return [
        {
            "name": name,
            "state": "uploaded",
            "digest": "sha256:" + hashlib.sha256((dist / name).read_bytes()).hexdigest(),
        }
        for name in rc.expected_assets(version)
    ]


class ExpectedAssets(unittest.TestCase):
    def test_lists_four_packages_and_their_sidecars(self):
        packages = [
            "orzma-0.2.2-arm64.zip",
            "orzma-0.2.2-x64.msi",
            "orzma-0.2.2-x86_64-linux.tar.gz",
            "orzma_0.2.2_amd64.deb",
        ]
        self.assertEqual(
            rc.expected_assets("0.2.2"),
            sorted(packages + [f"{p}.sha256" for p in packages]),
        )


class Dist(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.dist = Path(self.tmp.name) / "dist"
        _make_dist(self.dist)

    def tearDown(self):
        self.tmp.cleanup()

    def test_complete_dist_passes(self):
        self.assertEqual(rc.dist_problems(self.dist, "0.2.2"), [])

    def test_missing_file_is_reported(self):
        (self.dist / "orzma-0.2.2-x64.msi").unlink()
        self.assertEqual(rc.dist_problems(self.dist, "0.2.2"), ["Missing orzma-0.2.2-x64.msi."])

    def test_unexpected_file_is_reported(self):
        (self.dist / "orzma-0.2.1-arm64.zip").write_bytes(b"old")
        self.assertEqual(
            rc.dist_problems(self.dist, "0.2.2"), ["Unexpected orzma-0.2.1-arm64.zip."]
        )

    def test_sidecar_with_wrong_hash_is_reported(self):
        (self.dist / "orzma_0.2.2_amd64.deb").write_bytes(b"rebuilt")
        self.assertEqual(
            rc.dist_problems(self.dist, "0.2.2"),
            ["orzma_0.2.2_amd64.deb.sha256 does not match orzma_0.2.2_amd64.deb."],
        )

    def test_sidecar_naming_another_file_is_reported(self):
        name = "orzma-0.2.2-arm64.zip"
        digest = hashlib.sha256(name.encode()).hexdigest()
        (self.dist / f"{name}.sha256").write_text(f"{digest}  orzma.zip\n", encoding="utf-8")
        self.assertEqual(
            rc.dist_problems(self.dist, "0.2.2"),
            [f"{name}.sha256 does not match {name}."],
        )

    def test_dist_command_fails_on_problems(self):
        (self.dist / "orzma-0.2.2-x64.msi").unlink()
        code, out, err = _run(["dist", "--version", "0.2.2", "--dir", str(self.dist)])
        self.assertEqual((code, out, err), (1, "", "Missing orzma-0.2.2-x64.msi.\n"))


class Uploaded(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.dist = Path(self.tmp.name) / "dist"
        _make_dist(self.dist)
        self.assets = _uploaded_assets(self.dist)

    def tearDown(self):
        self.tmp.cleanup()

    def _asset(self, name: str) -> dict:
        return next(a for a in self.assets if a["name"] == name)

    def test_assets_matching_the_built_files_pass(self):
        self.assertEqual(rc.uploaded_problems(self.assets, "0.2.2", self.dist), [])

    def test_partial_upload_is_reported(self):
        self._asset("orzma_0.2.2_amd64.deb")["state"] = "starter"
        self.assertEqual(
            rc.uploaded_problems(self.assets, "0.2.2", self.dist),
            ["Asset orzma_0.2.2_amd64.deb is starter, not uploaded."],
        )

    def test_digest_mismatch_is_reported(self):
        self._asset("orzma-0.2.2-x64.msi")["digest"] = "sha256:" + "0" * 64
        self.assertEqual(
            rc.uploaded_problems(self.assets, "0.2.2", self.dist),
            [
                "Asset orzma-0.2.2-x64.msi has digest sha256:"
                + "0" * 64
                + ", which does not match the built file."
            ],
        )

    def test_missing_asset_is_reported(self):
        self.assets.remove(self._asset("orzma-0.2.2-arm64.zip.sha256"))
        self.assertEqual(
            rc.uploaded_problems(self.assets, "0.2.2", self.dist),
            ["Asset orzma-0.2.2-arm64.zip.sha256 is missing."],
        )

    def test_unexpected_asset_is_reported(self):
        self.assets.append({"name": "notes.txt", "state": "uploaded", "digest": None})
        self.assertEqual(
            rc.uploaded_problems(self.assets, "0.2.2", self.dist),
            ["Asset notes.txt is unexpected."],
        )

    def test_without_dist_only_names_and_states_are_checked(self):
        for asset in self.assets:
            asset["digest"] = None
        self.assertEqual(rc.uploaded_problems(self.assets, "0.2.2", None), [])

    def test_uploaded_command_reads_gh_json_from_stdin(self):
        result = _run(
            ["uploaded", "--version", "0.2.2", "--dir", str(self.dist)],
            stdin=json.dumps({"assets": self.assets}),
        )
        self.assertEqual(result, (0, "", ""))


if __name__ == "__main__":
    unittest.main()
