from __future__ import annotations

import os
import stat
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import package_deb as pd
import stage_linux as sl

RUNTIME_BINS = ("orzma", "orzmd", "orzbrowser", "bevy_cef_render_process")


def _write(path: Path, data: bytes = b"") -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)


def _make_stage_tree(root: Path, version: str = "0.2.0") -> Path:
    tree = root / "stage" / sl.dist_name(version)
    for name in RUNTIME_BINS:
        _write(tree / name, b"\x7fELF" + name.encode())
        (tree / name).chmod(0o700)
    _write(tree / "libcef.so", b"\x7fELFcef")
    (tree / "libcef.so").chmod(0o600)
    _write(tree / "locales" / "ja.pak", b"ja")
    _write(tree / "LICENSE", b"MIT License\n\nCopyright (c) notelm\n")
    _write(tree / "THIRD-PARTY-LICENSES.md", b"third party")
    _write(tree / "chromium" / "CREDITS.html", b"credits")
    for script in ("install.sh", "uninstall.sh"):
        _write(tree / script, b"#!/bin/sh\n")
        (tree / script).chmod(0o755)
    _write(
        tree / "share" / "applications" / "orzma.desktop",
        b"[Desktop Entry]\nType=Application\nExec=@ORZMA_EXEC@\nIcon=orzma\n",
    )
    _write(tree / "share" / "icons" / "hicolor" / "48x48" / "apps" / "orzma.png", b"png")
    return tree


def _control_fields(text: str) -> dict[str, str]:
    fields: dict[str, str] = {}
    key = ""
    for line in text.splitlines():
        if line.startswith(" "):
            fields[key] += "\n" + line
        else:
            key, _, value = line.partition(": ")
            fields[key] = value
    return fields


class DebVersion(unittest.TestCase):
    def test_release_version_is_unchanged(self):
        self.assertEqual(pd.deb_version("0.2.0"), "0.2.0")

    def test_prerelease_hyphen_becomes_tilde(self):
        self.assertEqual(pd.deb_version("0.2.0-rc.1"), "0.2.0~rc.1")

    def test_build_metadata_plus_is_kept(self):
        self.assertEqual(pd.deb_version("0.2.0+g1ce985c"), "0.2.0+g1ce985c")

    def test_invalid_versions_are_rejected(self):
        for bad in ("", "v0.2.0", "0.2.0 beta", "0.2.0_1", ".2.0", "0.2.0\n"):
            with self.subTest(bad=bad), self.assertRaises(SystemExit):
                pd.deb_version(bad)


class DebFileName(unittest.TestCase):
    def test_file_name_keeps_the_unmapped_version(self):
        self.assertEqual(pd.deb_file_name("0.2.0-rc.1"), "orzma_0.2.0-rc.1_amd64.deb")

    def test_file_name_rejects_invalid_version(self):
        with self.assertRaises(SystemExit):
            pd.deb_file_name("v0.2.0")


class Control(unittest.TestCase):
    def test_required_fields_are_rendered(self):
        fields = _control_fields(pd.render_control("0.2.0-dev", 1234))
        self.assertEqual(fields["Package"], "orzma")
        self.assertEqual(fields["Version"], "0.2.0~dev")
        self.assertEqual(fields["Architecture"], "amd64")
        self.assertEqual(fields["Section"], "x11")
        self.assertEqual(fields["Priority"], "optional")
        self.assertEqual(fields["Maintainer"], "notelm <notelm@users.noreply.github.com>")
        self.assertEqual(fields["Homepage"], "https://github.com/not-elm/orzma")
        self.assertEqual(fields["Installed-Size"], "1234")
        self.assertEqual(fields["Recommends"], "mesa-vulkan-drivers | vulkan-icd")
        self.assertEqual(fields["Description"], f"Terminal emulator with in-process webviews\n {pd.LONG_DESCRIPTION}")

    def test_depends_covers_glibc_floor_alsa_rename_and_dlopen_libraries(self):
        depends = [d.strip() for d in _control_fields(pd.render_control("0.2.0", 1))["Depends"].split(",")]
        self.assertEqual(depends, list(pd.DEPENDS))
        for expected in ("libc6 (>= 2.35)", "libasound2t64 | libasound2", "libxkbcommon-x11-0",
                         "libx11-xcb1", "libvulkan1", "libegl1"):
            self.assertIn(expected, depends)

    def test_control_ends_with_a_newline(self):
        self.assertTrue(pd.render_control("0.2.0", 1).endswith("\n"))


