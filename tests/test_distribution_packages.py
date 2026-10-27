"""Validate packages, using disposable destinations for installation checks."""

import hashlib
import os
import pathlib
import platform
import struct
import subprocess
import tarfile
import tempfile
import time
import unittest
import zipfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
SYSTEM = platform.system().lower()
NAME = f"spacemosaic-{SYSTEM}-{platform.machine().lower()}"
DIST = ROOT / "dist"


class PortablePackageTests(unittest.TestCase):
    def test_archive_contains_executable_and_complete_notices(self):
        with zipfile.ZipFile(DIST / f"{NAME}.zip") as archive:
            self.assertIsNone(archive.testzip())
            executable = (
                "SpaceMosaic.app/Contents/MacOS/spacemosaic"
                if SYSTEM == "darwin"
                else "spacemosaic.exe"
                if SYSTEM == "windows"
                else "spacemosaic"
            )
            self.assertGreater(archive.getinfo(f"{NAME}/{executable}").file_size, 0)
            for file in ["LICENSE", "THIRD_PARTY_NOTICES.txt", "README.md"]:
                self.assertEqual(
                    archive.read(f"{NAME}/{file}"), (ROOT / file).read_bytes()
                )
            for member in archive.namelist():
                self.assertFalse(member.startswith("/"))
                self.assertNotIn("..", pathlib.PurePosixPath(member).parts)

    def test_manifest_covers_every_platform_format_and_matches_content(self):
        expected = {f"{NAME}.zip"}
        if SYSTEM == "darwin":
            expected.add(f"{NAME}.dmg")
        elif SYSTEM == "windows":
            expected.add(f"{NAME}-setup.exe")
        elif SYSTEM == "linux":
            expected.update({f"{NAME}.deb", f"{NAME}.tar.gz"})
        manifest = (DIST / f"{NAME}-SHA256SUMS").read_text().splitlines()
        actual = set()
        for line in manifest:
            digest, filename = line.split()
            actual.add(filename)
            self.assertEqual(
                hashlib.sha256((DIST / filename).read_bytes()).hexdigest(), digest
            )
        self.assertEqual(actual, expected)
        self.assertEqual(len(manifest), len(expected))


@unittest.skipUnless(SYSTEM == "darwin", "macOS disk image validation")
class DiskImageTests(unittest.TestCase):
    def test_disk_image_installs_the_signed_app_with_its_notices(self):
        with tempfile.TemporaryDirectory(prefix="spacemosaic-dmg-test-") as temporary:
            mount = pathlib.Path(temporary) / "mount"
            mount.mkdir()
            subprocess.run(
                [
                    "hdiutil",
                    "attach",
                    "-readonly",
                    "-nobrowse",
                    "-mountpoint",
                    str(mount),
                    str(DIST / f"{NAME}.dmg"),
                ],
                check=True,
                stdout=subprocess.DEVNULL,
            )
            try:
                self.assertTrue((mount / "Applications").is_symlink())
                self.assertEqual(os.readlink(mount / "Applications"), "/Applications")
                application = mount / "SpaceMosaic.app"
                subprocess.run(
                    ["codesign", "--verify", "--strict", str(application)], check=True
                )
                notices = (
                    application / "Contents/Resources/licenses/THIRD_PARTY_NOTICES.txt"
                )
                self.assertEqual(
                    notices.read_bytes(),
                    (ROOT / "THIRD_PARTY_NOTICES.txt").read_bytes(),
                )
                self.assertTrue(
                    (
                        application / "Contents/Resources/licenses/fonts/Inter-OFL.txt"
                    ).is_file()
                )
            finally:
                subprocess.run(
                    ["hdiutil", "detach", str(mount)],
                    check=True,
                    stdout=subprocess.DEVNULL,
                )


