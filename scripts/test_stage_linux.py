from __future__ import annotations

import json
import os
import stat
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path
from unittest import mock

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
    _write(cef / "locales" / "en-US.pak", b"en-US")
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

    def test_copy_keeps_only_the_linux_locale_packs(self):
        with tempfile.TemporaryDirectory() as tmp:
            cef = _fake_cef_dir(Path(tmp))
            for name in ("fr.pak", "ja_FEMININE.pak", "en-GB.pak"):
                _write(cef / "locales" / name, name.encode())
            tree = Path(tmp) / "tree"
            tree.mkdir()
            sl.copy_cef_runtime(cef, tree)
            self.assertEqual(
                sorted(p.name for p in (tree / "locales").iterdir()), ["en-US.pak", "ja.pak"]
            )

    def test_copy_rejects_a_missing_locale_pack_before_copying(self):
        with tempfile.TemporaryDirectory() as tmp:
            cef = _fake_cef_dir(Path(tmp))
            (cef / "locales" / "en-US.pak").unlink()
            tree = Path(tmp) / "tree"
            tree.mkdir()
            with self.assertRaises(SystemExit) as ctx:
                sl.copy_cef_runtime(cef, tree)
            self.assertIn("en-US", str(ctx.exception))
            self.assertEqual(list(tree.iterdir()), [])

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

    def test_capture_runs_with_a_c_locale(self):
        with mock.patch.object(sl.subprocess, "run") as run:
            run.return_value = mock.Mock(stdout="output")
            sl._capture(["readelf", "-d", "/tmp/bin"])
            self.assertEqual(run.call_args.kwargs["env"]["LC_ALL"], "C")


def _make_linux_dir(root: Path) -> Path:
    linux = root / "linux"
    _write(linux / "install.sh", b"#!/bin/sh\n")
    _write(linux / "uninstall.sh", b"#!/bin/sh\n")
    _write(linux / "orzma.desktop", b"[Desktop Entry]\n")
    for size in sl.ICON_SIZES:
        _write(sl.icon_path(linux / "icons", size), f"png{size}".encode())
    return linux


def _make_tree(root: Path, version: str = "0.2.0") -> Path:
    tree = root / "stage" / sl.dist_name(version)
    _write(tree / "orzma", b"bin")
    (tree / "orzma").chmod(0o700)
    _write(tree / "LICENSE", b"mit")
    (tree / "LICENSE").chmod(0o600)
    _write(tree / "locales" / "ja.pak", b"ja")
    return tree


class Naming(unittest.TestCase):
    def test_dist_name_carries_version_arch_and_os(self):
        self.assertEqual(sl.dist_name("0.2.0"), "orzma-0.2.0-x86_64-linux")


class CargoArgv(unittest.TestCase):
    def test_main_build_targets_orzma_and_render_process(self):
        self.assertEqual(
            sl.cargo_build_argv("x86_64-unknown-linux-gnu", "dist"),
            ["cargo", "build", "--profile", "dist", "--target", "x86_64-unknown-linux-gnu",
             "--locked", "--no-default-features", "-p", "orzma", "-p", "cef_render_process"],
        )

    def test_companion_build_keeps_default_features(self):
        self.assertEqual(
            sl.companion_cargo_build_argv("x86_64-unknown-linux-gnu", "dist", ("orzbrowser", "orzmd")),
            ["cargo", "build", "--profile", "dist", "--target", "x86_64-unknown-linux-gnu",
             "--locked", "-p", "orzbrowser", "-p", "orzmd"],
        )


