from __future__ import annotations

import json
import os
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import stage_linux as sl

LINUX_ARCHIVE_NAME = (
    "cef_binary_152.0.8+g1ce985c+chromium-152.0.7977.134_linux64_minimal.tar.bz2"
)
WINDOWS_ARCHIVE_NAME = (
    "cef_binary_152.0.8+g1ce985c+chromium-152.0.7977.134_windows64_minimal.tar.bz2"
)
OLD_LINUX_ARCHIVE_NAME = (
    "cef_binary_145.0.27+g4ddda2e+chromium-145.0.7632.117_linux64_minimal.tar.bz2"
)

READELF_WITH_RUNPATH = """
Dynamic section at offset 0x2d8 contains 30 entries:
  Tag        Type                         Name/Value
 0x0000000000000001 (NEEDED)             Shared library: [libcef.so]
 0x000000000000001d (RUNPATH)            Library runpath: [$ORIGIN]
"""
READELF_WITH_RPATH = """
 0x0000000000000001 (NEEDED)             Shared library: [libcef.so]
 0x000000000000000f (RPATH)              Library rpath: [/opt/lib:$ORIGIN]
"""
READELF_WITHOUT_RUNPATH = """
 0x0000000000000001 (NEEDED)             Shared library: [libcef.so]
"""
LDD_OUTPUT = """
\tlinux-vdso.so.1 (0x00007ffc5a1f0000)
\tlibcef.so => /tmp/stage/libcef.so (0x00007f0000000000)
\tlibnss3.so => not found
\tlibgtk-3.so.0 => not found
\tlibnss3.so => not found
"""


def _write(path: Path, data: bytes = b"") -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)


def _write_archive_json(cef_dir: Path, name: str) -> None:
    _write(cef_dir / "archive.json", json.dumps({"type": "minimal", "name": name}).encode())


def _fake_cef_dir(root: Path) -> Path:
    cef = root / "cef"
    for name in (
        "libcef.so", "libEGL.so", "icudtl.dat", "resources.pak",
        "v8_context_snapshot.bin", "chrome-sandbox", "CMakeLists.txt", "CREDITS.html",
    ):
        _write(cef / name, name.encode())
    _write_archive_json(cef, LINUX_ARCHIVE_NAME)
    _write(cef / "locales" / "ja.pak", b"ja")
    _write(cef / "include" / "cef_app.h", b"header")
    _write(cef / "libcef_dll" / "wrapper.cc", b"src")
    _write(cef / "cmake" / "cef.cmake", b"cmake")
    return cef


class CefBuildMeta(unittest.TestCase):
    def test_build_meta_is_the_part_after_plus(self):
        self.assertEqual(sl.cef_build_meta("152.4.0+152.0.8"), "152.0.8")

    def test_version_without_build_meta_is_rejected(self):
        with self.assertRaises(SystemExit):
            sl.cef_build_meta("152.4.0")

    def test_missing_lock_entry_is_rejected(self):
        with self.assertRaises(SystemExit):
            sl.cef_build_meta(None)

    def test_runtime_dir_joins_meta_and_platform(self):
        self.assertEqual(
            sl.cef_runtime_dir(Path("/c"), "152.0.8"),
            Path("/c/152.0.8/cef_linux_x86_64"),
        )

    def test_committed_lockfile_pins_cef_dll_sys_with_build_meta(self):
        locked = sl.locked_version(sl.CEF_SYS_CRATE)
        self.assertIsNotNone(locked)
        self.assertIn("+", locked)


class CefArchive(unittest.TestCase):
    def test_linux_archive_yields_build_meta(self):
        text = json.dumps({"name": LINUX_ARCHIVE_NAME})
        self.assertEqual(sl.archive_build_meta(text), "152.0.8")

    def test_other_platform_is_not_recognised(self):
        self.assertIsNone(sl.archive_build_meta(json.dumps({"name": WINDOWS_ARCHIVE_NAME})))

    def test_malformed_json_is_not_recognised(self):
        self.assertIsNone(sl.archive_build_meta("{not json"))
        self.assertIsNone(sl.archive_build_meta(json.dumps(["a"])))

    def test_verify_accepts_matching_archive(self):
        with tempfile.TemporaryDirectory() as tmp:
            cef = _fake_cef_dir(Path(tmp))
            sl.verify_cef_archive(cef, "152.0.8")

    def test_verify_rejects_older_cef(self):
        with tempfile.TemporaryDirectory() as tmp:
            cef = _fake_cef_dir(Path(tmp))
            _write_archive_json(cef, OLD_LINUX_ARCHIVE_NAME)
            with self.assertRaises(SystemExit) as ctx:
                sl.verify_cef_archive(cef, "152.0.8")
            self.assertIn("152.0.8", str(ctx.exception))

    def test_verify_rejects_missing_archive_json(self):
        with tempfile.TemporaryDirectory() as tmp:
            cef = _fake_cef_dir(Path(tmp))
            (cef / "archive.json").unlink()
            with self.assertRaises(SystemExit):
                sl.verify_cef_archive(cef, "152.0.8")


