"""Verifies the bundler wires the committed AppIcon.icns into the .app."""

import plistlib
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import bundle_macos as bm

ICNS = bm.REPO_ROOT / "build" / "macos" / "AppIcon.icns"


class TestBundleIconWiring(unittest.TestCase):
    def test_committed_icns_exists(self):
        self.assertTrue(ICNS.is_file(), f"missing committed icns: {ICNS}")

    def test_assemble_app_sets_icon_file_and_copies_icns(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            fake_bin = root / "ozmux"
            fake_bin.write_bytes(b"\x7fELF fake binary")
            args = bm.build_arg_parser().parse_args(
                [
                    "--version", "9.9.9",
                    "--bin", str(fake_bin),
                    "--no-sign",
                    "--out-dir", str(root / "out"),
                ]
            )
            cfg = bm.resolve_config(args)
            bm.assemble_app(cfg)
            info = plistlib.loads((cfg.app_path / "Contents" / "Info.plist").read_bytes())
            self.assertEqual(info.get("CFBundleIconFile"), "AppIcon.icns")
            self.assertTrue(
                (cfg.app_path / "Contents" / "Resources" / "AppIcon.icns").is_file()
            )


if __name__ == "__main__":
    unittest.main()