class StageFiles(unittest.TestCase):
    def test_stage_binaries_copies_all_four(self):
        with tempfile.TemporaryDirectory() as tmp:
            built, tree = Path(tmp) / "built", Path(tmp) / "tree"
            tree.mkdir()
            for name in ("orzma", "orzmd", "orzbrowser", "bevy_cef_render_process"):
                _write(built / name, name.encode())
            sl.stage_binaries(built, tree)
            self.assertEqual(
                sorted(p.name for p in tree.iterdir()),
                ["bevy_cef_render_process", "orzbrowser", "orzma", "orzmd"],
            )

    def test_stage_binaries_rejects_missing_binary(self):
        with tempfile.TemporaryDirectory() as tmp:
            built, tree = Path(tmp) / "built", Path(tmp) / "tree"
            tree.mkdir()
            _write(built / "orzma", b"bin")
            with self.assertRaises(SystemExit) as ctx:
                sl.stage_binaries(built, tree)
            self.assertIn("binary not found", str(ctx.exception))

    def test_stage_licenses_places_chromium_credits_in_a_subdir(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo, tree = Path(tmp) / "repo", Path(tmp) / "tree"
            tree.mkdir()
            _write(repo / "LICENSE", b"mit")
            _write(repo / "licenses" / "THIRD-PARTY-LICENSES.md", b"3p")
            _write(repo / "licenses" / "chromium" / "CREDITS.html", b"credits")
            sl.stage_licenses(repo, tree)
            self.assertEqual((tree / "LICENSE").read_bytes(), b"mit")
            self.assertEqual((tree / "THIRD-PARTY-LICENSES.md").read_bytes(), b"3p")
            self.assertEqual((tree / "chromium" / "CREDITS.html").read_bytes(), b"credits")

    def test_stage_desktop_integration_lays_out_share_and_scripts(self):
        with tempfile.TemporaryDirectory() as tmp:
            linux, tree = _make_linux_dir(Path(tmp)), Path(tmp) / "tree"
            tree.mkdir()
            sl.stage_desktop_integration(linux, tree)
            for script in ("install.sh", "uninstall.sh"):
                self.assertEqual(stat.S_IMODE((tree / script).stat().st_mode), 0o755)
            self.assertTrue((tree / "share" / "applications" / "orzma.desktop").is_file())
            for size in sl.ICON_SIZES:
                icon = tree / "share" / "icons" / "hicolor" / f"{size}x{size}" / "apps" / "orzma.png"
                self.assertEqual(icon.read_bytes(), f"png{size}".encode())

    def test_stage_desktop_integration_rejects_missing_icon(self):
        with tempfile.TemporaryDirectory() as tmp:
            linux, tree = _make_linux_dir(Path(tmp)), Path(tmp) / "tree"
            tree.mkdir()
            sl.icon_path(linux / "icons", 256).unlink()
            with self.assertRaises(SystemExit) as ctx:
                sl.stage_desktop_integration(linux, tree)
            self.assertIn("orzma-256.png", str(ctx.exception))


class DebugInfo(unittest.TestCase):
    def test_shared_objects_include_versioned_sonames_only_at_top_level(self):
        with tempfile.TemporaryDirectory() as tmp:
            tree = Path(tmp)
            for name in ("libcef.so", "libvulkan.so.1", "orzma", "icudtl.dat"):
                _write(tree / name, b"x")
            _write(tree / "locales" / "ja.pak", b"ja")
            self.assertEqual(
                [p.name for p in sl.shared_objects(tree)], ["libcef.so", "libvulkan.so.1"]
            )

    def test_strip_argv_strips_debug_info_only(self):
        self.assertEqual(
            sl.strip_argv([Path("/t/libcef.so"), Path("/t/libEGL.so")]),
            ["strip", "--strip-debug", "/t/libcef.so", "/t/libEGL.so"],
        )


class Archive(unittest.TestCase):
    def test_modes_are_normalized(self):
        with tempfile.TemporaryDirectory() as tmp:
            tree = _make_tree(Path(tmp))
            self.assertEqual(sl.normalized_mode(tree / "orzma"), 0o755)
            self.assertEqual(sl.normalized_mode(tree / "LICENSE"), 0o644)
            self.assertEqual(sl.normalized_mode(tree / "locales"), 0o755)

    def test_archive_is_byte_identical_across_mtimes_and_umasks(self):
        with tempfile.TemporaryDirectory() as tmp:
            tree = _make_tree(Path(tmp))
            first, second = Path(tmp) / "a.tar.gz", Path(tmp) / "b.tar.gz"
            sl.write_archive(tree, first)
            for path in tree.rglob("*"):
                os.utime(path, (1_700_000_000, 1_700_000_000))
            (tree / "LICENSE").chmod(0o640)
            sl.write_archive(tree, second)
            self.assertEqual(first.read_bytes(), second.read_bytes())

    def test_gzip_header_carries_no_time_or_name(self):
        with tempfile.TemporaryDirectory() as tmp:
            tree = _make_tree(Path(tmp))
            out = Path(tmp) / "a.tar.gz"
            sl.write_archive(tree, out)
            header = out.read_bytes()[:10]
            self.assertEqual(header[4:8], b"\0\0\0\0")
            self.assertEqual(header[3] & 0x08, 0)

    def test_entries_are_rooted_sorted_and_anonymous(self):
        with tempfile.TemporaryDirectory() as tmp:
            tree = _make_tree(Path(tmp))
            out = Path(tmp) / "a.tar.gz"
            sl.write_archive(tree, out)
            with tarfile.open(out, "r:gz") as tar:
                members = tar.getmembers()
            root = sl.dist_name("0.2.0")
            self.assertEqual(
                [m.name for m in members],
                [root, f"{root}/LICENSE", f"{root}/locales", f"{root}/locales/ja.pak", f"{root}/orzma"],
            )
            by_name = {m.name: m for m in members}
            self.assertEqual(by_name[f"{root}/orzma"].mode, 0o755)
            self.assertEqual(by_name[f"{root}/LICENSE"].mode, 0o644)
            for member in members:
                self.assertEqual((member.mtime, member.uid, member.gid, member.uname, member.gname),
                                 (0, 0, 0, "", ""))

    def test_sidecar_matches_sha256sum_format(self):
        with tempfile.TemporaryDirectory() as tmp:
            archive = Path(tmp) / "orzma-0.2.0-x86_64-linux.tar.gz"
            archive.write_bytes(b"payload")
            sidecar = sl.write_sidecar(archive)
            self.assertEqual(sidecar.name, "orzma-0.2.0-x86_64-linux.tar.gz.sha256")
            self.assertEqual(
                sidecar.read_text(encoding="ascii"),
                f"{sl.sha256_file(archive)}  orzma-0.2.0-x86_64-linux.tar.gz\n",
            )

    def test_package_requires_stage_tree(self):
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaises(SystemExit) as ctx:
                sl.package(Path(tmp) / "stage", "0.2.0", Path(tmp))
            self.assertIn("just stage", str(ctx.exception))

    def test_package_writes_archive_and_sidecar(self):
        with tempfile.TemporaryDirectory() as tmp:
            _make_tree(Path(tmp))
            archive = sl.package(Path(tmp) / "stage", "0.2.0", Path(tmp))
            self.assertEqual(archive, Path(tmp) / "orzma-0.2.0-x86_64-linux.tar.gz")
            self.assertTrue(archive.is_file())
            self.assertTrue(Path(f"{archive}.sha256").is_file())


class Cli(unittest.TestCase):
    def test_package_only_does_not_need_cef_path(self):
        with tempfile.TemporaryDirectory() as tmp:
            _make_tree(Path(tmp))
            env = {k: v for k, v in os.environ.items() if k != "CEF_PATH"}
            with mock.patch.dict(os.environ, env, clear=True):
                sl.main(["--package-only", "--version", "0.2.0", "--out-dir", tmp])
            self.assertTrue((Path(tmp) / "orzma-0.2.0-x86_64-linux.tar.gz").is_file())

    def test_staging_requires_cef_path(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = {k: v for k, v in os.environ.items() if k != "CEF_PATH"}
            with mock.patch.dict(os.environ, env, clear=True):
                with self.assertRaises(SystemExit) as ctx:
                    sl.main(["--version", "0.2.0", "--out-dir", tmp, "--skip-build"])
            self.assertIn("CEF_PATH", str(ctx.exception))


if __name__ == "__main__":
    unittest.main()
