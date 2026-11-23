#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# libopus as a static library for host-desktop on the Mac.
# Run on the Mac (Command Line Tools: clang, make), once:
#
#   host-desktop/build-opus-mac.sh [PREFIX]     default PREFIX: remote/build/mac-deps/opus
#
# The same release and SHA-256 as the Windows build (packaging/windows/build-deps.sh).
# host-desktop's build.rs links $SODIUM_REMOTE_OPUS/lib/libopus.a (default: that PREFIX).
# Idempotent: an existing PREFIX/lib/libopus.a is kept (FORCE=1 rebuilds).
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
REMOTE=$(cd "$HERE/.." && pwd)
PREFIX=${1:-$REMOTE/build/mac-deps/opus}
OPUS_VER=1.6.1            # xiph opus (BSD-3)
OPUS_URL=https://downloads.xiph.org/releases/opus/opus-$OPUS_VER.tar.gz
OPUS_SHA=6ffcb593207be92584df15b32466ed64bbec99109f007c82205f0194572411a1

[ "$(uname -s)" = Darwin ] || { echo "build-opus-mac.sh: run this on the Mac" >&2; exit 1; }
if [ -f "$PREFIX/lib/libopus.a" ] && [ "${FORCE:-0}" != 1 ]; then
  echo "libopus.a is already in $PREFIX/lib"
  exit 0
fi
# Same minimum as the Swift package and build-app.sh.
export MACOSX_DEPLOYMENT_TARGET=${MACOSX_DEPLOYMENT_TARGET:-14.0}
WORK=$REMOTE/build/mac-deps/src
mkdir -p "$WORK"
TAR=$WORK/opus-$OPUS_VER.tar.gz
[ -f "$TAR" ] || curl -fL --proto '=https' -o "$TAR" "$OPUS_URL"
got=$(shasum -a 256 "$TAR" | cut -d' ' -f1)
if [ "$got" != "$OPUS_SHA" ]; then
  echo "build-opus-mac.sh: $TAR has SHA-256 $got, expected $OPUS_SHA" >&2
  rm -f "$TAR"
  exit 1
fi
rm -rf "$WORK/opus-$OPUS_VER"
tar -xzf "$TAR" -C "$WORK"
cd "$WORK/opus-$OPUS_VER"
./configure --prefix="$PREFIX" --disable-shared --enable-static --disable-doc --disable-extra-programs \
  CFLAGS="-O2 -arch arm64 -mmacosx-version-min=$MACOSX_DEPLOYMENT_TARGET"
make -j"$(sysctl -n hw.ncpu)"
make install
cp COPYING "$PREFIX/opus-BSD-3-Clause.txt"
echo "libopus $OPUS_VER in $PREFIX"
