#!/usr/bin/env bash
set +x
set -euo pipefail
cd "$(dirname "$0")/.."
mode="${1:-debug}"
case "$mode" in debug|release) ;; *) echo 'Usage: build-android.sh [debug|release]' >&2; exit 2 ;; esac
python3 scripts/android_distribution.py preflight "$mode"
: "${ANDROID_HOME:?Set ANDROID_HOME to an Android SDK with platform 36 and build-tools 36.0.0}"
export ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-$ANDROID_HOME/ndk/29.0.14206865}"
command -v cargo-ndk >/dev/null || { echo 'Install cargo-ndk 4.1.2 first (see README).' >&2; exit 1; }
test -d "$ANDROID_HOME/platforms/android-36" || { echo 'Android SDK platform 36 is missing.' >&2; exit 1; }
cargo ndk -t arm64-v8a -t x86_64 -o mobile/android/app/src/main/jniLibs build --locked --release --lib
mkdir -p mobile/android/app/src/main/assets/licenses
cp LICENSE THIRD_PARTY_NOTICES.txt mobile/android/app/src/main/assets/licenses/
cp assets/fonts/*.txt mobile/android/app/src/main/assets/licenses/
if [[ "$mode" == release ]]; then
    task=assembleRelease
else
    task=assembleDebug
fi
# Signing values must not be serialized into a configuration cache or traced.
mobile/android/gradlew -p mobile/android --no-daemon --no-configuration-cache "$task"
mkdir -p dist
apk="spacemosaic-$mode.apk"
source_apk="mobile/android/app/build/outputs/apk/$mode/app-$mode.apk"
# Verify before promoting the APK to the distribution directory.
python3 scripts/android_distribution.py verify "$mode" --apk "$source_apk" \
    --tools "$ANDROID_HOME/build-tools/36.0.0" > "dist/android-$mode-metadata.json"
cp "$source_apk" "dist/$apk"
(cd dist && shasum -a 256 "$apk" > android-SHA256SUMS)
