from __future__ import annotations

import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import bundle_macos as bm


def _write_fake_macho(dest: Path) -> None:
    # NOTE: shutil.copy (not copy2) avoids PermissionError from SIP-restricted flags on /usr/bin/true
    shutil.copy("/usr/bin/true", dest)
    dest.chmod(0o755)


def _write_fake_cef_resources(fw: Path, locales: tuple[str, ...] = ("en", "ja", "de")) -> None:
    resources = fw / "Resources"
    resources.mkdir(exist_ok=True)
    for name in ("icudtl.dat", "resources.pak", "chrome_100_percent.pak",
                 "chrome_200_percent.pak", "v8_context_snapshot.arm64.bin",
                 "gpu_shader_cache.bin"):
        (resources / name).write_bytes(b"fake")
    for locale in locales:
        lproj = resources / f"{locale}.lproj"
        lproj.mkdir()
        (lproj / "locale.pak").write_bytes(b"fake")


def _write_fake_cef_swiftshader(fw: Path) -> None:
    libs = fw / "Libraries"
    _write_fake_macho(libs / "libvk_swiftshader.dylib")
    (libs / "vk_swiftshader_icd.json").write_bytes(b"{}")


class PureHelpers(unittest.TestCase):
    def test_version_less_than(self):
        self.assertTrue(bm.version_less_than("10.15", "11.0"))
        self.assertFalse(bm.version_less_than("11.0", "11.0"))
        self.assertFalse(bm.version_less_than("12.3", "11.0"))

    def test_helper_bundle_id_base(self):
        self.assertEqual(bm.helper_bundle_id("not.elm.orzma", ""), "not.elm.orzma.helper")

    def test_helper_bundle_id_variants(self):
        self.assertEqual(bm.helper_bundle_id("not.elm.orzma", " (GPU)"), "not.elm.orzma.helper.gpu")
        self.assertEqual(bm.helper_bundle_id("not.elm.orzma", " (Renderer)"), "not.elm.orzma.helper.renderer")
        self.assertEqual(bm.helper_bundle_id("not.elm.orzma", " (Plugin)"), "not.elm.orzma.helper.plugin")

    def test_config_paths(self):
        cfg = bm.BundleConfig(
            version="1.2.3", app_name="orzma", bin_name="orzma",
            bundle_id_base="not.elm.orzma", arch="arm64", target_triple="aarch64-apple-darwin",
            bin_source=Path("/tmp/orzma"), cef_framework=Path("/tmp/cef"),
            helper_bin=Path("/tmp/helper"), out_dir=Path("/tmp/out"),
            sign_identity="-", no_sign=False, notarize=False,
        )
        self.assertEqual(cfg.app_path, Path("/tmp/out/orzma.app"))
        self.assertEqual(cfg.dmg_path, Path("/tmp/out/orzma-1.2.3-arm64.dmg"))


class CaskTemplate(unittest.TestCase):
    def test_template_has_companion_binary_stanzas(self):
        tmpl = (bm.REPO_ROOT / "build" / "macos" / "homebrew" / "orzma.rb.tmpl").read_text()
        self.assertIn('app "orzma.app"', tmpl)
        for name in bm.COMPANION_BINS:
            self.assertIn(f'binary "#{{appdir}}/orzma.app/Contents/Resources/{name}"', tmpl)

    def test_template_downloads_the_dmg_the_bundler_writes(self):
        tmpl = (bm.REPO_ROOT / "build" / "macos" / "homebrew" / "orzma.rb.tmpl").read_text()
        name = bm.dmg_name(bm.APP_NAME, "#{version}", bm.ARCH)
        self.assertIn(
            f'url "https://github.com/not-elm/orzma/releases/download/v#{{version}}/{name}"',
            tmpl,
        )


class PlistLogic(unittest.TestCase):
    def test_merge_cef_keys_into_empty(self):
        out = bm.merge_cef_keys({})
        self.assertEqual(out["LSEnvironment"]["MallocNanoZone"], "0")
        self.assertEqual(out["LSMinimumSystemVersion"], "11.0")
        self.assertTrue(out["NSSupportsAutomaticGraphicsSwitching"])

    def test_merge_cef_keys_preserves_existing_env(self):
        out = bm.merge_cef_keys({"LSEnvironment": {"FOO": "bar"}})
        self.assertEqual(out["LSEnvironment"]["FOO"], "bar")
        self.assertEqual(out["LSEnvironment"]["MallocNanoZone"], "0")

    def test_merge_cef_keys_keeps_higher_min_version(self):
        out = bm.merge_cef_keys({"LSMinimumSystemVersion": "12.0"})
        self.assertEqual(out["LSMinimumSystemVersion"], "12.0")

    def test_merge_cef_keys_bumps_lower_min_version(self):
        out = bm.merge_cef_keys({"LSMinimumSystemVersion": "10.15"})
        self.assertEqual(out["LSMinimumSystemVersion"], "11.0")

    def test_merge_cef_keys_keeps_existing_graphics_switch(self):
        out = bm.merge_cef_keys({"NSSupportsAutomaticGraphicsSwitching": False})
        self.assertFalse(out["NSSupportsAutomaticGraphicsSwitching"])

    def test_merge_cef_keys_rejects_bad_env(self):
        with self.assertRaises(ValueError):
            bm.merge_cef_keys({"LSEnvironment": "not-a-dict"})

    def test_build_helper_plist(self):
        p = bm.build_helper_plist("orzma Helper (GPU)", "not.elm.orzma.helper.gpu")
        self.assertEqual(p["CFBundleExecutable"], "orzma Helper (GPU)")
        self.assertEqual(p["CFBundleName"], "orzma Helper (GPU)")
        self.assertEqual(p["CFBundleIdentifier"], "not.elm.orzma.helper.gpu")
        self.assertEqual(p["CFBundlePackageType"], "APPL")
        self.assertEqual(p["LSEnvironment"]["MallocNanoZone"], "0")
        self.assertTrue(p["LSUIElement"])


