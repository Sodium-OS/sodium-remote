// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
// Compiles the colour-conversion shader (glslc, from shaderc) and the NVENC/CUDA
// shim (C, against ffnvcodec-headers: the NVENC structs are big, versioned and
// full of bitfields, so the C compiler lays them out, not hand-written FFI).
use std::{env, path::PathBuf, process::Command};

fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());

    println!("cargo:rerun-if-changed=shaders/rgb_to_yuv.comp");
    let status = Command::new("glslc")
        .args(["-O", "--target-env=vulkan1.3", "shaders/rgb_to_yuv.comp", "-o"])
        .arg(out.join("rgb_to_yuv.spv"))
        .status()
        .expect("glslc not found (pacman -S shaderc)");
    assert!(status.success(), "glslc failed");

    println!("cargo:rerun-if-changed=csrc/nvenc_shim.c");
    println!("cargo:rerun-if-changed=csrc/nvenc_shim.h");
    cc::Build::new()
        .file("csrc/nvenc_shim.c")
        .flag_if_supported("-Wno-missing-field-initializers")
        .compile("nvenc_shim");
    println!("cargo:rustc-link-lib=dl");
    println!("cargo:rustc-link-lib=gbm");

    // The audio shim (PipeWire, Opus) is in remote-linux-media now; its libraries come
    // with that crate.
    // rustc-link-lib reaches only the library target (src/lib.rs). main.rs still
    // compiles its own copy of capture, gpu and nvenc rather than using the library,
    // so its binary needs the shim and its libraries named here too. Redundant (and
    // harmless) once main.rs uses `remoted::...`.
    for lib in ["nvenc_shim", "gbm", "dl"] {
        println!("cargo:rustc-link-arg-bin=remoted=-l{lib}");
    }
}
