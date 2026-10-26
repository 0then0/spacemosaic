use std::{env, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=src/macos_icon.m");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos")
        || env::var_os("CARGO_FEATURE_GUI").is_none()
    {
        return;
    }
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo output directory"));
    let object = output.join("macos_icon.o");
    let library = output.join("libspacemosaic_icon.a");
    let architecture = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "arm64",
        Ok("x86_64") => "x86_64",
        _ => panic!("unsupported macOS architecture"),
    };
    assert!(
        Command::new("xcrun")
            .args([
                "--sdk",
                "macosx",
                "clang",
                "-fobjc-arc",
                "-arch",
                architecture,
                "-c",
                "src/macos_icon.m",
                "-o"
            ])
            .arg(&object)
            .status()
            .expect("macOS SDK compiler")
            .success()
    );
    assert!(
        Command::new("xcrun")
            .args(["ar", "crs"])
            .arg(&library)
            .arg(&object)
            .status()
            .expect("macOS archiver")
            .success()
    );
    println!("cargo:rustc-link-search=native={}", output.display());
    println!("cargo:rustc-link-lib=static=spacemosaic_icon");
    println!("cargo:rustc-link-lib=framework=AppKit");
}
