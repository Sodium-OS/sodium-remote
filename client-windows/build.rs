// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
// Links Opus (static, from remote/build/win-deps via $WIN_DEPS: source
// packaging/windows/env.sh) for the audio decoder.
fn main() {
    let target = std::env::var("TARGET").unwrap_or_default();
    if !target.contains("windows") {
        return;
    }
    let deps = std::env::var("WIN_DEPS").expect("WIN_DEPS is not set: . remote/packaging/windows/env.sh");
    println!("cargo:rerun-if-env-changed=WIN_DEPS");
    println!("cargo:rustc-link-search=native={deps}/lib");
    println!("cargo:rustc-link-lib=static=opus");
}