class CommandBuilders(unittest.TestCase):
    def test_cargo_build_argv(self):
        self.assertEqual(
            bm.cargo_build_argv("aarch64-apple-darwin", "dist"),
            ["cargo", "build", "--profile", "dist", "--target", "aarch64-apple-darwin", "--locked",
             "--no-default-features"],
        )

    def test_parse_lipo_archs(self):
        self.assertEqual(bm.parse_lipo_archs("x86_64 arm64\n"), {"x86_64", "arm64"})

    def test_codesign_argv_adhoc(self):
        argv = bm.codesign_argv("-", Path("/tmp/a.app"), hardened=False, entitlements=None)
        self.assertEqual(argv, ["codesign", "--force", "--sign", "-", "/tmp/a.app"])

    def test_codesign_argv_hardened(self):
        argv = bm.codesign_argv(
            "Developer ID Application: X", Path("/tmp/a.app"),
            hardened=True, entitlements=Path("/tmp/e.plist"),
        )
        self.assertEqual(argv, [
            "codesign", "--force", "--sign", "Developer ID Application: X",
            "--options", "runtime", "--entitlements", "/tmp/e.plist", "/tmp/a.app",
        ])

    def test_codesign_verify_argv(self):
        self.assertEqual(
            bm.codesign_verify_argv(Path("/tmp/a.app")),
            ["codesign", "--verify", "--deep", "--strict", "/tmp/a.app"],
        )

    def test_ditto_zip_argv(self):
        self.assertEqual(
            bm.ditto_zip_argv(Path("/tmp/a.app"), Path("/tmp/a.zip")),
            ["ditto", "-c", "-k", "--sequesterRsrc", "--keepParent", "/tmp/a.app", "/tmp/a.zip"],
        )

    def test_xattr_strip_argv(self):
        self.assertEqual(bm.xattr_strip_argv(Path("/tmp/a.app")), ["xattr", "-cr", "/tmp/a.app"])

    def test_notarytool_submit_argv(self):
        self.assertEqual(
            bm.notarytool_submit_argv(Path("/tmp/a.zip"), "me@x.com", "TEAM", "pw"),
            ["xcrun", "notarytool", "submit", "/tmp/a.zip", "--apple-id", "me@x.com",
             "--team-id", "TEAM", "--password", "pw", "--wait"],
        )

    def test_stapler_argv(self):
        self.assertEqual(bm.stapler_argv(Path("/tmp/a.app")), ["xcrun", "stapler", "staple", "/tmp/a.app"])

    def test_companion_cargo_build_argv(self):
        self.assertEqual(
            bm.companion_cargo_build_argv("aarch64-apple-darwin", "dist", ("orzbrowser", "orzmd")),
            ["cargo", "build", "--profile", "dist", "--target", "aarch64-apple-darwin",
             "--locked", "-p", "orzbrowser", "-p", "orzmd"],
        )

    def test_companion_bins_constant(self):
        self.assertEqual(bm.COMPANION_BINS, ("orzbrowser", "orzmd"))

    def test_compute_sha256(self):
        with tempfile.NamedTemporaryFile(delete=False) as f:
            f.write(b"hello")
            name = f.name
        try:
            self.assertEqual(
                bm.compute_sha256(Path(name)),
                "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
            )
        finally:
            os.unlink(name)


class DmgCommands(unittest.TestCase):
    def test_dmg_name(self):
        self.assertEqual(bm.dmg_name("orzma", "0.1.0", "arm64"), "orzma-0.1.0-arm64.dmg")

    def test_ditto_copy_argv(self):
        self.assertEqual(
            bm.ditto_copy_argv(Path("/tmp/a.app"), Path("/tmp/stage/a.app")),
            ["ditto", "/tmp/a.app", "/tmp/stage/a.app"],
        )

    def test_hdiutil_create_argv(self):
        self.assertEqual(
            bm.hdiutil_create_argv("orzma", Path("/tmp/stage"), Path("/tmp/a.dmg")),
            ["hdiutil", "create", "-volname", "orzma", "-srcfolder", "/tmp/stage",
             "-fs", "HFS+", "-format", "ULMO", "-ov", "/tmp/a.dmg"],
        )

    def test_hdiutil_create_argv_keeps_a_spaced_path_as_one_argument(self):
        argv = bm.hdiutil_create_argv(
            "orzma", Path("/tmp/My Projects/stage"), Path("/tmp/My Projects/a.dmg")
        )
        self.assertEqual(argv[argv.index("-srcfolder") + 1], "/tmp/My Projects/stage")
        self.assertEqual(argv[-1], "/tmp/My Projects/a.dmg")

    def test_hdiutil_verify_argv(self):
        self.assertEqual(
            bm.hdiutil_verify_argv(Path("/tmp/a.dmg")), ["hdiutil", "verify", "/tmp/a.dmg"]
        )