class InstalledSize(unittest.TestCase):
    def test_size_rounds_files_up_and_counts_dirs_and_symlinks_once(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            _write(root / "a" / "one", b"x")
            _write(root / "a" / "kib", b"x" * 1024)
            _write(root / "a" / "more", b"x" * (1024 * 1024 + 1))
            os.symlink("a/more", root / "link")
            # one: 1, kib: 1, more: 1025, dir a: 1, link: 1
            self.assertEqual(pd.installed_size_kib(root), 1 + 1 + 1025 + 1 + 1)


class AssembleDebRoot(unittest.TestCase):
    def _assemble(self, tmp: str) -> Path:
        tree = _make_stage_tree(Path(tmp))
        root = Path(tmp) / "root"
        root.mkdir()
        pd.assemble_deb_root(tree, root)
        return root

    def test_runtime_entries_land_flat_under_usr_lib_orzma(self):
        with tempfile.TemporaryDirectory() as tmp:
            lib = self._assemble(tmp) / "usr" / "lib" / "orzma"
            for name in (*RUNTIME_BINS, "libcef.so", "locales/ja.pak"):
                self.assertTrue((lib / name).is_file(), name)
            for name in ("install.sh", "uninstall.sh", "share", "LICENSE",
                         "THIRD-PARTY-LICENSES.md", "chromium"):
                self.assertFalse((lib / name).exists(), name)

    def test_license_files_move_to_usr_share_doc(self):
        with tempfile.TemporaryDirectory() as tmp:
            doc = self._assemble(tmp) / "usr" / "share" / "doc" / "orzma"
            copyright_text = (doc / "copyright").read_text()
            self.assertIn("https://github.com/not-elm/orzma", copyright_text)
            self.assertIn("MIT License", copyright_text)
            self.assertEqual((doc / "THIRD-PARTY-LICENSES.md").read_bytes(), b"third party")
            self.assertEqual((doc / "chromium" / "CREDITS.html").read_bytes(), b"credits")

    def test_launchers_are_relative_symlinks_into_usr_lib_orzma(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = self._assemble(tmp)
            for name in ("orzma", "orzmd", "orzbrowser"):
                link = root / "usr" / "bin" / name
                self.assertTrue(link.is_symlink(), name)
                self.assertEqual(os.readlink(link), f"../lib/orzma/{name}")
                self.assertEqual(link.resolve(), (root / "usr" / "lib" / "orzma" / name).resolve())

    def test_desktop_entry_execs_the_launcher(self):
        with tempfile.TemporaryDirectory() as tmp:
            desktop = (self._assemble(tmp) / "usr" / "share" / "applications" / "orzma.desktop").read_text()
            self.assertIn("Exec=/usr/bin/orzma\n", desktop)
            self.assertNotIn("@ORZMA_EXEC@", desktop)

    def test_icons_are_installed_into_hicolor(self):
        with tempfile.TemporaryDirectory() as tmp:
            icon = self._assemble(tmp) / "usr" / "share" / "icons" / "hicolor" / "48x48" / "apps" / "orzma.png"
            self.assertEqual(icon.read_bytes(), b"png")

    def test_missing_launcher_binary_is_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            tree = _make_stage_tree(Path(tmp))
            (tree / "orzmd").unlink()
            root = Path(tmp) / "root"
            root.mkdir()
            with self.assertRaises(SystemExit) as ctx:
                pd.assemble_deb_root(tree, root)
            self.assertIn("orzmd", str(ctx.exception))

    def test_desktop_template_without_placeholder_is_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            tree = _make_stage_tree(Path(tmp))
            _write(tree / "share" / "applications" / "orzma.desktop", b"[Desktop Entry]\nExec=orzma\n")
            root = Path(tmp) / "root"
            root.mkdir()
            with self.assertRaises(SystemExit) as ctx:
                pd.assemble_deb_root(tree, root)
            self.assertIn("@ORZMA_EXEC@", str(ctx.exception))


class NormalizeModes(unittest.TestCase):
    def test_modes_follow_the_tarball_rules(self):
        with tempfile.TemporaryDirectory() as tmp:
            tree = _make_stage_tree(Path(tmp))
            root = Path(tmp) / "root"
            root.mkdir()
            pd.assemble_deb_root(tree, root)
            pd.normalize_modes(root)
            lib = root / "usr" / "lib" / "orzma"
            self.assertEqual(stat.S_IMODE((lib / "orzma").stat().st_mode), 0o755)
            self.assertEqual(stat.S_IMODE((lib / "libcef.so").stat().st_mode), 0o644)
            self.assertEqual(stat.S_IMODE((lib / "locales").stat().st_mode), 0o755)
            self.assertTrue((root / "usr" / "bin" / "orzma").is_symlink())

    def test_symlinks_are_neither_followed_nor_replaced(self):
        with tempfile.TemporaryDirectory() as tmp, tempfile.TemporaryDirectory() as outside:
            target = Path(outside) / "target"
            _write(target, b"x")
            target.chmod(0o600)
            root = Path(tmp)
            os.symlink(target, root / "link")
            pd.normalize_modes(root)
            self.assertTrue((root / "link").is_symlink())
            self.assertEqual(stat.S_IMODE(target.stat().st_mode), 0o600)


if __name__ == "__main__":
    unittest.main()
