from __future__ import annotations

import os
import sys
import unittest
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import build_linux_icons as bli
from build_icon import png_dimensions


class IconPaths(unittest.TestCase):
    def test_icon_path_names_the_file_by_size(self):
        self.assertEqual(bli.icon_path(Path("/out"), 48), Path("/out/orzma-48.png"))

    def test_sizes_cover_the_hicolor_sizes_the_installer_places(self):
        self.assertEqual(bli.ICON_SIZES, (48, 128, 256, 512))

    def test_render_argv_writes_each_size_to_its_icon_path(self):
        svg, font, out = Path("/m/appicon.svg"), Path("/f/font.ttf"), Path("/out")
        self.assertEqual(
            bli.render_argv(svg, out, 128, font),
            ["resvg", "--skip-system-fonts", "--use-font-file", "/f/font.ttf",
             "--width", "128", "--height", "128", "/m/appicon.svg", "/out/orzma-128.png"],
        )


class CommittedIcons(unittest.TestCase):
    def test_every_size_is_committed_at_its_dimensions(self):
        for size in bli.ICON_SIZES:
            path = bli.icon_path(bli.DEFAULT_OUT_DIR, size)
            with self.subTest(size=size):
                self.assertTrue(path.is_file(), path)
                self.assertEqual(png_dimensions(path), (size, size))


if __name__ == "__main__":
    unittest.main()