@unittest.skipUnless(SYSTEM == "linux", "Linux distribution validation")
class LinuxPackageTests(unittest.TestCase):
    def test_tar_preserves_executable_permissions_and_license_files(self):
        with tarfile.open(DIST / f"{NAME}.tar.gz") as archive:
            binary = archive.getmember(f"{NAME}/spacemosaic")
            self.assertTrue(binary.mode & 0o111)
            self.assertEqual(
                archive.extractfile(binary).read(),
                (ROOT / "target/release/spacemosaic").read_bytes(),
            )
            self.assertEqual(
                archive.extractfile(f"{NAME}/THIRD_PARTY_NOTICES.txt").read(),
                (ROOT / "THIRD_PARTY_NOTICES.txt").read_bytes(),
            )

    def test_deb_has_installation_paths_release_version_and_root_ownership(self):
        deb = DIST / f"{NAME}.deb"
        version = subprocess.check_output(
            ["dpkg-deb", "-f", str(deb), "Version"], text=True
        ).strip()
        self.assertEqual(version, "0.1.0")
        subprocess.run(
            ["dpkg", "--compare-versions", version, "eq", "0.1.0"], check=True
        )
        with tempfile.TemporaryDirectory(prefix="spacemosaic-deb-test-") as temporary:
            subprocess.run(["dpkg-deb", "-x", str(deb), temporary], check=True)
            extracted = pathlib.Path(temporary)
            binary = extracted / "usr/bin/spacemosaic"
            self.assertTrue(binary.stat().st_mode & 0o111)
            self.assertEqual(
                binary.read_bytes(), (ROOT / "target/release/spacemosaic").read_bytes()
            )
            self.assertIn(
                "Exec=spacemosaic",
                (extracted / "usr/share/applications/spacemosaic.desktop").read_text(),
            )
            self.assertTrue(
                (
                    extracted / "usr/share/icons/hicolor/scalable/apps/spacemosaic.svg"
                ).is_file()
            )
            self.assertEqual(
                (
                    extracted / "usr/share/doc/spacemosaic/THIRD_PARTY_NOTICES.txt"
                ).read_bytes(),
                (ROOT / "THIRD_PARTY_NOTICES.txt").read_bytes(),
            )
        with tempfile.TemporaryFile() as payload:
            subprocess.run(
                ["dpkg-deb", "--fsys-tarfile", str(deb)], stdout=payload, check=True
            )
            payload.seek(0)
            with tarfile.open(fileobj=payload) as archive:
                self.assertTrue(
                    all(
                        member.uid == 0 and member.gid == 0
                        for member in archive.getmembers()
                    )
                )


@unittest.skipUnless(SYSTEM == "windows", "Windows setup validation")
class WindowsPackageTests(unittest.TestCase):
    def test_release_executable_uses_gui_subsystem_without_a_console(self):
        executable = (ROOT / "target/release/spacemosaic.exe").read_bytes()
        pe = struct.unpack_from("<I", executable, 0x3C)[0]
        self.assertEqual(executable[pe : pe + 4], b"PE\0\0")
        self.assertEqual(struct.unpack_from("<H", executable, pe + 24 + 68)[0], 2)

    def test_setup_installs_shortcuts_and_uninstalls_only_its_own_files(self):
        import winreg

        with tempfile.TemporaryDirectory(prefix="spacemosaic-setup-test-") as temporary:
            install = pathlib.Path(temporary) / "Application"
            group = "SpaceMosaic CI " + pathlib.Path(temporary).name
            shortcut = (
                pathlib.Path(os.environ["APPDATA"])
                / "Microsoft/Windows/Start Menu/Programs"
                / group
                / "SpaceMosaic.lnk"
            )
            uninstall_key = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{B01C9932-CC90-4C94-B525-C1B0F43DF495}_is1"
            try:
                subprocess.run(
                    [
                        str(DIST / f"{NAME}-setup.exe"),
                        "/VERYSILENT",
                        "/SUPPRESSMSGBOXES",
                        "/NORESTART",
                        "/SP-",
                        f"/DIR={install}",
                        f"/GROUP={group}",
                        f"/LOG={temporary}/install.log",
                    ],
                    check=True,
                )
                self.assertEqual(
                    (install / "spacemosaic.exe").read_bytes(),
                    (ROOT / "target/release/spacemosaic.exe").read_bytes(),
                )
                self.assertTrue(shortcut.is_file())
                self.assertTrue((install / "SpaceMosaic.ico").is_file())
                self.assertTrue((install / "THIRD_PARTY_NOTICES.txt").is_file())
                with winreg.OpenKey(winreg.HKEY_CURRENT_USER, uninstall_key) as key:
                    self.assertEqual(
                        winreg.QueryValueEx(key, "DisplayVersion")[0], "0.1.0"
                    )
                sentinel = install / "user-owned.txt"
                sentinel.write_text("Keep this unrelated file")
            finally:
                uninstaller = install / "unins000.exe"
                if uninstaller.exists():
                    subprocess.run(
                        [
                            str(uninstaller),
                            "/VERYSILENT",
                            "/SUPPRESSMSGBOXES",
                            "/NORESTART",
                        ],
                        check=True,
                    )
                    for _ in range(100):
                        if not (install / "spacemosaic.exe").exists():
                            break
                        time.sleep(0.1)
            self.assertFalse((install / "spacemosaic.exe").exists())
            self.assertFalse(shortcut.exists())
            self.assertEqual(sentinel.read_text(), "Keep this unrelated file")
            with self.assertRaises(FileNotFoundError):
                winreg.OpenKey(winreg.HKEY_CURRENT_USER, uninstall_key)
