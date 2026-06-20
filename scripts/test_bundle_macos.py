from __future__ import annotations

import os
import sys
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
            version="1.2.3", app_name="ozmux", bin_name="ozmux-gui",
            bundle_id_base="not.elm.ozmux", arch="arm64", target_triple="aarch64-apple-darwin",
            bin_source=Path("/tmp/ozmux-gui"), cef_framework=Path("/tmp/cef"),
            helper_bin=Path("/tmp/helper"), out_dir=Path("/tmp/out"),
            sign_identity="-", no_sign=False, notarize=False,
        )
        self.assertEqual(cfg.app_path, Path("/tmp/out/ozmux.app"))
        self.assertEqual(cfg.zip_path, Path("/tmp/out/ozmux-1.2.3-arm64.zip"))


if __name__ == "__main__":
    unittest.main()
