#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# Builds ICU for Windows (mingw-w64, shared DLLs) into remote/build/win-deps/{include,lib,bin}.
# Sodium UI's formatters, calendars, locale display names and search transliteration use ICU
# directly (ui/native: LocaleFormats, CalendarInfo, I18n, TextMatch), and Qt for Windows is not
# built on it, so Windows ships its own copy.
#
#   remote/packaging/windows/build-icu.sh
#
# Same ICU version as the Linux desktop (Arch's icu 78.3), from the official release, pinned by
# SHA-256 (also checked against the release's SHASUM512.txt when this version was chosen).
# A cross build needs a native build of the same source first (its tools make the data);
# both live under remote/build/win-deps/work (deletable). Idempotent: stamped.
set -euo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REMOTE=$(cd "$HERE/../.." && pwd)
PREFIX=${WIN_DEPS:-$REMOTE/build/win-deps}
SRC=$PREFIX/src
WORK=$PREFIX/work
STAMP=$PREFIX/.stamps
JOBS=${JOBS:-8}
TRIPLET=x86_64-w64-mingw32

ICU_VER=78.3
ICU_URL=https://github.com/unicode-org/icu/releases/download/release-$ICU_VER/icu4c-$ICU_VER-sources.tgz
ICU_SHA=3a2e7a47604ba702f345878308e6fefeca612ee895cf4a5f222e7955fabfe0c0

mkdir -p "$SRC" "$WORK" "$STAMP" "$PREFIX"/{include,lib,bin}
[[ -f $STAMP/icu-$ICU_VER && -z ${FORCE:-} ]] && { echo "icu $ICU_VER: already built ($PREFIX)"; exit 0; }

tgz=$SRC/icu4c-$ICU_VER-sources.tgz
[[ -f $tgz ]] || curl -fL --retry 3 -o "$tgz" "$ICU_URL"
echo "$ICU_SHA  $tgz" | sha256sum -c - >/dev/null || { echo "icu: SHA-256 mismatch" >&2; exit 1; }

S=$WORK/icu/icu/source
if [[ ! -d $S || -n ${FORCE:-} ]]; then
    rm -rf "$WORK/icu"
    mkdir -p "$WORK/icu"
    tar -C "$WORK/icu" -xzf "$tgz"
fi

# 1. native build (its tools make the data; no tests or samples); kept between runs
if [[ ! -f $WORK/icu/host/.built ]]; then
    rm -rf "$WORK/icu/host"; mkdir -p "$WORK/icu/host"
    ( cd "$WORK/icu/host"
      env -u CC -u CXX -u AR -u RANLIB "$S/configure" --disable-tests --disable-samples --disable-extras --disable-layoutex >/dev/null
      make -j"$JOBS" >/dev/null
      touch .built )
fi

# 2. the Windows build. --build is given so that configure never tries to run its test programs
# (with wine registered in binfmt_misc it would, in the user's own wine prefix).
rm -rf "$WORK/icu/win"; mkdir -p "$WORK/icu/win"
( cd "$WORK/icu/win"
  "$S/configure" --build=x86_64-pc-linux-gnu --host=$TRIPLET --with-cross-build="$WORK/icu/host" --prefix="$PREFIX" \
      --enable-shared --disable-static --disable-tests --disable-samples --disable-extras \
      --disable-layoutex --disable-tools \
      CC=$TRIPLET-gcc CXX=$TRIPLET-g++ AR=$TRIPLET-ar RANLIB=$TRIPLET-ranlib \
      CFLAGS="-O2" CXXFLAGS="-O2" >/dev/null
  make -j"$JOBS" >/dev/null
  make install >/dev/null )

# mingw's installer puts the DLLs in lib/ (icuuc78.dll): the staging step wants them in bin/
for dll in "$PREFIX"/lib/icu*.dll; do [[ -f $dll ]] && mv -f "$dll" "$PREFIX/bin/"; done
mkdir -p "$PREFIX/licenses"
cp "$S/../LICENSE" "$HERE/licenses/ICU-LICENSE.txt" 2>/dev/null || true
touch "$STAMP/icu-$ICU_VER"
echo "icu $ICU_VER built into $PREFIX"
