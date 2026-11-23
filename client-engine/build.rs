// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
// Compiles the libavcodec shim (C, so FFmpeg 9's struct layouts come from its
// own headers) and links FFmpeg. Needs ffmpeg (libavcodec, libavutil); for
// x86_64-pc-windows-gnu the ones in remote/build/win-deps (source
// packaging/windows/env.sh), found with $PKG_CONFIG_x86_64_pc_windows_gnu.
fn main() {
    println!("cargo:rerun-if-changed=csrc/decode_shim.c");
    println!("cargo:rerun-if-changed=csrc/decode_shim.h");
    let target = std::env::var("TARGET").unwrap_or_default();
    let windows = target.contains("windows");
    let pkg_config = std::env::var(format!("PKG_CONFIG_{}", target.replace('-', "_"))).unwrap_or_else(|_| "pkg-config".into());
    let run = |args: &[&str]| {
        let out = std::process::Command::new(&pkg_config).args(args).output().expect("pkg-config not found");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let mut b = cc::Build::new();
    b.file("csrc/decode_shim.c").flag_if_supported("-Wno-deprecated-declarations");
    for f in run(&["--cflags", "libavcodec", "libavutil"]).split_whitespace() {
        if let Some(dir) = f.strip_prefix("-I") {
            b.include(dir);
        }
    }
    b.compile("decode_shim");
    if windows {
        for f in run(&["--libs", "libavcodec", "libavutil"]).split_whitespace() {
            if let Some(dir) = f.strip_prefix("-L") {
                println!("cargo:rustc-link-search=native={dir}");
            } else if let Some(lib) = f.strip_prefix("-l") {
                println!("cargo:rustc-link-lib={lib}");
            }
        }
    } else {
        println!("cargo:rustc-link-lib=avcodec");
        println!("cargo:rustc-link-lib=avutil");
    }
}
