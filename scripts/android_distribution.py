"""Validate Android distribution inputs and signed APK metadata without reading keys."""

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PREFIX = "SPACEMOSAIC_ANDROID_"


def version(root=ROOT):
    package = (
        (root / "Cargo.toml").read_text().split("[package]", 1)[1].split("\n[", 1)[0]
    )
    name = re.search(r'^version = "([^"]+)"$', package, re.MULTILINE)[1]
    props = (root / "mobile/android/gradle.properties").read_text()
    code = int(re.search(r"^spacemosaicVersionCode=(\d+)$", props, re.MULTILINE)[1])
    if not 1 <= code <= 2100000000:
        raise ValueError("Invalid spacemosaicVersionCode")
    return name, code


def fingerprint(value):
    value = value.replace(":", "").lower()
    if not re.fullmatch(r"[0-9a-f]{64}", value):
        raise ValueError("Expected a SHA-256 certificate fingerprint (64 hex digits)")
    return value


def preflight(mode, env=None, root=ROOT):
    env = os.environ if env is None else env
    _, code = version(root)
    if mode == "debug":
        return
    names = [
        "KEYSTORE",
        "STORE_PASSWORD",
        "KEY_ALIAS",
        "KEY_PASSWORD",
        "CERT_SHA256",
        "PREVIOUS_VERSION_CODE",
    ]
    missing = [PREFIX + n for n in names if not env.get(PREFIX + n, "").strip()]
    if missing:
        raise ValueError(
            "Release signing requires: "
            + ", ".join(missing)
            + ". See docs/android-distribution.md; no debug-key fallback."
        )
    if not Path(env[PREFIX + "KEYSTORE"]).is_file():
        raise ValueError("Release keystore is not a readable file")
    fingerprint(env[PREFIX + "CERT_SHA256"])
    try:
        previous = int(env[PREFIX + "PREVIOUS_VERSION_CODE"])
    except ValueError:
        raise ValueError("PREVIOUS_VERSION_CODE must be an integer") from None
    if previous < 0 or code <= previous:
        raise ValueError("spacemosaicVersionCode must exceed PREVIOUS_VERSION_CODE")


def verify(apk, mode, tools, env=None, root=ROOT):
    env = os.environ if env is None else env
    name, code = version(root)
    certs = subprocess.check_output(
        [str(tools / "apksigner"), "verify", "--verbose", "--print-certs", str(apk)],
        text=True,
    )
    digests = re.findall(
        r"^Signer #\d+ certificate SHA-256 digest: (\w+)$", certs, re.MULTILINE
    )
    if len(digests) != 1:
        raise ValueError("Expected exactly one APK signer")
    digest = fingerprint(digests[0])
    if mode == "release" and digest != fingerprint(env.get(PREFIX + "CERT_SHA256", "")):
        raise ValueError("APK signing certificate does not match CERT_SHA256")
    badging = subprocess.check_output(
        [str(tools / "aapt"), "dump", "badging", str(apk)],
        text=True,
    )
    package = re.search(
        r"^package: name='([^']+)' versionCode='(\d+)' versionName='([^']+)'",
        badging,
    )
    if not package or package.groups() != ("dev.spacemosaic", str(code), name):
        raise ValueError(
            "APK applicationId/version does not match the source configuration"
        )
    if mode == "release" and "application-debuggable" in badging:
        raise ValueError("Release APK must not be debuggable")
    # Release resource optimization shortens archive paths (for example res/IG.xml).
    # Check the resource identity referenced by the manifest, not its ZIP filename.
    resources = subprocess.check_output(
        [str(tools / "aapt"), "dump", "resources", str(apk)],
        text=True,
    )
    manifest = subprocess.check_output(
        [str(tools / "aapt"), "dump", "xmltree", str(apk), "AndroidManifest.xml"],
        text=True,
    )
    icon = re.search(
        r"spec resource (0x[0-9a-f]+) dev\.spacemosaic:mipmap/ic_launcher:", resources
    )
    if not icon or any(
        not re.search(
            r"android:" + attribute + r"\([^)]*\)=@" + icon[1] + r"\b", manifest
        )
        for attribute in ("icon", "roundIcon")
    ):
        raise ValueError("APK must advertise its project launcher icon and round icon")
    with zipfile.ZipFile(apk) as archive:
        if archive.testzip() is not None:
            raise ValueError("Corrupt APK")
        for abi in ("arm64-v8a", "x86_64"):
            archive.getinfo(f"lib/{abi}/libspacemosaic.so")
        if f"SpaceMosaic v{name}".encode() not in archive.read(
            "assets/licenses/THIRD_PARTY_NOTICES.txt"
        ):
            raise ValueError("APK license notices do not match the source version")
    return {
        "applicationId": "dev.spacemosaic",
        "versionCode": code,
        "versionName": name,
        "certificateSha256": digest,
        "apkSha256": hashlib.sha256(apk.read_bytes()).hexdigest(),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["preflight", "verify"])
    parser.add_argument("mode", choices=["debug", "release"])
    parser.add_argument("--apk", type=Path)
    parser.add_argument("--tools", type=Path)
    args = parser.parse_args()
    try:
        if args.action == "preflight":
            preflight(args.mode)
        else:
            if not args.apk or not args.tools:
                parser.error("verify requires --apk and --tools")
            print(json.dumps(verify(args.apk, args.mode, args.tools), indent=2))
    except (ValueError, OSError, KeyError, subprocess.CalledProcessError) as error:
        # Never print environment values or command arguments containing credentials.
        print(f"Android distribution validation failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