def _run_failing_first(failures: int):
    calls = []

    def fake_run(argv, redact=()):
        calls.append(argv)
        if len(calls) <= failures:
            raise subprocess.CalledProcessError(1, argv)

    return fake_run, calls


class CreateDmgRetry(unittest.TestCase):
    ARGV = ["hdiutil", "create", "/tmp/a.dmg"]

    def test_a_first_try_success_does_not_wait(self):
        fake_run, calls = _run_failing_first(0)
        with mock.patch.object(bm, "run", fake_run), mock.patch.object(bm.time, "sleep") as sleep:
            bm.create_dmg(self.ARGV)
        self.assertEqual(calls, [self.ARGV])
        sleep.assert_not_called()

    def test_a_transient_failure_is_retried_until_it_succeeds(self):
        fake_run, calls = _run_failing_first(2)
        with mock.patch.object(bm, "run", fake_run), mock.patch.object(bm.time, "sleep") as sleep:
            bm.create_dmg(self.ARGV)
        self.assertEqual(len(calls), 3)
        self.assertEqual(sleep.call_args_list, [mock.call(5), mock.call(5)])

    def test_the_last_failure_is_raised_without_a_final_wait(self):
        fake_run, calls = _run_failing_first(3)
        with mock.patch.object(bm, "run", fake_run), mock.patch.object(bm.time, "sleep") as sleep:
            with self.assertRaises(subprocess.CalledProcessError):
                bm.create_dmg(self.ARGV)
        self.assertEqual(len(calls), 3)
        self.assertEqual(sleep.call_count, 2)