class CefRuntime(unittest.TestCase):
    def test_select_skips_build_only_and_sandbox_entries(self):
        with tempfile.TemporaryDirectory() as tmp:
            cef = _fake_cef_dir(Path(tmp))
            names = [p.name for p in sl.select_cef_entries(cef)]
            self.assertEqual(
                names,
                ["icudtl.dat", "libEGL.so", "libcef.so", "locales",
                 "resources.pak", "v8_context_snapshot.bin"],
            )

    def test_copy_stages_selected_entries_with_locales(self):
        with tempfile.TemporaryDirectory() as tmp:
            cef = _fake_cef_dir(Path(tmp))
            tree = Path(tmp) / "tree"
            tree.mkdir()
            staged = sl.copy_cef_runtime(cef, tree)
            self.assertIn("locales", staged)
            self.assertTrue((tree / "locales" / "ja.pak").is_file())
            self.assertTrue((tree / "libcef.so").is_file())
            for excluded in ("chrome-sandbox", "CREDITS.html", "archive.json", "include"):
                self.assertFalse((tree / excluded).exists(), excluded)

    def test_copy_rejects_missing_required_entry(self):
        with tempfile.TemporaryDirectory() as tmp:
            cef = _fake_cef_dir(Path(tmp))
            (cef / "icudtl.dat").unlink()
            tree = Path(tmp) / "tree"
            tree.mkdir()
            with self.assertRaises(SystemExit) as ctx:
                sl.copy_cef_runtime(cef, tree)
            self.assertIn("icudtl.dat", str(ctx.exception))

    def test_stage_cef_uses_the_lockfile_build_meta_dir(self):
        with tempfile.TemporaryDirectory() as tmp:
            meta = sl.cef_build_meta(sl.locked_version(sl.CEF_SYS_CRATE))
            cef_path = Path(tmp) / "cache"
            fake = _fake_cef_dir(Path(tmp))
            runtime = sl.cef_runtime_dir(cef_path, meta)
            runtime.parent.mkdir(parents=True)
            fake.rename(runtime)
            _write_archive_json(runtime, LINUX_ARCHIVE_NAME.replace("152.0.8", meta))
            tree = Path(tmp) / "tree"
            tree.mkdir()
            sl.stage_cef(cef_path, tree)
            self.assertTrue((tree / "libcef.so").is_file())

    def test_stage_cef_fails_without_downloaded_runtime(self):
        with tempfile.TemporaryDirectory() as tmp:
            tree = Path(tmp) / "tree"
            tree.mkdir()
            with self.assertRaises(SystemExit) as ctx:
                sl.stage_cef(Path(tmp) / "empty-cache", tree)
            self.assertIn("CEF runtime not found", str(ctx.exception))


class ElfChecks(unittest.TestCase):
    def test_runpath_is_parsed(self):
        self.assertEqual(sl.runpath_entries(READELF_WITH_RUNPATH), ["$ORIGIN"])

    def test_legacy_rpath_is_parsed_and_split(self):
        self.assertEqual(sl.runpath_entries(READELF_WITH_RPATH), ["/opt/lib", "$ORIGIN"])

    def test_missing_runpath_yields_nothing(self):
        self.assertEqual(sl.runpath_entries(READELF_WITHOUT_RUNPATH), [])

    def test_unresolved_libraries_are_sorted_and_unique(self):
        self.assertEqual(sl.unresolved_libraries(LDD_OUTPUT), ["libgtk-3.so.0", "libnss3.so"])

    def test_fully_resolved_ldd_output_yields_nothing(self):
        self.assertEqual(sl.unresolved_libraries("\tlibc.so.6 => /lib/libc.so.6 (0x1)\n"), [])


if __name__ == "__main__":
    unittest.main()
