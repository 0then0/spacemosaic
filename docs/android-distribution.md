# Android APK distribution

SpaceMosaic is distributed directly through GitHub Releases, without Google
Play or an update service. The application ID remains `dev.spacemosaic`.
The owner supplies and retains the signing key; builds never generate a release
key. A compatible update needs the same signing identity and a higher
`versionCode`. Do not replace or rotate that key as part of routine builds.

## Published APK compatibility

Audit on 2026-10-27, using Android SDK Build Tools 36.0.0 `apksigner verify
--verbose --print-certs` and `aapt dump badging` on the GitHub Release assets:

- `v0.1.0-alpha.1`: no Android APK was published.
- `v0.1.0-alpha.2/spacemosaic-debug.apk`: application ID `dev.spacemosaic`,
  versionCode `2`, versionName `0.1.0-alpha.2`, certificate subject
  `C=US, O=Android, CN=Android Debug`, certificate SHA-256
  `45c92146e66880ce6c2b40acd16255cf3d574a6604d1e7ff9f0eadf58108f86a`.
- `v0.1.0-alpha.3/spacemosaic-debug.apk`: application ID `dev.spacemosaic`,
  versionCode `3`, versionName `0.1.0-alpha.3`, certificate subject
  `C=US, O=Android, CN=Android Debug`, certificate SHA-256
  `ee5d62d7ac79b8ee68975df2e861fdfa49a5cb795ab6a6fb500c4208911b23fd`.

Both APKs passed signature verification (v2). Their certificates differ, so
alpha.3 cannot update alpha.2 in place. No pair of published Android versions
currently shares a signing identity. Certificate subjects alone do not identify
a key. This audit read public APK certificates, not private keystores.

A future release signed with a different key cannot update either debug APK.
Users must explicitly uninstall the old application once and install the new
release. Uninstalling deletes application settings and retained SAF URI grants;
users must select their folders again. Export any wanted results first. Files
in the selected folders are not deleted by SpaceMosaic. No build or migration
script uninstalls the application or clears user data automatically.

## Local builds

