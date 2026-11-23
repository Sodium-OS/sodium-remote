#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# Builds Sodium Remote for Windows (SodiumRemote.exe) and stages the portable package:
#   remote/build/win-app/SodiumRemote/                  the program, Qt, ICU, FFmpeg/Opus DLLs, QML, fonts, icons, translations, licenses
#   remote/build/SodiumRemote-<version>-win64.zip       the same, zipped (portable; no installer, no code signing)
#
#   stage-app.sh [--no-ui] [--no-zip]
#
# Needs what build-sodiumui.sh needs (Qt for Windows from fetch-qt.sh, ICU, win-deps from build-deps.sh, the Linux Qt 6.12
# for moc/rcc/qsb), the cargo target x86_64-pc-windows-gnu, zip and the app's own icons (app/data/icons). Jobs: JOBS (default 8). Output only under remote/build/.
set -euo pipefail
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REMOTE=$(cd "$HERE/../.." && pwd)
REPO=$REMOTE
. "$HERE/env.sh"
export QT_WIN_ROOT=${QT_WIN_ROOT:-$REMOTE/build/qt-win/6.12.0/mingw_64}
JOBS=${JOBS:-8}
UI_BUILD=${SODIUMUI_WIN_BUILD:-$REMOTE/build/sodiumui-win}
APP_BUILD=$REMOTE/build/app-win
OUT=$REMOTE/build/win-app/SodiumRemote
ICONS=${ICONS:-$REMOTE/app/data/icons/sodium-remote}
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' "$REMOTE/Cargo.toml" | head -1)
NO_UI=0 NO_ZIP=0
for a in "$@"; do
  case $a in --no-ui) NO_UI=1 ;; --no-zip) NO_ZIP=1 ;; *) echo "unknown option $a" >&2; exit 2 ;; esac
done

# 1. Sodium UI (the plugin with WindowChrome), unless it is already built
if [[ $NO_UI == 0 ]]; then JOBS=$JOBS "$HERE/build-sodiumui.sh" --no-stage; fi

# 2. the engine: remote-qt's static library (client engine, Windows platform layer, in-process host)
"$HERE/cargo-win.sh" build --release -p remote-qt
cp -n "$(ls ~/.cargo/registry/src/*/windows_x86_64_gnu-0.52.*/lib/libwindows.0.52.0.a | head -1)" "$WIN_DEPS/lib/" || true

# 3. the app
cmake -S "$HERE/app" -B "$APP_BUILD" -G Ninja -DCMAKE_BUILD_TYPE=Release -DCMAKE_TOOLCHAIN_FILE="$HERE/toolchain-mingw.cmake" >/dev/null
cmake --build "$APP_BUILD" -j"$JOBS"

# 4. translations, then the package
"$REMOTE/i18n/i18n.sh" release --optional >/dev/null || true
python3 -I "$HERE/stage-sodiumui.py" --build "$UI_BUILD" --exe-from "$APP_BUILD/bin" --exe SodiumRemote.exe \
  --qml-dir "$REMOTE/app/qml" --no-gallery --catalog remote --out "$OUT" --qt "$QT_WIN_ROOT" --deps "$WIN_DEPS" --repo "$REPO" --icons "$ICONS"
mkdir -p "$OUT/licenses"
cp -r "$HERE/licenses/." "$OUT/licenses/"
cat >"$OUT/licenses/README.txt" <<'EOF'
Sodium Remote bundles:
  Qt 6.12 (LGPL-3.0, linked dynamically: the Qt DLLs beside the program can be replaced) - https://www.qt.io
  FFmpeg 9.0 (LGPL-2.1+, built without GPL or nonfree parts, shared DLLs: avcodec, avutil, swscale)
  OpenH264 Video Codec provided by Cisco Systems, Inc. (Cisco's own prebuilt DLL, downloaded when first needed; not in this package)
  Opus 1.6 (BSD-3-Clause), ICU 78 (Unicode license), the fonts Inter, JetBrains Mono and Noto Sans CJK (SIL OFL 1.1)
  GCC runtime libraries (libstdc++, libgcc, winpthreads: GPL-3.0 with the Runtime Library Exception)
The license texts are in this folder. Sources of the LGPL parts: the projects' own releases, and remote/packaging/windows/ in Sodium Remote's source.
EOF
echo "staged: $OUT"

# 5. the zip
if [[ $NO_ZIP == 0 ]]; then
  ZIP=$REMOTE/build/SodiumRemote-$VERSION-win64.zip
  rm -f "$ZIP"
  (cd "$(dirname "$OUT")" && 7z a -tzip -mx=6 -bso0 -bsp0 "$ZIP" SodiumRemote)
  ls -la "$ZIP"
fi
