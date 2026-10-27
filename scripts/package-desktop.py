#!/usr/bin/env python3
"""Package desktop binaries and license notices without publishing a release."""

import hashlib
import json
import os
import pathlib
import platform
import plistlib
import shutil
import subprocess
import tarfile
import tempfile
import zipfile

root = pathlib.Path(__file__).resolve().parent.parent
metadata = json.loads(
    subprocess.check_output(
        [
            "cargo",
            "metadata",
            "--locked",
            "--offline",
            "--no-deps",
            "--format-version",
            "1",
        ],
        cwd=root,
    )
)
version = next(
    package["version"]
    for package in metadata["packages"]
    if package["name"] == "spacemosaic"
)
system = platform.system().lower()
arch = platform.machine().lower()
dist = root / "dist"
dist.mkdir(exist_ok=True)
name = f"spacemosaic-{system}-{arch}"
bundle = dist / name
if bundle.exists():
    shutil.rmtree(bundle)
bundle.mkdir()
executable = (
    root
    / "target/release"
    / ("spacemosaic.exe" if system == "windows" else "spacemosaic")
)
if system == "darwin":
    app = bundle / "SpaceMosaic.app/Contents"
    (app / "MacOS").mkdir(parents=True, exist_ok=True)
    shutil.copy2(executable, app / "MacOS/spacemosaic")
    resources = app / "Resources"
    resources.mkdir(exist_ok=True)
    # AppKit's standard About panel resolves this named image independently of
    # the runtime Dock icon. Keep the bundle icon and named resource identical.
    previous_icon = resources / "SpaceMosaic.icns"
    if previous_icon.exists():
        previous_icon.unlink()
    with tempfile.TemporaryDirectory(prefix="spacemosaic-icon-") as temporary:
        iconset = pathlib.Path(temporary) / "SpaceMosaic.iconset"
        iconset.mkdir()
        for size in [16, 32, 128, 256, 512]:
            for scale in [1, 2]:
                suffix = "@2x" if scale == 2 else ""
                destination = iconset / f"icon_{size}x{size}{suffix}.png"
                subprocess.run(
                    [
                        "sips",
                        "-z",
                        str(size * scale),
                        str(size * scale),
                        str(root / "assets/icon.png"),
                        "--out",
                        str(destination),
                    ],
                    check=True,
                    stdout=subprocess.DEVNULL,
                )
        subprocess.run(
            [
                "iconutil",
                "-c",
                "icns",
                str(iconset),
                "-o",
                str(resources / "NSApplicationIcon.icns"),
            ],
            check=True,
        )
    with (app / "Info.plist").open("wb") as output:
        plistlib.dump(
            {
                "CFBundleIdentifier": "dev.spacemosaic.desktop",
                "CFBundleName": "SpaceMosaic",
                "CFBundleExecutable": "spacemosaic",
                "CFBundlePackageType": "APPL",
                "CFBundleShortVersionString": version.split("-", 1)[0],
                "CFBundleGetInfoString": f"SpaceMosaic {version}",
                "CFBundleVersion": "4",
                "CFBundleIconFile": "NSApplicationIcon.icns",
                "NSHighResolutionCapable": True,
            },
            output,
        )
else:
    shutil.copy2(executable, bundle / executable.name)
if system == "windows":
    shutil.copy2(root / "assets/icon.ico", bundle / "SpaceMosaic.ico")
    shutil.copy2(
        root / "packaging/windows/LICENSE-InnoSetup.txt",
        bundle / "LICENSE-InnoSetup.txt",
    )
shutil.copy2(root / "THIRD_PARTY_NOTICES.txt", bundle / "THIRD_PARTY_NOTICES.txt")
for file in ["README.md", "LICENSE"]:
    shutil.copy2(root / file, bundle / file)