Use the Rust, JDK 17, SDK 36, Build Tools 36.0.0, NDK and cargo-ndk versions in
[README](../README.md#mobile-development). Keep `Cargo.lock` and the Gradle wrapper.
Debug development and PR builds require no signing credentials:

```sh
bash scripts/build-android.sh debug
```

For release builds, provide these environment variables explicitly:

- `SPACEMOSAIC_ANDROID_KEYSTORE`: absolute path to the owner's existing keystore,
  stored outside the checkout.
- `SPACEMOSAIC_ANDROID_STORE_PASSWORD`: keystore password.
- `SPACEMOSAIC_ANDROID_KEY_ALIAS`: signing key alias.
- `SPACEMOSAIC_ANDROID_KEY_PASSWORD`: private key password.
- `SPACEMOSAIC_ANDROID_CERT_SHA256`: expected certificate SHA-256, independently
  recorded by the owner; 64 hexadecimal digits, with optional colons.
- `SPACEMOSAIC_ANDROID_PREVIOUS_VERSION_CODE`: highest code already distributed,
  including debug versions (`3` after alpha.3).

Supply passwords through a password manager or silent shell input. Do not put
values in command arguments, tracked files, terminal transcripts, shell history,
or shell tracing. For example, in Bash, `read -rs SPACEMOSAIC_ANDROID_STORE_PASSWORD`
then `export SPACEMOSAIC_ANDROID_STORE_PASSWORD` reads without echoing. Repeat for
the key password. Do not run Gradle with debug logging, build scans, or a
configuration cache when signing.

```sh
bash scripts/build-android.sh release
# Public certificate information, not the private key:
"$ANDROID_HOME/build-tools/36.0.0/apksigner" verify --verbose --print-certs dist/spacemosaic-release.apk
"$ANDROID_HOME/build-tools/36.0.0/aapt" dump badging dist/spacemosaic-release.apk
(cd dist && shasum -a 256 -c android-SHA256SUMS)
```

The script fails before compilation when required inputs are absent. Gradle
also rejects direct release builds without signing credentials; it never falls
back to a debug key. The script verifies the finished APK's signature,
certificate, application ID, versions, non-debuggable flag, ABIs and notices
before copying it to `dist/`. Outputs are `spacemosaic-release.apk`,
`android-SHA256SUMS` and `android-release-metadata.json` (public fingerprints and
version metadata only). A failed build must not be distributed; old files in
`dist/` are not evidence of a successful current build.

`versionName` is read from the root Cargo package version. Android's separate
counter is `spacemosaicVersionCode` in `mobile/android/gradle.properties`;
this change reserves code `4`, which is not yet published. Increase it for each
new distributed APK, even for prereleases or rebuilds with a new version name.
Never reset it when moving from alpha to stable. Update the Cargo version and
versioned license notices together before the next release. The script requires
the counter to exceed the explicitly supplied previous code. The owner must
check that previous code against all existing distribution channels.

Keep encrypted offline backups of the keystore, alias and passwords in separate
secure locations, and record the public certificate fingerprint independently.
Verify recovery before depending on the key. Losing it can make future in-place
updates impossible. No production key has been created by this implementation.

## GitHub Actions

The ordinary `CI` workflow remains a secret-free debug build for pushes and PRs.
The separate **Android signed APK** workflow is manual, runs only on `main`, uses
read-only repository permissions and uploads only the verified APK, public
metadata and checksums. It does not publish a release.

Before using it, the owner must configure an `android-release` GitHub environment
with required reviewer approval, restricted to the protected `main` branch.
Store these as **environment secrets**, never repository-wide secrets exposed to
other workflows:

- `ANDROID_RELEASE_KEYSTORE_BASE64`: single-line base64 of the existing keystore.
- `ANDROID_RELEASE_STORE_PASSWORD`
- `ANDROID_RELEASE_KEY_ALIAS`
- `ANDROID_RELEASE_KEY_PASSWORD`

Set environment variable `ANDROID_RELEASE_CERT_SHA256` to the independently
verified public certificate fingerprint. Dispatch from `main` with
`previous_version_code` set to the highest distributed code. Missing values or a
wrong certificate fail the build. The temporary decoded keystore is private to
the signing step and removed on exit; it is never included in artifact paths.
Review changes to signing scripts and this workflow before approving access.
PR code must never be checked out or executed by a job holding signing secrets;
do not convert this workflow into `pull_request_target` or `workflow_run`.

Download the `android-release` artifact, verify its fingerprint and SHA-256,
and attach those exact files to the intended GitHub Release through the existing
manual release process. Do not attach the `android-debug` artifact as a release
APK. These instructions do not configure secrets or publish anything.

## Upgrade verification on a disposable emulator

The user has confirmed SpaceMosaic works relatively stably on Pixel 8 with
Android 17. That is runtime evidence, not a signing, upgrade, screen-reader or
all-provider acceptance test. No Android runtime is installed in the local
signing-audit environment, so an in-place update has **not** been runtime-verified.

Use a disposable emulator with no personal data. Build A and B from disposable
source copies with codes `4` and `5`, respectively (edit the same Gradle property),
using a single temporary test key and its independently checked fingerprint.
Keep the normal Cargo version and notices consistent in each copy. Use previous
codes `3` and `4` for the respective builds. Never use production credentials.
A test-only key may be created outside the checkout:

```sh
# Set TEST_KEYSTORE and TEST_KEY_PASSWORD using private temporary storage and
# silent input. This key is for disposable tests only, never GitHub releases.
keytool -genkeypair -keystore "$TEST_KEYSTORE" -alias upgrade-test \
  -keyalg RSA -keysize 3072 -validity 30 -dname 'CN=SpaceMosaic disposable upgrade test' \
  -storepass:env TEST_KEY_PASSWORD -keypass:env TEST_KEY_PASSWORD
keytool -list -v -keystore "$TEST_KEYSTORE" -alias upgrade-test -storepass:env TEST_KEY_PASSWORD
# Map the test key/password/alias/fingerprint to the SPACEMOSAIC_ANDROID_*
# variables above, then run in each disposable copy:
bash scripts/build-android.sh release
# Save outputs separately as /tmp/spacemosaic-A.apk and /tmp/spacemosaic-B.apk.
```

For an explicitly selected disposable emulator serial:

```sh
export ANDROID_SERIAL=emulator-5554
adb devices -l
adb -s "$ANDROID_SERIAL" shell mkdir -p /sdcard/Download/SpaceMosaicUpgrade
printf 'upgrade fixture\n' > /tmp/spacemosaic-upgrade-fixture.txt
adb -s "$ANDROID_SERIAL" push /tmp/spacemosaic-upgrade-fixture.txt /sdcard/Download/SpaceMosaicUpgrade/sample.txt
adb -s "$ANDROID_SERIAL" install /tmp/spacemosaic-A.apk
adb -s "$ANDROID_SERIAL" shell am start -n dev.spacemosaic/dev.spacemosaic.StorageActivity
```

In A, choose a non-default language and theme. Select
`Download/SpaceMosaicUpgrade` using the system picker, grant access, and scan it.
Restart A once to confirm settings and access already persist before upgrading.
Then, without uninstalling or clearing data:

```sh
adb -s "$ANDROID_SERIAL" install -r /tmp/spacemosaic-B.apk
adb -s "$ANDROID_SERIAL" shell dumpsys package dev.spacemosaic | grep versionCode
adb -s "$ANDROID_SERIAL" shell am start -n dev.spacemosaic/dev.spacemosaic.StorageActivity
```

Check that the chosen theme and language remain, the previous tree is restored
without a new picker grant (unless the OS revoked it), and a new scan finds
`sample.txt`. Export JSON and CSV through the system save picker into Download.
Pull the chosen filenames with `adb -s "$ANDROID_SERIAL" pull
/sdcard/Download/<export-name> /tmp/` and inspect their scope, entry and size.
Record A/B fingerprints and codes, install exit statuses, retained settings,
access and successful scan/export. An `adb install -r` success alone does not
prove settings or SAF grant retention. Never run this sequence against the
user's existing installation.

See official Android documentation for [app signing](https://developer.android.com/studio/publish/app-signing),
[APK verification](https://developer.android.com/tools/apksigner) and
[versioning](https://developer.android.com/studio/publish/versioning).
