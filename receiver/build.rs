use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }

    println!("cargo:rerun-if-changed=src/macos/bridge.m");

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is not set"));
    let obj = out_dir.join("bridge.o");
    let lib = out_dir.join("libtinta_macos.a");

    let mut clang = Command::new("clang");
    clang.args(["-c", "-fobjc-arc", "-O2", "-Wall", "-Wextra", "-Wno-unused-parameter", "-Isrc/macos"]);
    if let Ok(target) = env::var("MACOSX_DEPLOYMENT_TARGET") {
        clang.arg(format!("-mmacosx-version-min={target}"));
    } else if env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("aarch64") {
        clang.arg("-mmacosx-version-min=11.0");
    } else {
        clang.arg("-mmacosx-version-min=10.15");
    }
    let status = clang
        .arg("-o")
        .arg(&obj)
        .arg("src/macos/bridge.m")
        .status()
        .expect("failed to invoke clang");
    if !status.success() {
        panic!("clang failed to compile src/macos/bridge.m (exit {status})");
    }

    let ar_status = Command::new("ar")
        .arg("rcs")
        .arg(&lib)
        .arg(&obj)
        .status()
        .expect("failed to invoke ar");
    if !ar_status.success() {
        panic!("ar failed to archive {} (exit {ar_status})", lib.display());
    }

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=tinta_macos");

    for fw in [
        "Foundation",
        "CoreFoundation",
        "CoreGraphics",
        "AppKit",
        "ApplicationServices",
        "CoreMedia",
        "CoreVideo",
        "VideoToolbox",
        "ScreenCaptureKit",
        "IOKit",
    ] {
        println!("cargo:rustc-link-lib=framework={fw}");
    }
}
