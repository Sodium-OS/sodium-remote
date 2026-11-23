// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
// Compiles the PipeWire shims (csrc/audio_shim.c, csrc/lm_video.c) against the system's
// libpipewire-0.3 and Opus. Linux only: elsewhere the crate is empty.
use std::process::Command;

fn main() {
    for f in ["csrc/audio_shim.c", "csrc/audio_shim.h", "csrc/lm_video.c", "csrc/lm_video.h"] {
        println!("cargo:rerun-if-changed={f}");
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        return;
    }
    let mut b = cc::Build::new();
    b.file("csrc/audio_shim.c").file("csrc/lm_video.c");
    for lib in ["libpipewire-0.3", "opus"] {
        let out = Command::new("pkg-config").args(["--cflags", "--libs", lib]).output().expect("pkg-config not found");
        assert!(out.status.success(), "pkg-config {lib} failed (pacman -S pipewire opus)");
        for tok in String::from_utf8(out.stdout).unwrap().split_whitespace() {
            if let Some(dir) = tok.strip_prefix("-I") {
                b.include(dir);
            } else if let Some(dir) = tok.strip_prefix("-L") {
                println!("cargo:rustc-link-search=native={dir}");
            } else if let Some(name) = tok.strip_prefix("-l") {
                println!("cargo:rustc-link-lib={name}");
            } else if tok.starts_with("-D") || tok.starts_with("-f") {
                b.flag(tok);
            }
        }
    }
    b.flag_if_supported("-Wno-missing-field-initializers").compile("remote_linux_media");
}
