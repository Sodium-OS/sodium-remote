// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
// Compiles the PipeWire playback shim (csrc/audio_out.c) and links PipeWire and
// Opus, found with pkg-config (pacman -S pipewire opus).
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=csrc/audio_out.c");
    println!("cargo:rerun-if-changed=csrc/audio_out.h");
    let mut b = cc::Build::new();
    b.file("csrc/audio_out.c");
    for lib in ["libpipewire-0.3", "opus"] {
        let out = Command::new("pkg-config").args(["--cflags", "--libs", lib]).output().expect("pkg-config not found");
        assert!(out.status.success(), "pkg-config {lib} failed (pacman -S pipewire opus)");
        for tok in String::from_utf8(out.stdout).unwrap().split_whitespace() {
            if let Some(dir) = tok.strip_prefix("-I") {
                b.include(dir);
            } else if let Some(name) = tok.strip_prefix("-l") {
                println!("cargo:rustc-link-lib={name}");
            } else if tok.starts_with("-D") || tok.starts_with("-f") {
                b.flag(tok);
            }
        }
    }
    b.flag_if_supported("-Wno-missing-field-initializers").compile("audio_out");
}
