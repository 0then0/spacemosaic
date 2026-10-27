"""Regression checks for signing separation, versioning, and artifact promotion."""

import importlib.util
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location(
    "distribution", ROOT / "scripts/android_distribution.py"
)
distribution = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(distribution)
CERT = "ab" * 32
RESOURCES = "spec resource 0x7f030000 dev.spacemosaic:mipmap/ic_launcher: flags=0"
MANIFEST = (
    "A: android:icon(0x01010002)=@0x7f030000\n"
    "A: android:roundIcon(0x0101052c)=@0x7f030000\n"
)


class AndroidDistributionTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        (self.root / "mobile/android").mkdir(parents=True)
        (self.root / "Cargo.toml").write_text('[package]\nversion = "1.2.3-alpha.1"\n')
        (self.root / "mobile/android/gradle.properties").write_text(
            "spacemosaicVersionCode=42\n"
        )
        key = self.root / "test.keystore"
        key.touch()
        self.env = {
            "SPACEMOSAIC_ANDROID_" + k: v
            for k, v in {
                "KEYSTORE": str(key),
                "STORE_PASSWORD": "never-print-this-password",
                "KEY_ALIAS": "test",
                "KEY_PASSWORD": "never-print-this-password",
                "CERT_SHA256": CERT,
                "PREVIOUS_VERSION_CODE": "41",
            }.items()
        }
        self.apk = self.root / "app.apk"
        with zipfile.ZipFile(self.apk, "w") as archive:
            for abi in ("arm64-v8a", "x86_64"):
                archive.writestr(f"lib/{abi}/libspacemosaic.so", b"fixture")
            archive.writestr(
                "assets/licenses/THIRD_PARTY_NOTICES.txt", "SpaceMosaic v1.2.3-alpha.1"
            )
        self.badging = (
            "package: name='dev.spacemosaic' versionCode='42' versionName='1.2.3-alpha.1'\n"
            "application: label='SpaceMosaic' icon='res/mipmap/ic_launcher.png'\n"
        )

    def test_debug_requires_no_credentials(self):
        distribution.preflight("debug", {}, self.root)

    def test_release_requires_every_input_and_never_prints_values(self):
        for key in self.env:
            env = self.env.copy()
            del env[key]
            with self.subTest(key=key), self.assertRaises(ValueError) as error:
                distribution.preflight("release", env, self.root)
            self.assertIn(key, str(error.exception))
            self.assertNotIn("never-print-this-password", str(error.exception))

    def test_version_counter_must_increase(self):
        for previous in ("42", "43", "-1", "not-an-integer"):
            with self.subTest(previous=previous), self.assertRaises(ValueError):
                distribution.preflight(
                    "release",
                    {**self.env, "SPACEMOSAIC_ANDROID_PREVIOUS_VERSION_CODE": previous},
                    self.root,
                )
        distribution.preflight("release", self.env, self.root)

    def test_invalid_certificate_and_missing_keystore(self):
        for key, value in (
            ("CERT_SHA256", "not-a-fingerprint"),
            ("KEYSTORE", str(self.root / "missing")),
        ):
            with self.subTest(key=key), self.assertRaises(ValueError):
                distribution.preflight(
                    "release",
                    {**self.env, "SPACEMOSAIC_ANDROID_" + key: value},
                    self.root,
                )

    def verify(self, badging=None, cert=CERT):
        with patch.object(
            distribution.subprocess,
            "check_output",
            side_effect=[
                f"Signer #1 certificate SHA-256 digest: {cert}\n",
                self.badging if badging is None else badging,
                RESOURCES,
                MANIFEST,
            ],
        ):
            return distribution.verify(
                self.apk, "release", self.root, self.env, self.root
            )

    def test_verified_apk_records_public_identity(self):
        metadata = self.verify()
        self.assertEqual(metadata["certificateSha256"], CERT)
        self.assertEqual(metadata["versionCode"], 42)
        self.assertNotIn("never-print-this-password", str(metadata))
        self.assertEqual(len(metadata["apkSha256"]), 64)

    def test_wrong_key_and_multiple_signers_are_rejected(self):
        for cert in (
            "cd" * 32,
            CERT + "\nSigner #2 certificate SHA-256 digest: " + CERT,
        ):
            with self.subTest(cert=cert), self.assertRaises(ValueError):
                self.verify(cert=cert)

    def test_wrong_package_version_or_debuggable_release_is_rejected(self):
        for badging in (
            self.badging.replace("dev.spacemosaic", "dev.other"),
            self.badging.replace("'42'", "'41'"),
            self.badging.replace("1.2.3-alpha.1", "1.2.2"),
            self.badging + "application-debuggable\n",
        ):
            with self.subTest(badging=badging), self.assertRaises(ValueError):
                self.verify(badging)

    def test_optimized_release_icon_paths_are_accepted(self):
        self.verify(self.badging.replace("res/mipmap/ic_launcher.png", "res/IG.xml"))

    def test_wrong_manifest_icon_is_rejected(self):
        with (
            patch.object(
                distribution.subprocess,
                "check_output",
                side_effect=[
                    f"Signer #1 certificate SHA-256 digest: {CERT}\n",
                    self.badging,
                    RESOURCES,
                    MANIFEST.replace("@0x7f030000", "@0x01000000"),
                ],
            ),
            self.assertRaisesRegex(ValueError, "launcher icon"),
        ):
            distribution.verify(self.apk, "release", self.root, self.env, self.root)

    def test_invalid_apk_signature_is_rejected(self):
        with (
            patch.object(
                distribution.subprocess,
                "check_output",
                side_effect=subprocess.CalledProcessError(1, "apksigner"),
            ),
            self.assertRaises(subprocess.CalledProcessError),
        ):
            distribution.verify(self.apk, "release", self.root, self.env, self.root)

    @unittest.skipIf(
        os.name == "nt", "POSIX build script is exercised by Android CI on Linux"
    )
    def test_script_promotes_only_a_verified_release(self):
        # Fake the compiler/SDK boundary, exercise the real shell + validator pipeline.
        scripts = self.root / "scripts"
        scripts.mkdir()
        for name in ("build-android.sh", "android_distribution.py"):
            shutil.copyfile(ROOT / "scripts" / name, scripts / name)
        (self.root / "LICENSE").write_text("fixture")
        (self.root / "THIRD_PARTY_NOTICES.txt").write_text("SpaceMosaic v1.2.3-alpha.1")
        fonts = self.root / "assets/fonts"
        fonts.mkdir(parents=True)
        (fonts / "OFL.txt").write_text("fixture")
        bin_dir = self.root / "bin"
        bin_dir.mkdir()
        tools = self.root / "sdk/build-tools/36.0.0"
        tools.mkdir(parents=True)
        (self.root / "sdk/platforms/android-36").mkdir(parents=True)

        def executable(path, body):
            path.write_text("#!" + sys.executable + "\n" + body)
            path.chmod(0o755)

        executable(bin_dir / "cargo-ndk", "")
        executable(bin_dir / "cargo", "")
        executable(
            tools / "apksigner",
            f"print('Signer #1 certificate SHA-256 digest: {CERT}')\n",
        )
        executable(
            tools / "aapt",
            "import sys\nprint("
            + repr(self.badging)
            + " if sys.argv[2] == 'badging' else "
            + repr(RESOURCES)
            + " if sys.argv[2] == 'resources' else "
            + repr(MANIFEST)
            + ")\n",
        )
        gradle = self.root / "mobile/android/gradlew"
        executable(
            gradle,
            """import pathlib, shutil
p = pathlib.Path('mobile/android/app/build/outputs/apk/release/app-release.apk')
p.parent.mkdir(parents=True, exist_ok=True)
shutil.copyfile('app.apk', p)
""",
        )
        env = {
            **os.environ,
            **self.env,
            "ANDROID_HOME": str(self.root / "sdk"),
            "PATH": str(bin_dir) + os.pathsep + os.environ["PATH"],
        }
        command = ["bash", str(scripts / "build-android.sh"), "release"]
        result = subprocess.run(
            command, env=env, capture_output=True, text=True, check=False
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        artifact = self.root / "dist/spacemosaic-release.apk"
        self.assertEqual(artifact.read_bytes(), self.apk.read_bytes())
        self.assertTrue((self.root / "dist/android-SHA256SUMS").is_file())
        artifact.unlink()
        env["SPACEMOSAIC_ANDROID_CERT_SHA256"] = "cd" * 32
        result = subprocess.run(
            command, env=env, capture_output=True, text=True, check=False
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("does not match", result.stderr)
        self.assertFalse(artifact.exists())
        self.assertNotIn("never-print-this-password", result.stdout + result.stderr)

    @unittest.skipIf(
        os.name == "nt", "POSIX build script is exercised by Android CI on Linux"
    )
    def test_missing_credentials_fail_before_sdk_or_cargo(self):
        env = {
            k: v
            for k, v in os.environ.items()
            if not k.startswith("SPACEMOSAIC_ANDROID_")
        }
        env.pop("ANDROID_HOME", None)
        result = subprocess.run(
            ["bash", "scripts/build-android.sh", "release"],
            cwd=ROOT,
            env=env,
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Release signing requires", result.stderr)
        self.assertNotIn("Set ANDROID_HOME", result.stderr)


if __name__ == "__main__":
    unittest.main()
