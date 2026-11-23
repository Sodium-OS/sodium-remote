#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# The Swift side of remote-core.
#
#   build-swift.sh bindings <out dir>   generate RemoteCore.swift, RemoteCoreFFI.h and
#                                       RemoteCoreFFI.modulemap (works on Linux)
#   build-swift.sh check-mac            cargo check for aarch64-apple-darwin (no linking)
#   build-swift.sh mac <out dir>        on the Mac: the static library for Xcode (libremote_ffi.a,
#                                       core only; host-desktop/build-swift.sh makes the app's), plus bindings
#
# The bindings come from the compiled library's metadata, so they always match
# the Rust they were generated from.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
WS=$(cd "$HERE/.." && pwd)
MANIFEST="$WS/Cargo.toml"

bindings() {
  local out=${1:?out dir}
  local profile=${2:-debug}
  local flag=()
  [ "$profile" = release ] && flag=(--release)
  # A cdylib only for the bindgen to read; the crate itself stays an rlib.
  cargo rustc --manifest-path "$MANIFEST" -p remote-core --lib --features ffi "${flag[@]}" --crate-type cdylib
  local lib="$WS/target/$profile/libremote_core.so"
  [ -f "$lib" ] || lib="$WS/target/$profile/libremote_core.dylib"
  mkdir -p "$out"
  out=$(cd "$out" && pwd)
  # The bindgen reads uniffi.toml through `cargo metadata` in the workspace.
  cd "$WS"
  cargo run --manifest-path "$MANIFEST" -q -p remote-core --features uniffi-cli --bin uniffi-bindgen -- \
    generate --library "$lib" --language swift --out-dir "$out"
  echo "wrote $(ls "$out" | tr '\n' ' ')to $out"
}

case "${1:-}" in
  bindings) bindings "${2:-}" ;;
  check-mac)
    rustup target list --installed | grep -qx aarch64-apple-darwin || rustup target add aarch64-apple-darwin
    if [ "$(uname)" != Darwin ]; then
      # Off the Mac there's no macOS SDK, so ring's build script can't compile
      # its C for darwin. `cargo check` never links, so a stub compiler that
      # writes empty objects is enough to type-check every Rust crate.
      stub=$(mktemp -d)
      trap 'rm -rf "$stub"' EXIT
      cat > "$stub/cc" <<'STUB'
#!/bin/sh
# cc-rs first asks the compiler family (answer: clang, not clang-cl), then
# compiles with -o.
case " $* " in
  *detect_compiler_family.c*) echo '"clang"'; echo '"gcc"'; exit 0 ;;
  *" -? "*) exit 1 ;;
esac
out=""
while [ $# -gt 0 ]; do [ "$1" = -o ] && { out=$2; shift; }; shift; done
[ -n "$out" ] && : > "$out"
exit 0
STUB
      chmod +x "$stub/cc"
      export CC_aarch64_apple_darwin="$stub/cc" AR_aarch64_apple_darwin=ar
    fi
    cargo check --manifest-path "$MANIFEST" -p remote-core --features ffi --target aarch64-apple-darwin
    ;;
  mac)
    out=${2:?out dir}
    cargo rustc --manifest-path "$MANIFEST" -p remote-core --lib --features ffi --release --crate-type staticlib \
      --target aarch64-apple-darwin
    mkdir -p "$out"
    cp "$WS/target/aarch64-apple-darwin/release/libremote_core.a" "$out/libremote_ffi.a"
    bindings "$out" release
    ;;
  *) sed -n '2,10p' "$0"; exit 2 ;;
esac
