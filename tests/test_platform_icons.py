"""Check launcher resources and the adaptive icon's circular safe zone."""

import math
import pathlib
import re
import struct
import unittest
import xml.etree.ElementTree as ET

ROOT = pathlib.Path(__file__).resolve().parents[1]
RES = ROOT / "mobile/android/app/src/main/res"
A = "{http://schemas.android.com/apk/res/android}"


def path_bounds(data):
    tokens = re.findall(r"[A-Za-z]|[-+]?(?:\d*\.)?\d+", data)
    points = []
    current = start = (0.0, 0.0)
    while tokens:
        command = tokens.pop(0)
        count = {"m": 2, "l": 2, "h": 1, "v": 1, "a": 7, "z": 0}[command.lower()]
        values = [float(tokens.pop(0)) for _ in range(count)]
        x, y = current
        relative = command.islower()
        if command.lower() in ["m", "l"]:
            current = (
                values[0] + (x if relative else 0),
                values[1] + (y if relative else 0),
            )
            if command.lower() == "m":
                start = current
        elif command.lower() == "h":
            current = (values[0] + (x if relative else 0), y)
        elif command.lower() == "v":
            current = (x, values[0] + (y if relative else 0))
        elif command.lower() == "a":
            rx, ry, rotation, large, sweep, dx, dy = values
            current = (dx + (x if relative else 0), dy + (y if relative else 0))
            # The project glyph uses only axis-aligned quarter arcs. Include
            # every bounding-box corner, conservatively covering their curves.
            assert rotation == 0 and large == 0 and sweep in [0, 1]
            assert abs(current[0] - x) == rx and abs(current[1] - y) == ry
            points.extend([(x, current[1]), (current[0], y)])
        else:
            current = start
        points.append(current)
    return points


class PlatformIconTests(unittest.TestCase):
    def test_android_manifest_selects_adaptive_and_round_icons(self):
        app = (
            ET.parse(ROOT / "mobile/android/app/src/main/AndroidManifest.xml")
            .getroot()
            .find("application")
        )
        self.assertEqual(app.attrib[A + "icon"], "@mipmap/ic_launcher")
        self.assertEqual(app.attrib[A + "roundIcon"], "@mipmap/ic_launcher")
        for version in [26, 33]:
            icon = ET.parse(RES / f"mipmap-anydpi-v{version}/ic_launcher.xml").getroot()
            self.assertEqual(icon.tag, "adaptive-icon")
            self.assertEqual(
                icon.find("foreground").attrib[A + "drawable"],
                "@drawable/ic_launcher_foreground",
            )
            self.assertEqual(
                icon.find("background").attrib[A + "drawable"],
                "@color/ic_launcher_background",
            )
        self.assertIsNotNone(icon.find("monochrome"))

    def test_color_and_themed_glyphs_fit_inside_the_circular_safe_zone(self):
        for name in ["foreground", "monochrome"]:
            vector = ET.parse(RES / f"drawable/ic_launcher_{name}.xml").getroot()
            self.assertEqual(vector.attrib[A + "viewportWidth"], "108")
            group = vector.find("group")
            sx, sy = (
                float(group.attrib[A + "scaleX"]),
                float(group.attrib[A + "scaleY"]),
            )
            tx, ty = (
                float(group.attrib[A + "translateX"]),
                float(group.attrib[A + "translateY"]),
            )
            for path in group.findall("path"):
                for x, y in path_bounds(path.attrib[A + "pathData"]):
                    self.assertLessEqual(
                        math.hypot(x * sx + tx - 54, y * sy + ty - 54),
                        33,
                        f"{name} glyph would be clipped by a circular launcher mask",
                    )

    def test_windows_setup_icon_has_a_valid_256_pixel_png_representation(self):
        data = (ROOT / "assets/icon.ico").read_bytes()
        self.assertEqual(struct.unpack_from("<HHH", data), (0, 1, 1))
        width, height, colors, reserved, planes, bits, size, offset = (
            struct.unpack_from("<BBBBHHII", data, 6)
        )
        self.assertEqual((width, height, planes, bits), (0, 0, 1, 32))
        image = data[offset : offset + size]
        self.assertEqual(image[:8], b"\x89PNG\r\n\x1a\n")
        self.assertEqual(struct.unpack_from(">II", image, 16), (256, 256))
