from __future__ import annotations

import contextlib
import io
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

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


class Main(unittest.TestCase):
    def test_writes_lf_line_endings_on_every_host(self):
        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            Assemble()._fixture_dir(d)
            out = d / "THIRD-PARTY-LICENSES.md"
            with mock.patch.object(gl, "run_cargo_about", return_value="RUST\n"), \
                    mock.patch.object(gl, "run_pnpm_licenses", return_value=[]), \
                    mock.patch.object(gl, "LICENSES_DIR", d), \
                    mock.patch.object(gl, "REPO_ROOT", d), \
                    mock.patch.object(gl, "OUTPUT_PATH", out), \
                    contextlib.redirect_stdout(io.StringIO()):
                gl.main([])
            self.assertNotIn(b"\r\n", out.read_bytes())


class CargoAboutArgv(unittest.TestCase):
    def test_writes_to_an_output_file_instead_of_stdout(self):
        argv = gl.cargo_about_argv(Path("about.hbs"), Path("out.md"))
        self.assertEqual(argv[:3], ["cargo", "about", "generate"])
        self.assertIn("--output-file", argv)
        self.assertEqual(argv[argv.index("--output-file") + 1], "out.md")
        self.assertEqual(argv[-1], "about.hbs")


class ResolveProgram(unittest.TestCase):
    def test_returns_the_path_found_on_path(self):
        with mock.patch.object(gl.shutil, "which", return_value=r"C:\npm\pnpm.CMD"):
            self.assertEqual(gl.resolve_program("pnpm"), r"C:\npm\pnpm.CMD")

    def test_missing_program_exits_with_its_name(self):
        with mock.patch.object(gl.shutil, "which", return_value=None):
            with self.assertRaises(SystemExit) as ctx:
                gl.resolve_program("pnpm")
        self.assertIn("pnpm", str(ctx.exception))


if __name__ == "__main__":
    unittest.main()
