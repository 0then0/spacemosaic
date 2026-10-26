"""Validate the actual packaged macOS artifact, including its icon resource seal."""
import pathlib
import platform
import plistlib
import struct
import subprocess
import unittest
import zipfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
NAME = f"spacemosaic-darwin-{platform.machine().lower()}"
BUNDLE = ROOT / "dist" / NAME / "SpaceMosaic.app"


@unittest.skipUnless(platform.system() == "Darwin", "macOS bundle validation")
class MacOSBundleTests(unittest.TestCase):
    def test_finder_icon_is_declared_and_has_multiple_resolutions(self):
        with (BUNDLE / "Contents/Info.plist").open("rb") as source:
            info = plistlib.load(source)
        icon_name = info["CFBundleIconFile"]
        self.assertEqual(pathlib.Path(icon_name).stem, "NSApplicationIcon")
        icon = BUNDLE / "Contents/Resources" / icon_name
        data = icon.read_bytes()
        self.assertEqual(data[:4], b"icns")
        self.assertEqual(struct.unpack(">I", data[4:8])[0], len(data))
        offset = 8
        chunks = set()
        while offset < len(data):
            kind, size = struct.unpack(">4sI", data[offset:offset + 8])
            self.assertGreaterEqual(size, 8)
            self.assertLessEqual(offset + size, len(data))
            chunks.add(kind)
            offset += size
        self.assertEqual(offset, len(data))
        self.assertGreaterEqual(len(chunks), 5)
        self.assertIn(b"ic10", chunks)  # 1024px Retina representation
        with zipfile.ZipFile(ROOT / "dist" / f"{NAME}.zip") as archive:
            archived = archive.read(f"{NAME}/SpaceMosaic.app/Contents/Resources/{icon_name}")
        self.assertEqual(archived, data)

    def test_complete_bundle_has_a_valid_resource_seal(self):
        subprocess.run(["codesign", "--verify", "--strict", str(BUNDLE)], check=True)


if __name__ == "__main__":
    unittest.main()
