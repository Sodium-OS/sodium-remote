#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# host-desktop for the Mac: the type check off the Mac, the Swift
# bindings, and on the Mac the one static library Remote.app links.
#
#   build-swift.sh check-mac            cargo check for aarch64-apple-darwin, with and without
#                                       the `ffi` feature (works on Linux: no linking)
#   build-swift.sh bindings <out dir>   RemoteCore.swift and SodiumRemoteHost.swift, their
#                                       headers and one module.modulemap for both (works on Linux)
#   build-swift.sh mac <out dir>        on the Mac: libremote_ffi.a (host-desktop and
#                                       remote-core together), plus the bindings
#
# One library, one Swift module: host-desktop's `ffi` feature turns on remote-core's, so
# libhost_desktop.a holds both crates' UniFFI scaffolding (and one copy of Rust's std).
# The app links that library instead of libremote_core.a. Both crates' Swift files
# (RemoteCore.swift, SodiumRemoteHost.swift: each crate's uniffi.toml) go in the app's
# RemoteCore target, where SodiumRemoteHost.swift finds core's `Os` converter; both
# headers go in the RemoteCoreFFI C module, whose modulemap lists them.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
WS=$(cd "$HERE/.." && pwd)
MANIFEST="$WS/Cargo.toml"

# Off the Mac there's no macOS SDK, so build scripts can't compile C for darwin (ring,
# our Opus shim). `cargo check` never links, so a stub compiler that writes empty objects
# is enough to type-check every Rust crate (as core/build-swift.sh check-mac does).
stub_cc() {
  [ "$(uname)" = Darwin ] && return
  local stub="$WS/build/mac-check"
  mkdir -p "$stub"
  cat > "$stub/cc" <<'STUB'
#!/bin/sh
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
}

bindings() {
  local out=${1:?out dir}
  local profile=${2:-debug}
  local flag=()
  [ "$profile" = release ] && flag=(--release)
  # A cdylib only for the bindgen to read; the crate itself stays an rlib.
  cargo rustc --manifest-path "$MANIFEST" -p host-desktop --lib --features ffi "${flag[@]}" --crate-type cdylib
  local lib="$WS/target/$profile/libhost_desktop.so"
  [ -f "$lib" ] || lib="$WS/target/$profile/libhost_desktop.dylib"
  mkdir -p "$out"
  out=$(cd "$out" && pwd)
  cd "$WS"
  # remote-core's bindgen reads every crate's uniffi.toml through `cargo metadata`.
  cargo run --manifest-path "$MANIFEST" -q -p remote-core --features uniffi-cli --bin uniffi-bindgen -- \
    generate --library "$lib" --language swift --out-dir "$out"
  # One C module for both headers (each generated modulemap names only its own).
  rm -f "$out"/*.modulemap
  {
    echo "module RemoteCoreFFI {"
    for h in "$out"/*.h; do echo "    header \"$(basename "$h")\""; done
    echo "    export *"
    echo "}"
  } > "$out/module.modulemap"
  echo "wrote $(ls "$out" | tr '\n' ' ')to $out"
}

case "${1:-}" in
  check-mac)
    rustup target list --installed | grep -qx aarch64-apple-darwin || rustup target add aarch64-apple-darwin
    stub_cc
    cargo check --manifest-path "$MANIFEST" -p host-desktop --target aarch64-apple-darwin --bins --examples
    cargo check --manifest-path "$MANIFEST" -p host-desktop --target aarch64-apple-darwin --lib --features ffi
    ;;
  bindings) bindings "${2:-}" ;;
  mac)
    out=${2:?out dir}
    [ "$(uname)" = Darwin ] || { echo "build-swift.sh mac: run this on the Mac" >&2; exit 1; }
    export MACOSX_DEPLOYMENT_TARGET=${MACOSX_DEPLOYMENT_TARGET:-14.0}
    "$HERE/build-opus-mac.sh"
    mkdir -p "$out"
    # The frameworks and libraries the app must link with it (rustc prints them when it
    # builds the library; docs/reference/host-mac.md lists them too).
    cargo rustc --manifest-path "$MANIFEST" -p host-desktop --lib --features ffi --release --crate-type staticlib \
      --target aarch64-apple-darwin -- --print native-static-libs 2>&1 | tee "$out/staticlib.log"
    grep -o 'native-static-libs: .*' "$out/staticlib.log" > "$out/native-static-libs.txt" || true
    cp "$WS/target/aarch64-apple-darwin/release/libhost_desktop.a" "$out/libremote_ffi.a"
    bindings "$out" release
    ;;
  *) sed -n '2,16p' "$0"; exit 2 ;;
esac
