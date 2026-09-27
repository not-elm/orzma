from __future__ import annotations

import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from build_linux_icons import ICON_SIZES

REPO_ROOT = Path(__file__).resolve().parent.parent
LINUX_DIR = REPO_ROOT / "build" / "linux"


def _desktop_fields(text: str) -> dict[str, str]:
    fields = {}
    for line in text.splitlines():
        key, sep, value = line.partition("=")
        if sep and not line.startswith("["):
            fields[key] = value
    return fields


class DesktopTemplate(unittest.TestCase):
    def test_template_matches_the_window_name_and_icon(self):
        fields = _desktop_fields((LINUX_DIR / "orzma.desktop").read_text(encoding="utf-8"))
        self.assertEqual(fields["Type"], "Application")
        self.assertEqual(fields["Name"], "orzma")
        self.assertEqual(fields["Exec"], "@ORZMA_EXEC@")
        self.assertEqual(fields["Icon"], "orzma")
        self.assertEqual(fields["StartupWMClass"], "orzma")
        self.assertEqual(fields["Terminal"], "false")


class InstallScripts(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        self.home = self.root / "home"
        self.data = self.root / "data"
        self.home.mkdir()
        self.dist = self._make_dist(self.root / "orzma-0.2.0-x86_64-linux")

    def tearDown(self):
        self._tmp.cleanup()

    def _make_dist(self, dist: Path) -> Path:
        dist.mkdir(parents=True)
        for script in ("install.sh", "uninstall.sh"):
            shutil.copy2(LINUX_DIR / script, dist / script)
            (dist / script).chmod(0o755)
        (dist / "orzma").write_text("#!/bin/sh\n", encoding="utf-8")
        (dist / "orzma").chmod(0o755)
        (dist / "libcef.so").write_bytes(b"cef")
        apps = dist / "share" / "applications"
        apps.mkdir(parents=True)
        shutil.copy2(LINUX_DIR / "orzma.desktop", apps / "orzma.desktop")
        for size in ICON_SIZES:
            icon = dist / "share" / "icons" / "hicolor" / f"{size}x{size}" / "apps" / "orzma.png"
            icon.parent.mkdir(parents=True)
            icon.write_bytes(f"png{size}".encode())
        return dist

    def _run(self, script: Path, data: str | Path | None = None) -> subprocess.CompletedProcess:
        env = {
            "HOME": str(self.home),
            "XDG_DATA_HOME": str(data if data is not None else self.data),
            "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
        }
        return subprocess.run(["sh", str(script)], env=env, capture_output=True, text=True)

    def _install(self, data: str | Path | None = None) -> subprocess.CompletedProcess:
        return self._run(self.dist / "install.sh", data)

    @property
    def app_dir(self) -> Path:
        return self.data / "orzma"

    @property
    def link(self) -> Path:
        return self.home / ".local" / "bin" / "orzma"

    def test_install_places_tree_link_desktop_and_icons(self):
        result = self._install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((self.app_dir / "libcef.so").is_file())
        self.assertTrue(self.link.is_symlink())
        self.assertEqual(os.readlink(self.link), str(self.app_dir / "orzma"))
        for size in ICON_SIZES:
            icon = self.data / "icons" / "hicolor" / f"{size}x{size}" / "apps" / "orzma.png"
            self.assertEqual(icon.read_bytes(), f"png{size}".encode())

    def test_desktop_exec_points_at_installed_binary(self):
        self.assertEqual(self._install().returncode, 0)
        desktop = self.data / "applications" / "orzma.desktop"
        fields = _desktop_fields(desktop.read_text(encoding="utf-8"))
        self.assertEqual(fields["Exec"], f'"{self.app_dir / "orzma"}"')

    def test_install_path_with_space_is_quoted_in_exec(self):
        data = self.root / "my data"
        result = self._install(data)
        self.assertEqual(result.returncode, 0, result.stderr)
        fields = _desktop_fields((data / "applications" / "orzma.desktop").read_text(encoding="utf-8"))
        self.assertEqual(fields["Exec"], f'"{data / "orzma" / "orzma"}"')

    def test_install_rejects_path_the_desktop_entry_cannot_quote(self):
        for bad in ('quo"te', "dol$lar", "per%cent", "back\\slash"):
            with self.subTest(bad=bad):
                result = self._install(self.root / bad)
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse((self.root / bad / "orzma").exists())

    def test_reinstall_replaces_previous_tree(self):
        self.assertEqual(self._install().returncode, 0)
        (self.app_dir / "stale-file").write_text("old", encoding="utf-8")
        self.assertEqual(self._install().returncode, 0)
        self.assertFalse((self.app_dir / "stale-file").exists())
        self.assertTrue((self.app_dir / "orzma").is_file())

    def test_running_installed_copy_keeps_tree(self):
        self.assertEqual(self._install().returncode, 0)
        result = self._run(self.app_dir / "install.sh")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((self.app_dir / "orzma").is_file())
        self.assertTrue((self.app_dir / "libcef.so").is_file())

    def test_installed_copy_survives_rerun_with_trailing_slash_data_home(self):
        self.assertEqual(self._install().returncode, 0)
        result = self._run(self.app_dir / "install.sh", f"{self.data}/")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((self.app_dir / "orzma").is_file())
        self.assertTrue((self.app_dir / "libcef.so").is_file())

    def test_install_rejects_newline_in_path_before_copying(self):
        data = self.root / "new\nline"
        result = self._install(data)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((data / "orzma").exists())
        self.assertFalse(self.link.exists())

    def test_install_refuses_regular_file_at_link(self):
        self.link.parent.mkdir(parents=True)
        self.link.write_text("mine", encoding="utf-8")
        result = self._install()
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.link.read_text(encoding="utf-8"), "mine")
        self.assertFalse(self.app_dir.exists())

    def test_install_fails_outside_a_release_tree(self):
        (self.dist / "libcef.so").unlink()
        result = self._install()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("libcef.so", result.stderr)

    def test_uninstall_removes_only_what_install_created(self):
        config = self.home / ".config" / "orzma" / "config.toml"
        config.parent.mkdir(parents=True)
        config.write_text("x", encoding="utf-8")
        other = self.data / "applications" / "other.desktop"
        other.parent.mkdir(parents=True)
        other.write_text("x", encoding="utf-8")
        self.assertEqual(self._install().returncode, 0)
        result = self._run(self.app_dir / "uninstall.sh")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(self.app_dir.exists())
        self.assertFalse(self.link.exists())
        self.assertFalse((self.data / "applications" / "orzma.desktop").exists())
        for size in ICON_SIZES:
            self.assertFalse(
                (self.data / "icons" / "hicolor" / f"{size}x{size}" / "apps" / "orzma.png").exists()
            )
        self.assertTrue(config.is_file())
        self.assertTrue(other.is_file())

    def test_uninstall_keeps_foreign_symlink(self):
        self.assertEqual(self._install().returncode, 0)
        self.link.unlink()
        self.link.symlink_to("/usr/bin/true")
        result = self._run(self.dist / "uninstall.sh")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(self.link.is_symlink())


if __name__ == "__main__":
    unittest.main()
