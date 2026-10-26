#!/usr/bin/env python3
"""Package desktop binaries and license notices without publishing a release."""
import hashlib
import pathlib
import platform
import plistlib
import shutil
import subprocess
import zipfile

root = pathlib.Path(__file__).resolve().parent.parent
system = platform.system().lower()
arch = platform.machine().lower()
dist = root / "dist"
dist.mkdir(exist_ok=True)
name = f"spacemosaic-{system}-{arch}"
bundle = dist / name
bundle.mkdir(exist_ok=True)
executable = root / "target/release" / ("spacemosaic.exe" if system == "windows" else "spacemosaic")
if system == "darwin":
    app = bundle / "SpaceMosaic.app/Contents"
    (app / "MacOS").mkdir(parents=True, exist_ok=True)
    shutil.copy2(executable, app / "MacOS/spacemosaic")
    with (app / "Info.plist").open("wb") as output:
        plistlib.dump({"CFBundleIdentifier": "dev.spacemosaic.desktop",
                      "CFBundleName": "SpaceMosaic", "CFBundleExecutable": "spacemosaic",
                      "CFBundlePackageType": "APPL", "CFBundleShortVersionString": "0.1.0",
                      "CFBundleVersion": "1", "NSHighResolutionCapable": True}, output)
else:
    shutil.copy2(executable, bundle / executable.name)
for file in ["README.md", "LICENSE"]:
    shutil.copy2(root / file, bundle / file)
shutil.copytree(root / "assets/fonts", bundle / "font-licenses", dirs_exist_ok=True,
                ignore=shutil.ignore_patterns("*.ttf"))
if system == "darwin":
    # Seal the complete bundle; rustc's executable-only ad-hoc signature has no resource seal.
    # '-' is local ad-hoc signing, with no certificate, identity or provisioning access.
    application = bundle / "SpaceMosaic.app"
    subprocess.run(["codesign", "--force", "--sign", "-", str(application)], check=True)
    subprocess.run(["codesign", "--verify", "--strict", str(application)], check=True)
archive = dist / f"{name}.zip"
with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as output:
    for file in sorted(bundle.rglob("*")):
        if file.is_file():
            output.write(file, file.relative_to(dist))
digest = hashlib.sha256(archive.read_bytes()).hexdigest()
(dist / f"{name}-SHA256SUMS").write_text(f"{digest}  {archive.name}\n")
print(archive)
