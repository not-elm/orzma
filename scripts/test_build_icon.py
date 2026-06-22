"""Unit tests for the pure helpers in scripts/build_icon.py."""

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import build_icon as bi


class TestIconsetPlan(unittest.TestCase):
    def test_entries_are_apples_ten(self):
        self.assertEqual(
            bi.iconset_entries(),
            [
                ("icon_16x16.png", 16),
                ("icon_16x16@2x.png", 32),
                ("icon_32x32.png", 32),
                ("icon_32x32@2x.png", 64),
                ("icon_128x128.png", 128),
                ("icon_128x128@2x.png", 256),
                ("icon_256x256.png", 256),
                ("icon_256x256@2x.png", 512),
                ("icon_512x512.png", 512),
                ("icon_512x512@2x.png", 1024),
            ],
        )

    def test_unique_sizes_are_seven(self):
        self.assertEqual(
            bi.unique_sizes(bi.iconset_entries()),
            [16, 32, 64, 128, 256, 512, 1024],
        )


class TestArgv(unittest.TestCase):
    def test_resvg_argv_pins_font_and_skips_system(self):
        argv = bi.resvg_argv(Path("/a/in.svg"), Path("/b/out.png"), 128, Path("/f/Bold.ttf"))
        self.assertEqual(
            argv,
            [
                "resvg",
                "--skip-system-fonts",
                "--use-font-file", "/f/Bold.ttf",
                "--width", "128",
                "--height", "128",
                "/a/in.svg",
                "/b/out.png",
            ],
        )

    def test_iconutil_argv(self):
        argv = bi.iconutil_argv(Path("/t/AppIcon.iconset"), Path("/o/AppIcon.icns"))
        self.assertEqual(
            argv,
            ["iconutil", "-c", "icns", "/t/AppIcon.iconset", "-o", "/o/AppIcon.icns"],
        )


class TestPngDimensions(unittest.TestCase):
    def _png_header(self, width: int, height: int) -> bytes:
        return (
            b"\x89PNG\r\n\x1a\n"
            + b"\x00\x00\x00\x0dIHDR"
            + width.to_bytes(4, "big")
            + height.to_bytes(4, "big")
        )

    def test_parse_valid_header(self):
        self.assertEqual(bi.parse_png_dimensions(self._png_header(512, 512)), (512, 512))

    def test_parse_rejects_non_png(self):
        with self.assertRaises(ValueError):
            bi.parse_png_dimensions(b"not-a-png" + b"\x00" * 16)


if __name__ == "__main__":
    unittest.main()
