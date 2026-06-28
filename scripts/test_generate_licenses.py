from __future__ import annotations

import os
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import generate_licenses as gl


class RenderNpm(unittest.TestCase):
    def test_sorts_by_name_and_includes_text(self):
        entries = [
            {"name": "b-pkg", "version": "2.0.0", "license": "MIT", "licenseText": "MIT TEXT B"},
            {"name": "a-pkg", "version": "1.0.0", "license": "ISC", "licenseText": "ISC TEXT A"},
        ]
        out = gl.render_npm_section(entries)
        self.assertTrue(out.startswith("## npm packages"))
        self.assertLess(out.index("a-pkg"), out.index("b-pkg"))
        self.assertIn("ISC TEXT A", out)
        self.assertIn("MIT TEXT B", out)

    def test_missing_text_falls_back_to_marker(self):
        entries = [{"name": "x", "version": "1.0.0", "license": "MIT",
                    "licenseText": None, "homepage": "https://example.test"}]
        out = gl.render_npm_section(entries)
        self.assertIn("x 1.0.0", out)
        self.assertIn("https://example.test", out)


class RenderFonts(unittest.TestCase):
    def test_reads_vendor_dirs_sorted_and_excludes_chromium(self):
        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            (d / "alpha").mkdir()
            (d / "alpha" / "LICENSE").write_text("ALPHA LIC", encoding="utf-8")
            (d / "beta").mkdir()
            (d / "beta" / "OFL.txt").write_text("BETA OFL", encoding="utf-8")
            (d / "chromium").mkdir()
            (d / "chromium" / "LICENSE.txt").write_text("CEF SHOULD NOT APPEAR", encoding="utf-8")
            out = gl.render_fonts_section(d)
            self.assertIn("ALPHA LIC", out)
            self.assertIn("BETA OFL", out)
            self.assertNotIn("CEF SHOULD NOT APPEAR", out)
            self.assertLess(out.index("alpha"), out.index("beta"))


class RenderChromium(unittest.TestCase):
    def test_includes_cef_text_and_credits_pointer(self):
        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            (d / "chromium").mkdir()
            (d / "chromium" / "LICENSE.txt").write_text("CEF BSD LICENSE", encoding="utf-8")
            out = gl.render_chromium_section(d)
            self.assertIn("CEF BSD LICENSE", out)
            self.assertIn("CREDITS.html", out)


class Assemble(unittest.TestCase):
    def _fixture_dir(self, d: Path) -> None:
        (d / "font1").mkdir()
        (d / "font1" / "OFL.txt").write_text("FONT1 TEXT", encoding="utf-8")
        (d / "chromium").mkdir()
        (d / "chromium" / "LICENSE.txt").write_text("CEF BSD", encoding="utf-8")

    def test_deterministic_and_ordered(self):
        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            self._fixture_dir(d)
            npm = [{"name": "pkg", "version": "1.0.0", "license": "MIT", "licenseText": "PKG MIT"}]
            rust = "### some-crate — `MIT`\n\nUsed by:\n- some-crate 1.0\n\n~~~text\nRUST TEXT\n~~~\n"
            a = gl.assemble(rust, npm, d)
            b = gl.assemble(rust, npm, d)
            self.assertEqual(a, b)
            self.assertTrue(a.startswith("# Third-Party Licenses"))
            self.assertTrue(a.endswith("\n"))
            self.assertLess(a.index("Rust crates"), a.index("npm packages"))
            self.assertLess(a.index("npm packages"), a.index("FONT1 TEXT"))
            self.assertLess(a.index("FONT1 TEXT"), a.index("CEF BSD"))


if __name__ == "__main__":
    unittest.main()
