"""Structure checks for the orzma app-icon master SVG."""

import unittest
import xml.etree.ElementTree as ET
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
SVG = REPO_ROOT / "build" / "macos" / "icon" / "appicon.svg"


class TestMasterSvg(unittest.TestCase):
    def setUp(self):
        self.assertTrue(SVG.is_file(), f"missing master SVG: {SVG}")
        self.text = SVG.read_text(encoding="utf-8")

    def test_well_formed_xml(self):
        ET.fromstring(self.text)

    def test_canvas_is_1024(self):
        self.assertIn('viewBox="0 0 1024 1024"', self.text)

    def test_gradient_colors(self):
        self.assertIn('stop-color="#7c3aed"', self.text)
        self.assertIn('stop-color="#2563eb"', self.text)

    def test_glyphs_white_oz(self):
        self.assertIn('fill="#ffffff"', self.text)
        self.assertIn(">oz<", self.text)

    def test_cursor_cyan(self):
        self.assertIn('fill="#22d3ee"', self.text)

    def test_font_family_is_internal_name(self):
        self.assertIn('font-family="JetBrainsMono Nerd Font Mono"', self.text)
        self.assertIn('font-weight="700"', self.text)


if __name__ == "__main__":
    unittest.main()
