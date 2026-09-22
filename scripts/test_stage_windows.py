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


class PeImports(unittest.TestCase):
    def test_rejects_non_pe_file(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "not-a-pe.bin"
            path.write_bytes(b"this is not a pe image")
            with self.assertRaises(ValueError):
                sw.pe_imported_dlls(path)

    @unittest.skipUnless(sys.platform == "win32", "PE parsing needs a real Windows binary")
    def test_parses_a_system_library(self):
        # NOTE: not sys.executable. A Microsoft Store Python resolves it to an app
        # execution alias whose is_file() is True but whose open() raises OSError 22.
        kernel32 = Path(os.environ.get("SystemRoot", r"C:\Windows")) / "System32" / "kernel32.dll"
        if not kernel32.is_file():
            self.skipTest("kernel32.dll not available")
        names = sw.pe_imported_dlls(kernel32)
        self.assertTrue(names)
        for name in names:
            self.assertTrue(name.endswith(".dll"), name)
            self.assertEqual(name, name.lower())
        self.assertIn("ntdll.dll", names)

    def test_forbidden_crt_imports_flags_vcruntime(self):
        names = ["kernel32.dll", "vcruntime140.dll", "vcruntime140_1.dll", "msvcp140.dll"]
        self.assertEqual(
            sw.forbidden_crt_imports(names),
            ["msvcp140.dll", "vcruntime140.dll", "vcruntime140_1.dll"],
        )

    def test_forbidden_crt_imports_flags_concrt_and_vcomp(self):
        names = ["kernel32.dll", "concrt140.dll", "vcomp140.dll"]
        self.assertEqual(
            sw.forbidden_crt_imports(names),
            ["concrt140.dll", "vcomp140.dll"],
        )

    def test_forbidden_crt_imports_allows_ucrt(self):
        names = ["api-ms-win-crt-runtime-l1-1-0.dll", "kernel32.dll", "ucrtbase.dll"]
        self.assertEqual(sw.forbidden_crt_imports(names), [])


class CargoInvocation(unittest.TestCase):
    def test_cargo_build_argv_disables_default_features(self):
        self.assertEqual(
            sw.cargo_build_argv("x86_64-pc-windows-msvc", "dist"),
            ["cargo", "build", "--profile", "dist", "--target", "x86_64-pc-windows-msvc",
             "--locked", "--no-default-features"],
        )

    def test_companion_cargo_build_argv_lists_packages(self):
        self.assertEqual(
            sw.companion_cargo_build_argv("x86_64-pc-windows-msvc", "dist", ("orzbrowser", "orzmd")),
            ["cargo", "build", "--profile", "dist", "--target", "x86_64-pc-windows-msvc",
             "--locked", "-p", "orzbrowser", "-p", "orzmd"],
        )

    def test_render_process_install_argv_pins_version_and_target(self):
        argv = sw.render_process_install_argv("0.13.0", "x86_64-pc-windows-msvc", Path("/tmp/tools"))
        self.assertIn("bevy_cef_render_process@0.13.0", argv)
        self.assertIn("--target", argv)
        self.assertEqual(argv[argv.index("--target") + 1], "x86_64-pc-windows-msvc")
        self.assertEqual(argv[argv.index("--root") + 1], str(Path("/tmp/tools")))

    def test_render_process_install_argv_is_idempotent(self):
        argv = sw.render_process_install_argv("0.13.0", "x86_64-pc-windows-msvc", Path("/tmp/tools"))
        self.assertIn("--force", argv)

    def test_cargo_env_adds_crt_static(self):
        self.assertEqual(sw.cargo_env({})["RUSTFLAGS"], "-Ctarget-feature=+crt-static")

    def test_cargo_env_preserves_existing_rustflags(self):
        env = sw.cargo_env({"RUSTFLAGS": "-Dwarnings"})
        self.assertEqual(env["RUSTFLAGS"], "-Dwarnings -Ctarget-feature=+crt-static")

    def test_cargo_env_does_not_duplicate_crt_static(self):
        env = sw.cargo_env({"RUSTFLAGS": "-Ctarget-feature=+crt-static"})
        self.assertEqual(env["RUSTFLAGS"], "-Ctarget-feature=+crt-static")


class RenderProcessPin(unittest.TestCase):
    LOCK = (
        '[[package]]\nname = "bevy_cef"\nversion = "0.13.0"\n\n'
        '[[package]]\nname = "bevy_cef_core"\nversion = "0.13.4"\n'
    )

    def _lock(self, tmp: str) -> Path:
        path = Path(tmp) / "Cargo.lock"
        path.write_text(self.LOCK, encoding="utf-8")
        return path

    def test_locked_version_reads_the_named_package(self):
        with tempfile.TemporaryDirectory() as tmp:
            lock = self._lock(tmp)
            self.assertEqual(sw.locked_version("bevy_cef_core", lock), "0.13.4")
            self.assertEqual(sw.locked_version("bevy_cef", lock), "0.13.0")
            self.assertIsNone(sw.locked_version("not_a_package", lock))

    def test_a_lockfile_bump_the_pin_missed_is_rejected(self):
        with self.assertRaises(SystemExit) as raised:
            sw.assert_render_process_matches_lockfile("0.13.0", "0.13.4")
        self.assertIn("0.13.4", str(raised.exception))

    def test_a_matching_pin_passes(self):
        self.assertIsNone(sw.assert_render_process_matches_lockfile("0.13.0", "0.13.0"))

    def test_the_committed_lockfile_matches_the_pin(self):
        self.assertEqual(
            sw.locked_version(sw.RENDER_PROCESS_CRATE), sw.RENDER_PROCESS_VERSION
        )


class LicenseRtf(unittest.TestCase):
    def test_wraps_text_in_rtf(self):
        out = sw.license_rtf("MIT License\n")
        self.assertTrue(out.startswith("{\\rtf1"))
        self.assertTrue(out.endswith("}"))
        self.assertIn("MIT License", out)

    def test_escapes_rtf_control_characters(self):
        out = sw.license_rtf("a{b}c\\d")
        self.assertIn("a\\{b\\}c\\\\d", out)

    def test_converts_newlines_to_par(self):
        self.assertIn("first\\par", sw.license_rtf("first\nsecond"))

    def test_escapes_non_ascii_so_the_rtf_stays_ascii(self):
        out = sw.license_rtf("Copyright © 2026 山\U0001f600")
        self.assertIn("\\u169?", out)
        self.assertIn("\\u23665?", out)
        self.assertIn("\\u-10179?\\u-8704?", out)
        out.encode("ascii")


class StageConfigResolution(unittest.TestCase):
    def test_defaults_point_at_target_dist(self):
        args = sw.build_arg_parser().parse_args(["--version", "1.2.3"])
        cfg = sw.resolve_config(args)
        self.assertEqual(cfg.version, "1.2.3")
        self.assertEqual(cfg.stage_dir, sw.REPO_ROOT / "target" / "dist" / "stage")
        self.assertEqual(cfg.tools_dir, sw.REPO_ROOT / "target" / "dist" / "tools")
        self.assertIsNone(cfg.render_process_bin)
        self.assertFalse(cfg.skip_build)

    def test_overrides_are_expanded(self):
        args = sw.build_arg_parser().parse_args(
            ["--version", "1.2.3", "--out-dir", "/tmp/out", "--cef-dir", "/tmp/cef",
             "--render-process-bin", "/tmp/rp.exe", "--skip-build"]
        )
        cfg = sw.resolve_config(args)
        self.assertEqual(cfg.out_dir, Path("/tmp/out"))
        self.assertEqual(cfg.cef_dir, Path("/tmp/cef"))
        self.assertEqual(cfg.render_process_bin, Path("/tmp/rp.exe"))
        self.assertTrue(cfg.skip_build)


class StageCefTree(unittest.TestCase):
    def test_copies_staged_entries_and_creates_subdirectories(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            cef = root / "cef"
            _write(cef / "libcef.dll", b"cef")
            _write(cef / "locales" / "ja.pak", b"ja")
            stage = root / "stage"
            sw.copy_cef_entries(cef, stage, ["libcef.dll", "locales/ja.pak"])
            self.assertEqual((stage / "libcef.dll").read_bytes(), b"cef")
            self.assertEqual((stage / "locales" / "ja.pak").read_bytes(), b"ja")

    def test_inventory_failures_are_reported_together(self):
        with self.assertRaises(SystemExit) as raised:
            sw.assert_inventory_clean(["icudtl.dat"], ["brand_new.dll"], ["libcef.dll"])
        message = str(raised.exception)
        self.assertIn("icudtl.dat", message)
        self.assertIn("brand_new.dll", message)
        self.assertIn("libcef.dll", message)

    def test_inventory_clean_passes_silently(self):
        self.assertIsNone(sw.assert_inventory_clean([], [], []))


class OrzmdAssets(unittest.TestCase):
    def test_missing_web_assets_are_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            assets = Path(tmp) / "assets"
            assets.mkdir()
            (assets / ".gitkeep").touch()
            with self.assertRaises(SystemExit):
                sw.verify_orzmd_web_assets(assets)

    def test_present_web_assets_pass(self):
        with tempfile.TemporaryDirectory() as tmp:
            assets = Path(tmp) / "assets"
            assets.mkdir()
            (assets / "index.js").write_text("console.log(1)")
            self.assertIsNone(sw.verify_orzmd_web_assets(assets))


if __name__ == "__main__":
    unittest.main()
