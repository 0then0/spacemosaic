#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
destination="${1:-simulator}"
case "$destination" in
  simulator) sdk=iphonesimulator; triple=aarch64-apple-ios-sim ;;
  device) sdk=iphoneos; triple=aarch64-apple-ios ;;
  *) echo 'Usage: build-ios.sh [simulator|device]' >&2; exit 2 ;;
esac
xcrun --sdk "$sdk" --show-sdk-path >/dev/null
export IPHONEOS_DEPLOYMENT_TARGET=16.0
cargo rustc --locked --release --lib --target "$triple" --crate-type staticlib
xcodebuild -project mobile/ios/SpaceMosaic.xcodeproj -scheme SpaceMosaic \
  -configuration Release -sdk "$sdk" -destination "generic/platform=iOS$([ "$destination" = simulator ] && echo ' Simulator' || true)" \
  -derivedDataPath "dist/ios-$destination" \
  "RUST_LIBRARY_DIR=$PWD/target/$triple/release" CODE_SIGNING_ALLOWED=NO build
mkdir -p dist
ditto -c -k --keepParent "dist/ios-$destination/Build/Products/Release-$sdk/SpaceMosaic.app" "dist/spacemosaic-$destination.zip"
(cd dist && shasum -a 256 "spacemosaic-$destination.zip" > "ios-$destination-SHA256SUMS")
