#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# CI recipe: builds the Qt modules the app needs from source for Windows on UCRT, into remote/build/qt-win-ucrt/<version>/.
#
#   remote/packaging/windows/build-qt-ucrt.sh [--print-pins]
#   QT_SRC_DIR=/path/to/unpacked/sources remote/packaging/windows/build-qt-ucrt.sh
#
# Why: Qt's official mingw build is made against the msvcrt C runtime, our toolchain (Arch's
# mingw-w64 GCC) against UCRT, and a package with both runtimes is a trap (README, "Two C
# runtimes"). The same GCC builds Qt here, so every DLL of the package imports UCRT only;
# check-no-msvcrt.sh is the gate.
#
# Modules, in this order (the ones the app and the vendored Sodium UI use; sodium-ui/CMakeLists.txt
# asks for Core Gui Qml Quick Svg ShaderTools Network, and Qt Quick Controls comes with qtdeclarative):
#   qtbase  qtshadertools  qtdeclarative  qtsvg
# Host tools (moc, rcc, qsb, qmlcachegen, ...) come from the Linux Qt at /usr (QT_HOST_PATH), which
# must be the same version. Needs: x86_64-w64-mingw32-{gcc,g++,windres}, cmake, ninja, curl (not
# with QT_SRC_DIR), tar, xz, sha256sum, pkgconf. A full build takes hours and several GB: never in /tmp.
#
# Sources: the official submodule tarballs on download.qt.io, each pinned by SHA-256 below. FAILS
# CLOSED: with a placeholder pin it refuses to download or build; a maintainer or CI runs
# `build-qt-ucrt.sh --print-pins` (it fetches Qt's own published .sha256 files) and pastes the
# values here. QT_SRC_DIR names an already unpacked tree instead (folders qtbase, qtshadertools,
# qtdeclarative, qtsvg, or their `-everywhere-src-<version>` names, inside it); nothing is
# downloaded then and the pins are not consulted.
#
# Qt itself is built without ICU, as Qt's own Windows builds are (Sodium UI's native part uses the
# ICU of build-icu.sh directly, found through win-deps). QT_ICU=1 builds Qt against that ICU too.
# Output: remote/build/qt-win-ucrt/<version>/{bin,lib,plugins,qml,...}. Use it with
#   QT_WIN_ROOT=remote/build/qt-win-ucrt/6.12.0  (toolchain-mingw.cmake reads it)
# d3dcompiler_47.dll and opengl32sw.dll are not Qt code: they still come from fetch-qt.sh's archives.
# Idempotent: a finished module is skipped (FORCE=1 redoes all). JOBS sets the parallelism.
set -euo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REMOTE=$(cd "$HERE/../.." && pwd)
QT_VER=6.12.0
QT_MINOR=${QT_VER%.*}
OUT=${QT_UCRT_OUT:-$REMOTE/build/qt-win-ucrt}
PREFIX=$OUT/$QT_VER
SRC=$OUT/src
WORK=$OUT/work
STAMP=$OUT/.stamps
WIN_DEPS=${WIN_DEPS:-$REMOTE/build/win-deps}
JOBS=${JOBS:-8}
TRIPLET=x86_64-w64-mingw32
QT_HOST_PATH=${QT_HOST_PATH:-/usr}
MODULES=(qtbase qtshadertools qtdeclarative qtsvg)

BASE_URL=https://download.qt.io/official_releases/qt/$QT_MINOR/$QT_VER/submodules
# SHA-256 of <module>-everywhere-src-$QT_VER.tar.xz. PLACEHOLDERS until a maintainer or CI runs --print-pins.
declare -A QT_SHA=(
  [qtbase]=UNPINNED-run-build-qt-ucrt.sh---print-pins
  [qtshadertools]=UNPINNED-run-build-qt-ucrt.sh---print-pins
  [qtdeclarative]=UNPINNED-run-build-qt-ucrt.sh---print-pins
  [qtsvg]=UNPINNED-run-build-qt-ucrt.sh---print-pins
)

log() { printf '\n==== %s\n' "$*" >&2; }
die() { echo "build-qt-ucrt: $*" >&2; exit 1; }
is_sha() { [[ $1 =~ ^[0-9a-fA-F]{64}$ ]]; }

if [[ ${1:-} == --print-pins ]]; then
  command -v curl >/dev/null || die "missing tool: curl"
  for m in "${MODULES[@]}"; do
    f=$m-everywhere-src-$QT_VER.tar.xz
    sum=$(curl -fsSL "$BASE_URL/$f.sha256" | awk '{print $1}')
    is_sha "$sum" || die "no SHA-256 published for $f"
    printf '  [%s]=%s\n' "$m" "$sum"
  done
  echo "Paste these into the QT_SHA table of $(basename "${BASH_SOURCE[0]}")." >&2
  exit 0
