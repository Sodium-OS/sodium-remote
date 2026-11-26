#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# Cross-builds Sodium UI and its gallery for Windows x64 on Linux, and stages the program with
# every DLL and the QML tree it needs (what windeployqt would do) into remote/build/win-gallery/.
#
#   remote/packaging/windows/build-sodiumui.sh [--no-stage]
#
# Needs: the mingw-w64 toolchain (pacman: mingw-w64-gcc), Linux Qt 6.12.1 (qt6-base,
# qt6-declarative, qt6-shadertools: moc, rcc, qsb and qmlimportscanner run on this PC), cmake,
# ninja, curl, 7z. It fetches Qt for Windows (fetch-qt.sh) and builds ICU (build-icu.sh) first
# when they are missing. Nothing is installed anywhere: all output is under remote/build/.
# Jobs: JOBS (default 8). Never builds in /tmp (a RAM disk).
#
# Output:
#   remote/build/sodiumui-win/      the CMake build tree
#   remote/build/win-gallery/       the program: sodiumui-gallery.exe, DLLs, qml/, SodiumUI/, fonts/ ...
set -euo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REMOTE=$(cd "$HERE/../.." && pwd)
REPO=$REMOTE
export WIN_DEPS=${WIN_DEPS:-$REMOTE/build/win-deps}
export QT_WIN_ROOT=${QT_WIN_ROOT:-$REMOTE/build/qt-win/6.12.1/mingw_64}
BUILD=${SODIUMUI_WIN_BUILD:-$REMOTE/build/sodiumui-win}
OUT=${SODIUMUI_WIN_OUT:-$REMOTE/build/win-gallery}
JOBS=${JOBS:-8}

for t in x86_64-w64-mingw32-g++ cmake ninja python3 /usr/lib/qt6/qmlimportscanner; do
  command -v "$t" >/dev/null || { echo "missing tool: $t" >&2; exit 1; }
done

# the host Qt must be the same version as Qt for Windows, or moc/qsb output would not match
host_ver=$(/usr/lib/qt6/moc --version | awk '{print $NF}')
[[ $host_ver == 6.12.1 ]] || { echo "the Linux Qt is $host_ver; Qt for Windows is 6.12.1 (QT_HOST_PATH must match)" >&2; exit 1; }

"$HERE/fetch-qt.sh"
JOBS=$JOBS "$HERE/build-icu.sh"

cmake -S "$REPO/sodium-ui" -B "$BUILD" -G Ninja -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_TOOLCHAIN_FILE="$HERE/toolchain-mingw.cmake"
cmake --build "$BUILD" -j"$JOBS"

[[ ${1:-} == --no-stage ]] && exit 0

# translations the staged program carries (skipped with a note when qt6-tools is missing)
"$REPO/i18n/i18n.sh" release --optional >/dev/null || true
# the vendored Sodium UI has no gallery program: it is staged only as part of the app (stage-app.sh)
echo "Sodium UI built for Windows: $BUILD/SodiumUI"
