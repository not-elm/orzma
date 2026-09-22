from __future__ import annotations

import json
import os
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import stage_windows as sw


def _write(path: Path, data: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)


class CefInventory(unittest.TestCase):
    def _fake_cef_dir(self, root: Path) -> Path:
        cef = root / "cef"
        _write(cef / "libcef.dll", b"cef")
        _write(cef / "icudtl.dat", b"icu")
        _write(cef / "locales" / "ja.pak", b"ja")
        _write(cef / "libcef.lib", b"lib")
        _write(cef / "include" / "cef_app.h", b"header")
        return cef

    def test_iter_cef_files_skips_build_only_entries(self):
        with tempfile.TemporaryDirectory() as tmp:
            cef = self._fake_cef_dir(Path(tmp))
            entries = sw.iter_cef_files(cef, {"include", "libcef.lib"})
            self.assertEqual(entries, ["icudtl.dat", "libcef.dll", "locales/ja.pak"])

    def test_classify_entries_reports_missing_required(self):
        staged, missing, unclassified = sw.classify_entries(
            ["libcef.dll"], {"libcef.dll": "a", "icudtl.dat": "b"}, {}
        )
        self.assertEqual(staged, ["libcef.dll"])
        self.assertEqual(missing, ["icudtl.dat"])
        self.assertEqual(unclassified, [])

    def test_classify_entries_reports_unclassified(self):
        staged, missing, unclassified = sw.classify_entries(
            ["libcef.dll", "brand_new.dll"], {"libcef.dll": "a"}, {}
        )
        self.assertEqual(staged, ["libcef.dll"])
        self.assertEqual(missing, [])
        self.assertEqual(unclassified, ["brand_new.dll"])

    def test_classify_entries_accepts_optional(self):
        staged, missing, unclassified = sw.classify_entries(
            ["libcef.dll", "vulkan-1.dll"], {"libcef.dll": "a"}, {"vulkan-1.dll": "b"}
        )
        self.assertEqual(staged, ["libcef.dll", "vulkan-1.dll"])
        self.assertEqual(unclassified, [])

    def test_digest_mismatches_detects_changed_content(self):
        with tempfile.TemporaryDirectory() as tmp:
            cef = self._fake_cef_dir(Path(tmp))
            good = sw.sha256_file(cef / "libcef.dll")
            self.assertEqual(sw.digest_mismatches(cef, ["libcef.dll"], {"libcef.dll": good}), [])
            self.assertEqual(
                sw.digest_mismatches(cef, ["libcef.dll"], {"libcef.dll": "0" * 64}),
                ["libcef.dll"],
            )

    def test_build_inventory_records_digests_and_version(self):
        with tempfile.TemporaryDirectory() as tmp:
            cef = self._fake_cef_dir(Path(tmp))
            inv = sw.build_inventory(cef, "152.4.0+152.0.8", {"include", "libcef.lib"}, {"locales/ja.pak"})
            self.assertEqual(inv["cef_version"], "152.4.0+152.0.8")
            self.assertEqual(sorted(inv["required"]), ["icudtl.dat", "libcef.dll"])
            self.assertEqual(sorted(inv["optional"]), ["locales/ja.pak"])
            self.assertEqual(inv["required"]["libcef.dll"], sw.sha256_file(cef / "libcef.dll"))
            self.assertEqual(sorted(inv["build_only"]), ["include", "libcef.lib"])

    def test_build_inventory_keeps_optional_names_absent_from_the_host(self):
        with tempfile.TemporaryDirectory() as tmp:
            cef = self._fake_cef_dir(Path(tmp))
            inv = sw.build_inventory(cef, "152.4.0+152.0.8", set(), {"vulkan-1.dll"})
            self.assertIn("vulkan-1.dll", inv["optional"])
            self.assertIsNone(inv["optional"]["vulkan-1.dll"])

    def test_classify_entries_accepts_an_optional_entry_without_a_digest(self):
        staged, missing, unclassified = sw.classify_entries(
            ["vulkan-1.dll"], {}, {"vulkan-1.dll": None}
        )
        self.assertEqual(staged, ["vulkan-1.dll"])
        self.assertEqual(unclassified, [])


class CommittedInventory(unittest.TestCase):
    def test_inventory_has_expected_shape(self):
        inv = sw.load_inventory(sw.INVENTORY_PATH)
        self.assertIn("cef_version", inv)
        for key in ("required", "optional"):
            self.assertIsInstance(inv[key], dict)
        self.assertIsInstance(inv["build_only"], list)
        self.assertIn("libcef.dll", inv["required"])
        self.assertTrue(any(name.startswith("locales/") for name in inv["required"]))
        for digest in inv["required"].values():
            self.assertEqual(len(digest), 64)


if __name__ == "__main__":
    unittest.main()