shutil.copytree(
    root / "assets/fonts",
    bundle / "font-licenses",
    dirs_exist_ok=True,
    ignore=shutil.ignore_patterns("*.ttf"),
)
if system == "darwin":
    licenses = resources / "licenses"
    licenses.mkdir()
    for file in ["LICENSE", "THIRD_PARTY_NOTICES.txt"]:
        shutil.copy2(root / file, licenses / file)
    shutil.copytree(bundle / "font-licenses", licenses / "fonts")
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
artifacts = [archive]
if system == "darwin":
    image = dist / f"{name}.dmg"
    with tempfile.TemporaryDirectory(prefix="spacemosaic-dmg-") as temporary:
        staging = pathlib.Path(temporary) / "image"
        shutil.copytree(bundle, staging)
        (staging / "Applications").symlink_to("/Applications", target_is_directory=True)
        subprocess.run(
            [
                "hdiutil",
                "create",
                "-ov",
                "-fs",
                "HFS+",
                "-format",
                "UDZO",
                "-volname",
                f"SpaceMosaic {version}",
                "-srcfolder",
                str(staging),
                str(image),
            ],
            check=True,
        )
    subprocess.run(["hdiutil", "verify", str(image)], check=True)
    artifacts.append(image)
elif system == "windows":
    compiler = os.environ.get("INNO_SETUP_COMPILER")
    if not compiler:
        raise SystemExit(
            "Set INNO_SETUP_COMPILER using scripts/setup-inno.ps1 (Inno Setup 7.1.0)."
        )
    environment = os.environ.copy()
    environment.update(
        SPACEMOSAIC_VERSION=version,
        SPACEMOSAIC_BUNDLE_DIR=str(bundle),
        SPACEMOSAIC_OUTPUT_DIR=str(dist),
        SPACEMOSAIC_ICON=str(root / "assets/icon.ico"),
    )
    subprocess.run(
        [compiler, str(root / "packaging/windows/setup.iss")],
        env=environment,
        check=True,
    )
    artifacts.append(dist / f"{name}-setup.exe")
elif system == "linux":
    portable = dist / f"{name}.tar.gz"
    with tarfile.open(portable, "w:gz") as output:
        output.add(bundle, arcname=name)
    artifacts.append(portable)
    deb = dist / f"{name}.deb"
    deb_arch = {"x86_64": "amd64", "aarch64": "arm64"}[arch]
    with tempfile.TemporaryDirectory(prefix="spacemosaic-deb-") as temporary:
        package = pathlib.Path(temporary) / "package"
        binary = package / "usr/bin"
        binary.mkdir(parents=True)
        shutil.copy2(executable, binary / "spacemosaic")
        docs = package / "usr/share/doc/spacemosaic"
        shutil.copytree(bundle / "font-licenses", docs / "font-licenses")
        for file in ["README.md", "LICENSE", "THIRD_PARTY_NOTICES.txt"]:
            shutil.copy2(root / file, docs / file)
        icon = package / "usr/share/icons/hicolor/scalable/apps"
        icon.mkdir(parents=True)
        shutil.copy2(root / "assets/icon.svg", icon / "spacemosaic.svg")
        applications = package / "usr/share/applications"
        applications.mkdir(parents=True)
        (applications / "spacemosaic.desktop").write_text(
            "[Desktop Entry]\nType=Application\nName=SpaceMosaic\n"
            "Comment=Explore storage usage without changing files\n"
            "Exec=spacemosaic\nIcon=spacemosaic\nTerminal=false\nCategories=System;Utility;\n"
        )
        control = package / "DEBIAN"
        control.mkdir()
        (control / "control").write_text(
            f"Package: spacemosaic\nVersion: {version.replace('-', '~', 1)}\n"
            f"Architecture: {deb_arch}\nSection: utils\nPriority: optional\n"
            "Maintainer: SpaceMosaic contributors <195298332+0then0@users.noreply.github.com>\n"
            "Homepage: https://github.com/0then0/spacemosaic\n"
            "Depends: libc6 (>= 2.39), libgcc-s1, libstdc++6, libxkbcommon0, "
            "libxkbcommon-x11-0, libegl1, libwayland-client0, libx11-6, libxcb1, "
            "libxcursor1, libxi6, libxrandr2\n"
            "Description: Local read-only storage explorer with an interactive treemap\n"
        )
        subprocess.run(
            ["dpkg-deb", "--build", "--root-owner-group", str(package), str(deb)],
            check=True,
        )
    artifacts.append(deb)
manifest = ""
for artifact in artifacts:
    digest = hashlib.sha256(artifact.read_bytes()).hexdigest()
    manifest += f"{digest}  {artifact.name}\n"
    print(artifact)
(dist / f"{name}-SHA256SUMS").write_text(manifest)