elif [[ $# -gt 0 ]]; then
  die "usage: build-qt-ucrt.sh [--print-pins]"
fi

# ---- fail closed on the pins (unless the sources are already here) ----
if [[ -z ${QT_SRC_DIR:-} ]]; then
  for m in "${MODULES[@]}"; do
    is_sha "${QT_SHA[$m]}" || die "the SHA-256 of $m is not pinned (maintainer/CI step: run build-qt-ucrt.sh --print-pins and paste the result into this script), or set QT_SRC_DIR to an unpacked source tree"
  done
fi

for t in $TRIPLET-gcc $TRIPLET-g++ $TRIPLET-windres cmake ninja tar xz sha256sum pkg-config; do
  command -v "$t" >/dev/null || die "missing tool: $t"
done
[[ -z ${QT_SRC_DIR:-} ]] && { command -v curl >/dev/null || die "missing tool: curl"; }
[[ -d $QT_HOST_PATH/lib/qt6 || -d $QT_HOST_PATH/lib/cmake/Qt6 ]] || die "no Linux Qt at QT_HOST_PATH=$QT_HOST_PATH"
host_ver=$(pkg-config --modversion Qt6Core 2>/dev/null || true)
[[ $host_ver == "$QT_VER" ]] || die "the Linux Qt (host tools) is ${host_ver:-not found}, Qt for Windows is built as $QT_VER: they must match"

mkdir -p "$SRC" "$WORK" "$STAMP" "$PREFIX"

# ---- sources ----
src_of() { # module -> its source directory
  local m=$1 d
  if [[ -n ${QT_SRC_DIR:-} ]]; then
    for d in "$QT_SRC_DIR/$m" "$QT_SRC_DIR/$m-everywhere-src-$QT_VER"; do
      [[ -f $d/CMakeLists.txt ]] && { echo "$d"; return; }
    done
    die "no $m under QT_SRC_DIR=$QT_SRC_DIR"
  fi
  local f=$m-everywhere-src-$QT_VER.tar.xz tgz=$SRC/$m-everywhere-src-$QT_VER.tar.xz
  if ! { [[ -f $tgz ]] && echo "${QT_SHA[$m]}  $tgz" | sha256sum -c --status; }; then
    rm -f "$tgz"
    log "download $BASE_URL/$f"
    curl -fL --retry 3 -o "$tgz.part" "$BASE_URL/$f" >&2
    mv "$tgz.part" "$tgz"
    echo "${QT_SHA[$m]}  $tgz" | sha256sum -c --status || { rm -f "$tgz"; die "SHA-256 mismatch: $f"; }
  fi
  d=$WORK/$m-everywhere-src-$QT_VER
  if [[ ! -f $d/CMakeLists.txt ]]; then
    rm -rf "$d"
    tar -C "$WORK" -xf "$tgz"
  fi
  echo "$d"
}

# ---- the toolchain: the same GCC, UCRT explicit, nothing of the official Qt on the search paths ----
TOOLCHAIN=$WORK/toolchain-ucrt.cmake
write_toolchain() {
  cat >"$TOOLCHAIN" <<EOF
set(CMAKE_SYSTEM_NAME Windows)
set(CMAKE_SYSTEM_PROCESSOR x86_64)
set(CMAKE_C_COMPILER $TRIPLET-gcc)
set(CMAKE_CXX_COMPILER $TRIPLET-g++)
set(CMAKE_RC_COMPILER $TRIPLET-windres)
set(CMAKE_AR $TRIPLET-ar)
set(CMAKE_RANLIB $TRIPLET-ranlib)
# UCRT, spelled out: the C runtime Qt's DLLs link must be the one the rest of the package uses.
set(CMAKE_C_FLAGS_INIT "-mcrtdll=ucrt")
set(CMAKE_CXX_FLAGS_INIT "-mcrtdll=ucrt")
set(CMAKE_EXE_LINKER_FLAGS_INIT "-mcrtdll=ucrt")
set(CMAKE_SHARED_LINKER_FLAGS_INIT "-mcrtdll=ucrt")
set(CMAKE_MODULE_LINKER_FLAGS_INIT "-mcrtdll=ucrt")
set(CMAKE_FIND_ROOT_PATH "$PREFIX;$WIN_DEPS;/usr/$TRIPLET")
set(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM NEVER)
set(CMAKE_FIND_ROOT_PATH_MODE_LIBRARY ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_INCLUDE ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_PACKAGE ONLY)
EOF
}

build_module() { # module
  local m=$1 stamp=$STAMP/$1-$QT_VER src b
  if [[ -f $stamp && -z ${FORCE:-} ]]; then echo "skip $m ($QT_VER already built)"; return; fi
  src=$(src_of "$m")
  b=$WORK/build-$m
  rm -rf "$b"
  log "configure $m"
  local common=(-G Ninja -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX="$PREFIX"
    -DQT_HOST_PATH="$QT_HOST_PATH" -DQT_HOST_PATH_CMAKE_DIR="$QT_HOST_PATH/lib/cmake"
    -DBUILD_SHARED_LIBS=ON -DQT_BUILD_EXAMPLES=OFF -DQT_BUILD_TESTS=OFF -DQT_BUILD_BENCHMARKS=OFF)
  if [[ $m == qtbase ]]; then
    local icu=(-DFEATURE_icu=OFF)
    [[ ${QT_ICU:-} == 1 ]] && icu=(-DFEATURE_icu=ON -DICU_ROOT="$WIN_DEPS")
    cmake -S "$src" -B "$b" "${common[@]}" -DCMAKE_TOOLCHAIN_FILE="$TOOLCHAIN" "${icu[@]}" \
      -DFEATURE_dbus=OFF -DFEATURE_sql=OFF -DFEATURE_testlib=OFF >&2
  else
    # The other modules find qtbase (and each other) through the toolchain file qtbase wrote.
    cmake -S "$src" -B "$b" "${common[@]}" -DCMAKE_TOOLCHAIN_FILE="$PREFIX/lib/cmake/Qt6/qt.toolchain.cmake" \
      -DCMAKE_PREFIX_PATH="$PREFIX" >&2
  fi
  [[ ${QT_CONFIGURE_ONLY:-} == 1 ]] && { echo "configured $m (QT_CONFIGURE_ONLY)"; return; }
  log "build $m"
  cmake --build "$b" --parallel "$JOBS" >&2
  cmake --install "$b" >&2
  touch "$stamp"
}

write_toolchain
for m in "${MODULES[@]}"; do
  if [[ ${QT_CONFIGURE_ONLY:-} == 1 && $m != qtbase ]]; then break; fi
  build_module "$m"
done
log "done: $PREFIX (check the package with check-no-msvcrt.sh)"
