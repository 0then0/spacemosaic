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
application="dist/ios-$destination/Build/Products/Release-$sdk/SpaceMosaic.app"
mkdir -p "$application/licenses"
cp LICENSE THIRD_PARTY_NOTICES.txt "$application/licenses/"
cp assets/fonts/*.txt "$application/licenses/"
python3 - "$application" <<'CHECK_IOS'
import pathlib, plistlib, sys
app = pathlib.Path(sys.argv[1])
with (app / "Info.plist").open("rb") as source:
    info = plistlib.load(source)
with pathlib.Path("mobile/ios/Info.plist").open("rb") as source:
    expected = plistlib.load(source)
for key in ("CFBundleShortVersionString", "CFBundleGetInfoString", "CFBundleVersion"):
    assert info[key] == expected[key], f"Unexpected {key}: {info[key]}"
assert (app / "licenses/THIRD_PARTY_NOTICES.txt").read_bytes() == pathlib.Path("THIRD_PARTY_NOTICES.txt").read_bytes()
assert (app / "THIRD_PARTY_NOTICES.txt").read_bytes() == pathlib.Path("THIRD_PARTY_NOTICES.txt").read_bytes()
CHECK_IOS
mkdir -p dist
ditto -c -k --keepParent "dist/ios-$destination/Build/Products/Release-$sdk/SpaceMosaic.app" "dist/spacemosaic-$destination.zip"
(cd dist && shasum -a 256 "spacemosaic-$destination.zip" > "ios-$destination-SHA256SUMS")
