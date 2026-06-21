from __future__ import annotations

import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import bundle_macos as bm


class PureHelpers(unittest.TestCase):
    def test_zip_name(self):
        self.assertEqual(bm.zip_name("ozmux", "0.1.0", "arm64"), "ozmux-0.1.0-arm64.zip")

    def test_version_less_than(self):
        self.assertTrue(bm.version_less_than("10.15", "11.0"))
        self.assertFalse(bm.version_less_than("11.0", "11.0"))
        self.assertFalse(bm.version_less_than("12.3", "11.0"))

    def test_helper_bundle_id_base(self):
        self.assertEqual(bm.helper_bundle_id("not.elm.ozmux", ""), "not.elm.ozmux.helper")

    def test_helper_bundle_id_variants(self):
        self.assertEqual(bm.helper_bundle_id("not.elm.ozmux", " (GPU)"), "not.elm.ozmux.helper.gpu")
        self.assertEqual(bm.helper_bundle_id("not.elm.ozmux", " (Renderer)"), "not.elm.ozmux.helper.renderer")
        self.assertEqual(bm.helper_bundle_id("not.elm.ozmux", " (Plugin)"), "not.elm.ozmux.helper.plugin")

    def test_config_paths(self):
        cfg = bm.BundleConfig(
            version="1.2.3", app_name="ozmux", bin_name="ozmux",
            bundle_id_base="not.elm.ozmux", arch="arm64", target_triple="aarch64-apple-darwin",
            bin_source=Path("/tmp/ozmux"), cef_framework=Path("/tmp/cef"),
            helper_bin=Path("/tmp/helper"), out_dir=Path("/tmp/out"),
            sign_identity="-", no_sign=False, notarize=False,
        )
        self.assertEqual(cfg.app_path, Path("/tmp/out/ozmux.app"))
        self.assertEqual(cfg.zip_path, Path("/tmp/out/ozmux-1.2.3-arm64.zip"))


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
        p = bm.build_helper_plist("ozmux Helper (GPU)", "not.elm.ozmux.helper.gpu")
        self.assertEqual(p["CFBundleExecutable"], "ozmux Helper (GPU)")
        self.assertEqual(p["CFBundleName"], "ozmux Helper (GPU)")
        self.assertEqual(p["CFBundleIdentifier"], "not.elm.ozmux.helper.gpu")
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


class ConfigResolution(unittest.TestCase):
    def _parse(self, argv):
        return bm.build_arg_parser().parse_args(argv)

    def test_resolve_defaults_with_explicit_version(self):
        cfg = bm.resolve_config(self._parse(["--version", "0.1.0", "--skip-build"]))
        self.assertEqual(cfg.version, "0.1.0")
        self.assertEqual(cfg.bin_name, "ozmux")
        self.assertEqual(cfg.sign_identity, "-")
        self.assertFalse(cfg.notarize)
        self.assertEqual(cfg.bin_source.name, "ozmux")
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


@unittest.skipUnless(sys.platform == "darwin", "macOS-only integration test")
class AssembleAndEmbed(unittest.TestCase):
    def _macho(self, dest: Path) -> None:
        # NOTE: shutil.copy (not copy2) avoids PermissionError from SIP-restricted flags on /usr/bin/true
        shutil.copy("/usr/bin/true", dest)
        dest.chmod(0o755)

    def _fake_cef(self, root: Path) -> Path:
        fw = root / "Chromium Embedded Framework.framework"
        (fw / "Libraries").mkdir(parents=True)
        self._macho(fw / "Chromium Embedded Framework")
        self._macho(fw / "Libraries" / "libEGL.dylib")
        (fw / "Resources").mkdir()
        (fw / "Resources" / "icudtl.dat").write_bytes(b"fake")
        return fw

    def _cfg(self, d: Path) -> "bm.BundleConfig":
        self._macho(d / "ozmux")
        self._macho(d / "helper")
        fw = self._fake_cef(d)
        return bm.BundleConfig(
            version="9.9.9", app_name="ozmux", bin_name="ozmux",
            bundle_id_base="not.elm.ozmux", arch="arm64", target_triple="aarch64-apple-darwin",
            bin_source=d / "ozmux", cef_framework=fw, helper_bin=d / "helper",
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
        self.assertTrue((contents / "MacOS" / "ozmux").is_file())
        with open(contents / "Info.plist", "rb") as f:
            plist = plistlib.load(f)
        self.assertEqual(plist["CFBundleShortVersionString"], "9.9.9")
        self.assertEqual(plist["LSEnvironment"]["MallocNanoZone"], "0")
        self.assertTrue(plist["NSSupportsAutomaticGraphicsSwitching"])
        fw = contents / "Frameworks" / "Chromium Embedded Framework.framework"
        self.assertTrue((fw / "Chromium Embedded Framework").is_file())
        for suffix, idsfx in [("", "helper"), (" (GPU)", "helper.gpu"),
                              (" (Renderer)", "helper.renderer"), (" (Plugin)", "helper.plugin")]:
            helper = contents / "Frameworks" / f"ozmux Helper{suffix}.app"
            self.assertTrue((helper / "Contents" / "MacOS" / f"ozmux Helper{suffix}").is_file())
            with open(helper / "Contents" / "Info.plist", "rb") as f:
                hp = plistlib.load(f)
            self.assertEqual(hp["CFBundleIdentifier"], f"not.elm.ozmux.{idsfx}")
            self.assertTrue(hp["LSUIElement"])


@unittest.skipUnless(sys.platform == "darwin", "macOS-only integration test")
class EndToEnd(unittest.TestCase):
    def _macho(self, dest: Path) -> None:
        # NOTE: shutil.copy (not copy2) avoids PermissionError from SIP-restricted flags on /usr/bin/true
        shutil.copy("/usr/bin/true", dest)
        dest.chmod(0o755)

    def _fake_cef(self, root: Path) -> Path:
        fw = root / "Chromium Embedded Framework.framework"
        (fw / "Libraries").mkdir(parents=True)
        self._macho(fw / "Chromium Embedded Framework")
        self._macho(fw / "Libraries" / "libEGL.dylib")
        return fw

    def test_main_adhoc_end_to_end(self):
        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            self._macho(d / "ozmux")
            self._macho(d / "helper")
            fw = self._fake_cef(d)
            out = d / "out"
            bm.main([
                "--skip-build", "--version", "9.9.9",
                "--bin", str(d / "ozmux"),
                "--cef-framework", str(fw),
                "--helper-bin", str(d / "helper"),
                "--out-dir", str(out),
            ])
            zip_path = out / "ozmux-9.9.9-arm64.zip"
            self.assertTrue(zip_path.is_file())
            sha_file = out / "ozmux-9.9.9-arm64.zip.sha256"
            self.assertTrue(sha_file.is_file())
            self.assertEqual(bm.compute_sha256(zip_path), sha_file.read_text().split()[0])
            # ad-hoc signature must verify
            subprocess.run(
                ["codesign", "--verify", "--deep", "--strict", str(out / "ozmux.app")],
                check=True,
            )


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
