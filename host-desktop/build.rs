// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
// Compiles the C shim and links its libraries.
//
// - Linux and Windows: csrc/hd_shim.c (FFmpeg and OpenH264 encoders, swscale) and
//   csrc/hd_opus.c (Opus), with the libraries from pkg-config. On Linux with the
//   `linux-portal` feature also VA-API and libavfilter (HD_VAAPI). For
//   x86_64-pc-windows-gnu they come from remote/build/win-deps through
//   `$PKG_CONFIG_x86_64_pc_windows_gnu` (env.sh), never the host's .pc files.
// - macOS: only csrc/hd_opus.c. The encoder is VideoToolbox (src/mac/vt.rs), so there is
//   no FFmpeg or OpenH264, and the Mac has no pkg-config: Opus is the static library that
//   `host-desktop/build-opus-mac.sh` installs under `$SODIUM_REMOTE_OPUS` (default
//   remote/build/mac-deps/opus). Off the Mac (`cargo check --target
//   aarch64-apple-darwin` on Linux, see build-swift.sh) the library is absent and
//   nothing is linked: a check never links.
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=csrc/hd_shim.c");
    println!("cargo:rerun-if-changed=csrc/hd_opus.c");
    println!("cargo:rerun-if-changed=csrc/hd_shim.h");
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();
    if os == "macos" {
        mac();
        return;
    }
    let target = std::env::var("TARGET").unwrap();
    let windows = os == "windows";
    let var = format!("PKG_CONFIG_{}", target.replace('-', "_"));
    println!("cargo:rerun-if-env-changed={var}");
    let pkg_config = std::env::var(&var).unwrap_or_else(|_| {
        assert!(!windows, "${var} is not set: source remote/packaging/windows/env.sh first");
        "pkg-config".into()
    });
    let mut b = cc::Build::new();
    b.file("csrc/hd_shim.c").file("csrc/hd_opus.c").flag_if_supported("-Wno-deprecated-declarations");
    // OpenH264 is only a header here (<wels/codec_api.h>): its library is loaded by path at run
    // time (Cisco's downloaded DLL on Windows, the system's elsewhere), never linked. On Windows
    // the headers are in win-deps/include (build-deps.sh), where FFmpeg's .pc files point.
    let mut libs = vec!["libavcodec", "libavutil", "libswscale", "opus"];
    if !windows {
        libs.push("openh264");
    }
    // The generic Linux host (feature linux-portal): VA-API with DMA-BUF import needs
    // libavfilter (hwmap, scale_vaapi).
    if os == "linux" && std::env::var_os("CARGO_FEATURE_LINUX_PORTAL").is_some() {
        b.define("HD_VAAPI", None);
        libs.push("libavfilter");
    }
    for lib in libs {
        let out = Command::new(&pkg_config)
            .args(["--cflags", "--libs", lib])
            .output()
            .unwrap_or_else(|e| panic!("{pkg_config}: {e}"));
        assert!(out.status.success(), "{pkg_config} {lib} failed (Linux: pacman -S ffmpeg openh264 opus; Windows: build-deps.sh)");
        for tok in String::from_utf8(out.stdout).unwrap().split_whitespace() {
            if let Some(dir) = tok.strip_prefix("-I") {
                b.include(dir);
                // <wels/codec_api.h> and <opus.h>: the parent of an include/opus dir too.
                if let Some(parent) = std::path::Path::new(dir).parent() {
                    b.include(parent);
                }
            } else if let Some(dir) = tok.strip_prefix("-L") {
                println!("cargo:rustc-link-search=native={dir}");
            } else if let Some(name) = tok.strip_prefix("-l") {
                // Not OpenH264 (the Linux .pc names it): it is loaded at run time.
                if name != "openh264" {
                    println!("cargo:rustc-link-lib={name}");
                }
            }
        }
    }
    b.compile("hd_shim");
    if windows {
        println!("cargo:rustc-link-lib=winmm");
    }
}

/// macOS: Opus only, static, from `$SODIUM_REMOTE_OPUS` (build-opus-mac.sh's prefix).
fn mac() {
    println!("cargo:rerun-if-env-changed=SODIUM_REMOTE_OPUS");
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let prefix = std::env::var_os("SODIUM_REMOTE_OPUS")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest.join("../build/mac-deps/opus"));
    let mut b = cc::Build::new();
    b.file("csrc/hd_opus.c").include(prefix.join("include")).include(prefix.join("include/opus"));
    b.compile("hd_opus");
    let lib = prefix.join("lib");
    if lib.join("libopus.a").exists() {
        println!("cargo:rustc-link-search=native={}", lib.display());
        println!("cargo:rustc-link-lib=static=opus");
    } else if std::env::var("HOST").is_ok_and(|h| h.contains("apple-darwin")) {
        // On the Mac a missing Opus is a link error later; say why now.
        println!(
            "cargo:warning=no {}: run host-desktop/build-opus-mac.sh first (docs/reference/host-mac.md)",
            lib.join("libopus.a").display()
        );
    }
}