class PackageDmg(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.out = Path(self.tmp.name) / "out"
        self.out.mkdir()
        self.cfg = bm.BundleConfig(
            version="9.9.9", app_name="orzma", bin_name="orzma",
            bundle_id_base="not.elm.orzma", arch="arm64", target_triple="aarch64-apple-darwin",
            bin_source=self.out / "orzma", cef_framework=self.out / "cef",
            helper_bin=self.out / "helper", out_dir=self.out,
            sign_identity="-", no_sign=False, notarize=False,
        )
        self.dmg = self.out / "orzma-9.9.9-arm64.dmg"
        self.sidecar = self.out / "orzma-9.9.9-arm64.dmg.sha256"
        self.staged = []

    def tearDown(self):
        self.tmp.cleanup()

    def _fake_run(self, fail_on=None, error=None):
        def fake_run(argv, redact=()):
            if argv[0] == "ditto":
                Path(argv[-1]).mkdir()
            if argv[:2] == ["hdiutil", "create"]:
                srcfolder = Path(argv[argv.index("-srcfolder") + 1])
                self.staged.append({
                    "entries": sorted(p.name for p in srcfolder.iterdir()),
                    "link": os.readlink(srcfolder / "Applications"),
                })
                Path(argv[-1]).write_bytes(b"new dmg")
            if fail_on is not None and argv[:2] == fail_on:
                raise error if error is not None else subprocess.CalledProcessError(1, argv)

        return fake_run

    def _staging_dirs(self):
        return list(self.out.glob("dmg-staging-*"))

    def test_package_replaces_stale_files_with_a_dmg_and_its_sidecar(self):
        self.dmg.write_bytes(b"old dmg")
        self.sidecar.write_text("stale\n")
        with mock.patch.object(bm, "run", self._fake_run()):
            digest = bm.package(self.cfg)
        self.assertEqual(digest, bm.compute_sha256(self.dmg))
        self.assertEqual(self.dmg.read_bytes(), b"new dmg")
        self.assertEqual(self.sidecar.read_text(), f"{digest}  orzma-9.9.9-arm64.dmg\n")
        self.assertEqual(
            self.staged, [{"entries": ["Applications", "orzma.app"], "link": "/Applications"}]
        )
        self.assertEqual(self._staging_dirs(), [])

    def test_a_failed_verify_leaves_neither_dmg_nor_sidecar(self):
        self.sidecar.write_text("stale\n")
        with mock.patch.object(bm, "run", self._fake_run(fail_on=["hdiutil", "verify"])):
            with self.assertRaises(subprocess.CalledProcessError):
                bm.package(self.cfg)
        self.assertFalse(self.dmg.exists())
        self.assertFalse(self.sidecar.exists())
        self.assertEqual(self._staging_dirs(), [])

    def test_a_create_that_fails_every_attempt_leaves_neither_dmg_nor_sidecar(self):
        self.sidecar.write_text("stale\n")
        with mock.patch.object(bm, "run", self._fake_run(fail_on=["hdiutil", "create"])), \
                mock.patch.object(bm.time, "sleep"):
            with self.assertRaises(subprocess.CalledProcessError):
                bm.package(self.cfg)
        self.assertEqual(len(self.staged), bm.HDIUTIL_CREATE_ATTEMPTS)
        self.assertFalse(self.dmg.exists())
        self.assertFalse(self.sidecar.exists())
        self.assertEqual(self._staging_dirs(), [])

    def test_an_interrupted_create_removes_the_partial_dmg(self):
        fake = self._fake_run(fail_on=["hdiutil", "create"], error=KeyboardInterrupt())
        with mock.patch.object(bm, "run", fake):
            with self.assertRaises(KeyboardInterrupt):
                bm.package(self.cfg)
        self.assertFalse(self.dmg.exists())
        self.assertFalse(self.sidecar.exists())
        self.assertEqual(self._staging_dirs(), [])


class ConfigResolution(unittest.TestCase):
    def _parse(self, argv):
        return bm.build_arg_parser().parse_args(argv)

    def test_resolve_defaults_with_explicit_version(self):
        cfg = bm.resolve_config(self._parse(["--version", "0.1.0", "--skip-build"]))
        self.assertEqual(cfg.version, "0.1.0")
        self.assertEqual(cfg.bin_name, "orzma")
        self.assertEqual(cfg.sign_identity, "-")
        self.assertFalse(cfg.notarize)
        self.assertEqual(cfg.bin_source.name, "orzma")
        self.assertIn("aarch64-apple-darwin", str(cfg.bin_source))

    def test_resolve_sign_identity_from_env(self):
        os.environ["MACOS_SIGN_IDENTITY"] = "Developer ID Application: Y"
        try:
            cfg = bm.resolve_config(self._parse(["--version", "0.1.0"]))
            self.assertEqual(cfg.sign_identity, "Developer ID Application: Y")
        finally:
            del os.environ["MACOS_SIGN_IDENTITY"]

    def test_notarize_downgraded_when_adhoc(self):
        cfg = bm.resolve_config(self._parse(["--version", "0.1.0", "--notarize"]))
        self.assertFalse(cfg.notarize)

    def test_verify_prerequisites_missing_binary(self):
        with tempfile.TemporaryDirectory() as d:
            cfg = bm.resolve_config(self._parse([
                "--version", "0.1.0", "--bin", str(Path(d) / "missing"),
                "--cef-framework", d, "--helper-bin", str(Path(d) / "missing-helper"),
            ]))
            with self.assertRaises(SystemExit):
                bm.verify_prerequisites(cfg)

    def test_resolve_companion_defaults(self):
        cfg = bm.resolve_config(self._parse(["--version", "0.1.0", "--skip-build"]))
        names = list(cfg.companion_bins.keys())
        self.assertEqual(names, ["orzbrowser", "orzmd"])
        for p in cfg.companion_bins.values():
            self.assertIn("aarch64-apple-darwin", str(p))
            self.assertIn("dist", str(p))

    def test_resolve_companion_overrides(self):
        cfg = bm.resolve_config(self._parse([
            "--version", "0.1.0", "--skip-build",
            "--orzbrowser-bin", "/tmp/ob", "--orzmd-bin", "/tmp/om",
        ]))
        self.assertEqual(cfg.companion_bins, {"orzbrowser": Path("/tmp/ob"), "orzmd": Path("/tmp/om")})

    def test_resolve_companion_override_expands_user(self):
        cfg = bm.resolve_config(self._parse([
            "--version", "0.1.0", "--skip-build", "--orzbrowser-bin", "~/bins/orzbrowser",
        ]))
        self.assertFalse(str(cfg.companion_bins["orzbrowser"]).startswith("~"))
        self.assertTrue(str(cfg.companion_bins["orzbrowser"]).endswith("/bins/orzbrowser"))

    def test_verify_prerequisites_missing_companion(self):
        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            # main bin, cef dir, helper all present; only a companion is missing
            (d / "orzma").write_bytes(b"")
            (d / "helper").write_bytes(b"")
            cef = d / "cef"
            cef.mkdir()
            cfg = bm.resolve_config(bm.build_arg_parser().parse_args([
                "--version", "0.1.0", "--bin", str(d / "orzma"),
                "--cef-framework", str(cef), "--helper-bin", str(d / "helper"),
                "--orzbrowser-bin", str(d / "missing-ob"), "--orzmd-bin", str(d / "missing-om"),
            ]))
            with self.assertRaises(SystemExit):
                bm.verify_prerequisites(cfg)


@unittest.skipUnless(sys.platform == "darwin", "macOS-only integration test")
class AssembleAndEmbed(unittest.TestCase):
    def _fake_cef(self, root: Path) -> Path:
        fw = root / "Chromium Embedded Framework.framework"
        (fw / "Libraries").mkdir(parents=True)
        _write_fake_macho(fw / "Chromium Embedded Framework")
        _write_fake_macho(fw / "Libraries" / "libEGL.dylib")
        _write_fake_macho(fw / "Libraries" / "libGLESv2.dylib")
        _write_fake_cef_swiftshader(fw)
        _write_fake_cef_resources(fw)
        return fw

    def _cfg(self, d: Path) -> "bm.BundleConfig":
        _write_fake_macho(d / "orzma")
        _write_fake_macho(d / "helper")
        fw = self._fake_cef(d)
        return bm.BundleConfig(
            version="9.9.9", app_name="orzma", bin_name="orzma",
            bundle_id_base="not.elm.orzma", arch="arm64", target_triple="aarch64-apple-darwin",
            bin_source=d / "orzma", cef_framework=fw, helper_bin=d / "helper",
            out_dir=d / "out", sign_identity="-", no_sign=True, notarize=False,
        )

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.d = Path(self._tmp.name)
        self.cfg = self._cfg(self.d)
        self.cfg.out_dir.mkdir(parents=True)

    def tearDown(self):
        self._tmp.cleanup()

    def test_assemble_then_embed(self):
        import plistlib
        bm.assemble_app(self.cfg)
        bm.embed_cef(self.cfg)
        contents = self.cfg.app_path / "Contents"
        self.assertTrue((contents / "MacOS" / "orzma").is_file())
        with open(contents / "Info.plist", "rb") as f:
            plist = plistlib.load(f)
        self.assertEqual(plist["CFBundleShortVersionString"], "9.9.9")
        self.assertEqual(plist["LSEnvironment"]["MallocNanoZone"], "0")
        self.assertTrue(plist["NSSupportsAutomaticGraphicsSwitching"])
        fw = contents / "Frameworks" / "Chromium Embedded Framework.framework"
        self.assertTrue((fw / "Chromium Embedded Framework").is_file())
        for suffix, idsfx in [("", "helper"), (" (GPU)", "helper.gpu"),
                              (" (Renderer)", "helper.renderer"), (" (Plugin)", "helper.plugin")]:
            helper = contents / "Frameworks" / f"orzma Helper{suffix}.app"
            self.assertTrue((helper / "Contents" / "MacOS" / f"orzma Helper{suffix}").is_file())
            with open(helper / "Contents" / "Info.plist", "rb") as f:
                hp = plistlib.load(f)
            self.assertEqual(hp["CFBundleIdentifier"], f"not.elm.orzma.{idsfx}")
            self.assertTrue(hp["LSUIElement"])

    def test_embed_drops_dev_only_render_process(self):
        _write_fake_macho(self.cfg.cef_framework / "Libraries" / "bevy_cef_debug_render_process")
        bm.assemble_app(self.cfg)
        bm.embed_cef(self.cfg)
        libs = (self.cfg.app_path / "Contents" / "Frameworks"
                / "Chromium Embedded Framework.framework" / "Libraries")
        self.assertFalse((libs / "bevy_cef_debug_render_process").exists())
        self.assertTrue((libs / "libEGL.dylib").is_file())
        self.assertTrue(
            (self.cfg.cef_framework / "Libraries" / "bevy_cef_debug_render_process").is_file()
        )


class CefLocaleHelpers(unittest.TestCase):
    SHIPPED = ("en", "ja")

    def test_a_shipped_locale_matches_only_its_exact_lproj_dir(self):
        self.assertTrue(bm.is_cef_locale_dir("en.lproj", self.SHIPPED))
        self.assertTrue(bm.is_cef_locale_dir("ja.lproj", self.SHIPPED))
        for name in ("fr.lproj", "ja_FEMININE.lproj", "en_NEUTER.lproj",
                     "en-GB.lproj", "en", "en.lproj.bak"):
            self.assertFalse(bm.is_cef_locale_dir(name, self.SHIPPED), name)

    def test_missing_cef_locale_dirs_lists_absent_packs_in_order(self):
        self.assertEqual(bm.missing_cef_locale_dirs(["en.lproj", "ja.lproj", "fr.lproj"], self.SHIPPED), [])
        self.assertEqual(bm.missing_cef_locale_dirs(["ja.lproj", "en_FEMININE.lproj"], self.SHIPPED), ["en"])
        self.assertEqual(bm.missing_cef_locale_dirs([], self.SHIPPED), ["en", "ja"])

    def test_app_advertised_localizations_unions_every_source(self):
        self.assertEqual(
            bm.app_advertised_localizations(
                {"CFBundleLocalizations": ["fr"], "CFBundleDevelopmentRegion": "en"},
                ["AppIcon.icns", "de.lproj", "ja.lproj"],
            ),
            {"fr", "en", "de", "ja"},
        )

    def test_app_advertised_localizations_falls_back_to_the_development_region(self):
        self.assertEqual(
            bm.app_advertised_localizations({"CFBundleDevelopmentRegion": "ja"}, ["AppIcon.icns"]),
            {"ja"},
        )

    def test_app_advertised_localizations_is_empty_without_any_declaration(self):
        self.assertEqual(bm.app_advertised_localizations({}, []), set())


@unittest.skipUnless(sys.platform == "darwin", "macOS-only integration test")
class CefFrameworkPruning(unittest.TestCase):
    def _cfg(self, d: Path) -> "bm.BundleConfig":
        _write_fake_macho(d / "orzma")
        _write_fake_macho(d / "helper")
        fw = d / "Chromium Embedded Framework.framework"
        (fw / "Libraries").mkdir(parents=True)
        _write_fake_macho(fw / "Chromium Embedded Framework")
        _write_fake_macho(fw / "Libraries" / "libEGL.dylib")
        _write_fake_macho(fw / "Libraries" / "libGLESv2.dylib")
        _write_fake_cef_swiftshader(fw)
        _write_fake_cef_resources(fw)
        return bm.BundleConfig(
            version="9.9.9", app_name="orzma", bin_name="orzma",
            bundle_id_base="not.elm.orzma", arch="arm64", target_triple="aarch64-apple-darwin",
            bin_source=d / "orzma", cef_framework=fw, helper_bin=d / "helper",
            out_dir=d / "out", sign_identity="-", no_sign=True, notarize=False,
        )

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.d = Path(self._tmp.name)
        self.cfg = self._cfg(self.d)
        self.cfg.out_dir.mkdir(parents=True)

    def tearDown(self):
        self._tmp.cleanup()

    def _embedded_framework(self) -> Path:
        return (self.cfg.app_path / "Contents" / "Frameworks"
                / "Chromium Embedded Framework.framework")

    def _set_app_localizations(self, locales: list[str]) -> None:
        import plistlib
        plist_path = self.cfg.app_path / "Contents" / "Info.plist"
        with open(plist_path, "rb") as f:
            plist = plistlib.load(f)
        plist["CFBundleLocalizations"] = locales
        with open(plist_path, "wb") as f:
            plistlib.dump(plist, f)

    def test_embed_ships_only_the_locale_the_app_resolves(self):
        bm.assemble_app(self.cfg)
        bm.embed_cef(self.cfg)
        resources = self._embedded_framework() / "Resources"
        self.assertTrue((resources / "en.lproj" / "locale.pak").is_file())
        self.assertFalse((resources / "ja.lproj").exists())
        self.assertFalse((resources / "de.lproj").exists())

    def test_embed_drops_swiftshader_and_the_shader_cache(self):
        bm.assemble_app(self.cfg)
        bm.embed_cef(self.cfg)
        fw = self._embedded_framework()
        self.assertFalse((fw / "Libraries" / "libvk_swiftshader.dylib").exists())
        self.assertFalse((fw / "Libraries" / "vk_swiftshader_icd.json").exists())
        self.assertFalse((fw / "Resources" / "gpu_shader_cache.bin").exists())

    def test_embed_keeps_every_required_and_angle_component(self):
        bm.assemble_app(self.cfg)
        bm.embed_cef(self.cfg)
        fw = self._embedded_framework()
        self.assertTrue((fw / "Chromium Embedded Framework").is_file())
        for name in ("libEGL.dylib", "libGLESv2.dylib"):
            self.assertTrue((fw / "Libraries" / name).is_file(), name)
        for name in ("icudtl.dat", "resources.pak", "chrome_100_percent.pak",
                     "chrome_200_percent.pak", "v8_context_snapshot.arm64.bin"):
            self.assertTrue((fw / "Resources" / name).is_file(), name)

    def test_embed_leaves_the_shared_cef_framework_untouched(self):
        bm.assemble_app(self.cfg)
        bm.embed_cef(self.cfg)
        source = self.cfg.cef_framework
        self.assertTrue((source / "Resources" / "ja.lproj" / "locale.pak").is_file())
        self.assertTrue((source / "Resources" / "gpu_shader_cache.bin").is_file())
        self.assertTrue((source / "Libraries" / "libvk_swiftshader.dylib").is_file())

    def test_embed_raises_when_a_shipped_locale_pack_is_absent(self):
        shutil.rmtree(self.cfg.cef_framework / "Resources" / "en.lproj")
        bm.assemble_app(self.cfg)
        with self.assertRaises(SystemExit):
            bm.embed_cef(self.cfg)

    def test_embed_raises_when_the_app_advertises_an_unshipped_locale(self):
        bm.assemble_app(self.cfg)
        self._set_app_localizations(["en", "ja"])
        with self.assertRaises(SystemExit):
            bm.embed_cef(self.cfg)

    def _set_app_development_region(self, region: str) -> None:
        import plistlib
        plist_path = self.cfg.app_path / "Contents" / "Info.plist"
        with open(plist_path, "rb") as f:
            plist = plistlib.load(f)
        plist["CFBundleDevelopmentRegion"] = region
        with open(plist_path, "wb") as f:
            plistlib.dump(plist, f)

    def test_embed_raises_when_the_development_region_is_unshipped(self):
        bm.assemble_app(self.cfg)
        self._set_app_development_region("ja")
        with self.assertRaises(SystemExit):
            bm.embed_cef(self.cfg)

    def test_embed_accepts_an_app_advertising_only_a_shipped_locale(self):
        bm.assemble_app(self.cfg)
        self._set_app_localizations(["en"])
        bm.embed_cef(self.cfg)
        self.assertTrue(
            (self._embedded_framework() / "Resources" / "en.lproj" / "locale.pak").is_file()
        )


@unittest.skipUnless(sys.platform == "darwin", "macOS-only integration test")
class CopyCompanions(unittest.TestCase):
    def test_copy_companions_into_resources(self):
        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            _write_fake_macho(d / "orzbrowser")
            _write_fake_macho(d / "orzmd")
            cfg = bm.BundleConfig(
                version="9.9.9", app_name="orzma", bin_name="orzma",
                bundle_id_base="not.elm.orzma", arch="arm64",
                target_triple="aarch64-apple-darwin",
                bin_source=d / "orzma", cef_framework=d / "cef", helper_bin=d / "helper",
                out_dir=d / "out", sign_identity="-", no_sign=True, notarize=False,
                companion_bins={"orzbrowser": d / "orzbrowser", "orzmd": d / "orzmd"},
            )
            resources = cfg.app_path / "Contents" / "Resources"
            resources.mkdir(parents=True)
            bm.copy_companions(cfg)
            for name in ("orzbrowser", "orzmd"):
                dest = resources / name
                self.assertTrue(dest.is_file())
                self.assertTrue(os.access(dest, os.X_OK))

    def test_override_basename_embeds_under_canonical_name(self):
        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            _write_fake_macho(d / "orzbrowser-cli")
            _write_fake_macho(d / "orzmd-v2")
            cfg = bm.BundleConfig(
                version="9.9.9", app_name="orzma", bin_name="orzma",
                bundle_id_base="not.elm.orzma", arch="arm64",
                target_triple="aarch64-apple-darwin",
                bin_source=d / "orzma", cef_framework=d / "cef", helper_bin=d / "helper",
                out_dir=d / "out", sign_identity="-", no_sign=True, notarize=False,
                companion_bins={"orzbrowser": d / "orzbrowser-cli", "orzmd": d / "orzmd-v2"},
            )
            (cfg.app_path / "Contents" / "Resources").mkdir(parents=True)
            bm.copy_companions(cfg)
            resources = cfg.app_path / "Contents" / "Resources"
            self.assertTrue((resources / "orzbrowser").is_file())
            self.assertTrue((resources / "orzmd").is_file())
            self.assertFalse((resources / "orzbrowser-cli").exists())


class CopyLicenses(unittest.TestCase):
    def test_copy_licenses_into_resources(self):
        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            cfg = bm.BundleConfig(
                version="9.9.9", app_name="orzma", bin_name="orzma",
                bundle_id_base="not.elm.orzma", arch="arm64",
                target_triple="aarch64-apple-darwin",
                bin_source=d / "orzma", cef_framework=d / "cef", helper_bin=d / "helper",
                out_dir=d / "out", sign_identity="-", no_sign=True, notarize=False,
            )
            (cfg.app_path / "Contents" / "Resources").mkdir(parents=True)
            bm.copy_licenses(cfg)
            resources = cfg.app_path / "Contents" / "Resources"
            self.assertTrue((resources / "THIRD-PARTY-LICENSES.md").is_file())
            self.assertTrue((resources / "CREDITS.html").is_file())


@unittest.skipUnless(sys.platform == "darwin", "macOS-only integration test")
class EndToEnd(unittest.TestCase):
    def _unsigned_macho(self, dest: Path) -> None:
        _write_fake_macho(dest)
        subprocess.run(["codesign", "--remove-signature", str(dest)], check=True)

    def _fake_cef(self, root: Path) -> Path:
        fw = root / "Chromium Embedded Framework.framework"
        (fw / "Libraries").mkdir(parents=True)
        _write_fake_macho(fw / "Chromium Embedded Framework")
        _write_fake_macho(fw / "Libraries" / "libEGL.dylib")
        _write_fake_cef_swiftshader(fw)
        _write_fake_cef_resources(fw)
        return fw

    def test_main_adhoc_end_to_end(self):
        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            _write_fake_macho(d / "orzma")
            _write_fake_macho(d / "helper")
            self._unsigned_macho(d / "orzbrowser")
            self._unsigned_macho(d / "orzmd")
            fw = self._fake_cef(d)
            out = d / "out"
            bm.main([
                "--skip-build", "--version", "9.9.9",
                "--bin", str(d / "orzma"),
                "--cef-framework", str(fw),
                "--helper-bin", str(d / "helper"),
                "--orzbrowser-bin", str(d / "orzbrowser"),
                "--orzmd-bin", str(d / "orzmd"),
                "--out-dir", str(out),
            ])
            dmg_path = out / "orzma-9.9.9-arm64.dmg"
            self.assertTrue(dmg_path.is_file())
            sha_file = out / "orzma-9.9.9-arm64.dmg.sha256"
            self.assertTrue(sha_file.is_file())
            self.assertEqual(
                sha_file.read_text(),
                f"{bm.compute_sha256(dmg_path)}  orzma-9.9.9-arm64.dmg\n",
            )
            self.assertEqual(list(out.glob("dmg-staging-*")), [])
            self.assertFalse((out / "orzma-9.9.9-arm64.zip").exists())
            resources = out / "orzma.app" / "Contents" / "Resources"
            self.assertTrue((resources / "orzbrowser").is_file())
            self.assertTrue((resources / "orzmd").is_file())
            self.assertTrue((resources / "THIRD-PARTY-LICENSES.md").is_file())
            self.assertTrue((resources / "CREDITS.html").is_file())
            # ad-hoc signature must verify deep+strict on the outer bundle
            subprocess.run(
                ["codesign", "--verify", "--deep", "--strict", str(out / "orzma.app")],
                check=True,
            )
            # NOTE: codesign --verify --deep --strict on the outer bundle does not descend into
            # plain executables inside Contents/Resources (only into sub-bundles). We must
            # explicitly verify each companion so the test fails if the signing loop is removed.
            for name in ("orzbrowser", "orzmd"):
                subprocess.run(
                    ["codesign", "--verify", str(resources / name)],
                    check=True,
                )
            mount = d / "mnt"
            mount.mkdir()
            subprocess.run(
                ["hdiutil", "attach", "-nobrowse", "-readonly", "-mountpoint", str(mount),
                 str(dmg_path)],
                check=True,
            )
            try:
                mounted_app = mount / "orzma.app"
                self.assertTrue(mounted_app.is_dir())
                link = mount / "Applications"
                self.assertTrue(link.is_symlink())
                self.assertEqual(os.readlink(link), "/Applications")
                subprocess.run(
                    ["codesign", "--verify", "--deep", "--strict", str(mounted_app)],
                    check=True,
                )
                for name in ("orzbrowser", "orzmd"):
                    subprocess.run(
                        ["codesign", "--verify",
                         str(mounted_app / "Contents" / "Resources" / name)],
                        check=True,
                    )
            finally:
                subprocess.run(["hdiutil", "detach", str(mount)], check=True)


class CompanionSigning(unittest.TestCase):
    def test_companions_signed_hardened_without_entitlements(self):
        recorded = []

        def fake_run(argv, redact=()):
            recorded.append(argv)

        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            app = d / "out" / "orzma.app"
            (app / "Contents" / "Resources").mkdir(parents=True)
            cef = app / "Contents" / "Frameworks" / "Chromium Embedded Framework.framework"
            (cef / "Libraries").mkdir(parents=True)
            cfg = bm.BundleConfig(
                version="9.9.9", app_name="orzma", bin_name="orzma",
                bundle_id_base="not.elm.orzma", arch="arm64",
                target_triple="aarch64-apple-darwin",
                bin_source=d / "orzma", cef_framework=d / "cef", helper_bin=d / "helper",
                out_dir=d / "out",
                sign_identity="Developer ID Application: TEST", no_sign=False, notarize=False,
                companion_bins={"orzbrowser": d / "orzbrowser", "orzmd": d / "orzmd"},
            )
            orig = bm.run
            bm.run = fake_run
            try:
                bm.codesign_bundle(cfg)
            finally:
                bm.run = orig

        resources = app / "Contents" / "Resources"
        sign_argvs = [a for a in recorded if a[:1] == ["codesign"] and "--sign" in a]

        def argv_for(path):
            return next(a for a in sign_argvs if a[-1] == str(path))

        for name in ("orzbrowser", "orzmd"):
            a = argv_for(resources / name)
            self.assertIn("--options", a)            # hardened runtime kept
            self.assertNotIn("--entitlements", a)    # least privilege: no CEF grants
        # the outer app IS signed with the CEF entitlements
        self.assertIn("--entitlements", argv_for(app))


class OrzmdWebAssetsGuard(unittest.TestCase):
    def test_missing_assets_raises(self):
        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            (d / ".gitkeep").write_text("")
            (d / ".gitignore").write_text("*\n")
            with self.assertRaises(SystemExit):
                bm.verify_orzmd_web_assets(d)

    def test_present_assets_ok(self):
        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            (d / "index.html").write_text("<html></html>")
            bm.verify_orzmd_web_assets(d)

    def test_missing_dir_raises(self):
        with tempfile.TemporaryDirectory() as d:
            with self.assertRaises(SystemExit):
                bm.verify_orzmd_web_assets(Path(d) / "does-not-exist")


class NotarizeGuards(unittest.TestCase):
    def _parse(self, argv):
        return bm.build_arg_parser().parse_args(argv)

    def test_no_sign_disables_notarize(self):
        cfg = bm.resolve_config(self._parse([
            "--version", "0.1.0", "--no-sign", "--notarize",
            "--sign-identity", "Developer ID Application: X",
        ]))
        self.assertFalse(cfg.notarize)

    def test_notarize_raises_without_credentials(self):
        cfg = bm.resolve_config(self._parse([
            "--version", "0.1.0", "--notarize",
            "--sign-identity", "Developer ID Application: X",
        ]))
        for var in ("APPLE_ID", "APPLE_TEAM_ID", "APPLE_APP_PASSWORD"):
            os.environ.pop(var, None)
        with self.assertRaises(SystemExit):
            bm.notarize(cfg)


if __name__ == "__main__":
    unittest.main()
