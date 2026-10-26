#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
: "${ANDROID_HOME:?Set ANDROID_HOME to an Android SDK with platform 36 and build-tools 36.0.0}"
export ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-$ANDROID_HOME/ndk/29.0.14206865}"
command -v cargo-ndk >/dev/null || { echo 'Install cargo-ndk 4.1.2 first (see README).' >&2; exit 1; }
test -d "$ANDROID_HOME/platforms/android-36" || { echo 'Android SDK platform 36 is missing.' >&2; exit 1; }
cargo ndk -t arm64-v8a -t x86_64 -o mobile/android/app/src/main/jniLibs build --locked --release --lib
mobile/android/gradlew -p mobile/android --no-daemon assembleDebug
mkdir -p dist
cp mobile/android/app/build/outputs/apk/debug/app-debug.apk dist/spacemosaic-debug.apk
(cd dist && shasum -a 256 spacemosaic-debug.apk > android-SHA256SUMS)
