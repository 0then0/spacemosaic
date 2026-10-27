#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
: "${ANDROID_HOME:?Set ANDROID_HOME to an Android SDK with platform 36 and build-tools 36.0.0}"
export ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-$ANDROID_HOME/ndk/29.0.14206865}"
command -v cargo-ndk >/dev/null || { echo 'Install cargo-ndk 4.1.2 first (see README).' >&2; exit 1; }
test -d "$ANDROID_HOME/platforms/android-36" || { echo 'Android SDK platform 36 is missing.' >&2; exit 1; }
cargo ndk -t arm64-v8a -t x86_64 -o mobile/android/app/src/main/jniLibs build --locked --release --lib
mkdir -p mobile/android/app/src/main/assets/licenses
cp LICENSE THIRD_PARTY_NOTICES.txt mobile/android/app/src/main/assets/licenses/
cp assets/fonts/*.txt mobile/android/app/src/main/assets/licenses/
mobile/android/gradlew -p mobile/android --no-daemon assembleDebug
mkdir -p dist
cp mobile/android/app/build/outputs/apk/debug/app-debug.apk dist/spacemosaic-debug.apk
(cd dist && shasum -a 256 spacemosaic-debug.apk > android-SHA256SUMS)
"$ANDROID_HOME/build-tools/36.0.0/apksigner" verify --verbose dist/spacemosaic-debug.apk
python3 - "$ANDROID_HOME/build-tools/36.0.0/aapt" <<'CHECK_APK'
import subprocess, sys, zipfile
badging = subprocess.check_output([sys.argv[1], "dump", "badging", "dist/spacemosaic-debug.apk"], text=True)
application = next(line for line in badging.splitlines() if line.startswith("application:"))
assert "ic_launcher" in application, "APK must advertise its project launcher icon"
with zipfile.ZipFile("dist/spacemosaic-debug.apk") as package:
    assert package.testzip() is None
    for abi in ["arm64-v8a", "x86_64"]:
        assert f"lib/{abi}/libspacemosaic.so" in package.namelist()
    assert b"SpaceMosaic v0.1.0-alpha.3" in package.read("assets/licenses/THIRD_PARTY_NOTICES.txt")
CHECK_APK
