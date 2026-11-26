#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# Builds Sodium Remote for Windows (SodiumRemote.exe) and stages the portable package:
#   remote/build/win-app/SodiumRemote/                  the program, Qt, ICU, FFmpeg/Opus DLLs, QML, fonts, icons, translations, licenses
#   remote/build/SodiumRemote-<version>-win64.zip       the same, zipped (portable; no installer, no code signing)
#
#   stage-app.sh [--no-ui] [--no-zip] [--no-notices]
#
# Needs what build-sodiumui.sh needs (Qt for Windows from fetch-qt.sh, ICU, win-deps from build-deps.sh, the Linux Qt 6.12
# for moc/rcc/qsb), the cargo target x86_64-pc-windows-gnu, zip and the app's own icons (app/data/icons). Jobs: JOBS (default 8). Output only under remote/build/.
set -euo pipefail
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REMOTE=$(cd "$HERE/../.." && pwd)
REPO=$REMOTE
. "$HERE/env.sh"
export QT_WIN_ROOT=${QT_WIN_ROOT:-$REMOTE/build/qt-win/6.12.1/mingw_64}
JOBS=${JOBS:-8}
UI_BUILD=${SODIUMUI_WIN_BUILD:-$REMOTE/build/sodiumui-win}
APP_BUILD=$REMOTE/build/app-win
OUT=$REMOTE/build/win-app/SodiumRemote
ICONS=${ICONS:-$REMOTE/app/data/icons/sodium-remote}
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' "$REMOTE/Cargo.toml" | head -1)
NO_UI=0 NO_ZIP=0 NO_NOTICES=0   # --no-notices: no cargo-about list (a local test run); a release never uses it
for a in "$@"; do
  case $a in --no-ui) NO_UI=1 ;; --no-zip) NO_ZIP=1 ;; --no-notices) NO_NOTICES=1 ;; *) echo "unknown option $a" >&2; exit 2 ;; esac
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
  --qml-dir "$REMOTE/app/qml" --no-gallery --catalog remote --out "$OUT" --qt "$QT_WIN_ROOT" --deps "$WIN_DEPS" --repo "$REPO" --icons "$ICONS" \
  --qt-licenses "${QT_LICENSES:-/usr/share/licenses/qt6-base}" --qt-sbom "$QT_WIN_ROOT/sbom" --qt-sbom "$REMOTE/build/qt-win/6.12.1/mingw_64/sbom"
mkdir -p "$OUT/licenses"
cp -r "$HERE/licenses/." "$OUT/licenses/"
cp "$REMOTE/LICENSE" "$REMOTE/LICENSING.md" "$REMOTE/TRADEMARKS.md" "$OUT/licenses/"
# The matched display's driver, when a signed build is there (sudovda/build-sign.ps1 in a VM, copied back, then
# sudovda/pin.sh): SudoVDA.inf, SudoVDA.dll, sudovda.cat and our certificate, as drivers\sudovda\. The app checks
# each file against the pins in host-desktop/src/vdd_pin.rs before installing anything.
SUDOVDA=${SUDOVDA_DIR:-$REMOTE/build/sudovda}
rm -rf "$OUT/drivers/sudovda"
if [[ -f $SUDOVDA/sudovda.cat && -f $HERE/sudovda/sodium-remote-driver-signing.cer ]]; then
  mkdir -p "$OUT/drivers/sudovda"
  cp "$SUDOVDA/SudoVDA.inf" "$SUDOVDA/SudoVDA.dll" "$SUDOVDA/sudovda.cat" "$HERE/sudovda/sodium-remote-driver-signing.cer" "$OUT/drivers/sudovda/"
  echo "staged the virtual display driver from $SUDOVDA"
fi
QT_VER=$(sed -n 's/^QT_VER=//p' "$HERE/build-qt-ucrt.sh" | head -1)
FFMPEG_VER=$(sed -n 's/^FFMPEG_VER=\([0-9.]*\).*/\1/p' "$HERE/build-deps.sh" | head -1)
TAG=v$VERSION
cat >"$OUT/licenses/README.txt" <<EOF
Sodium Remote $VERSION for Windows: the third-party software in this folder, its licenses and its sources
==========================================================================================================

Sodium Remote itself is AGPL-3.0-only (licenses/LICENSE, LICENSING.md). The full text of every license below is in this
licenses folder, and the same texts follow the Rust libraries' list in THIRD-PARTY-NOTICES.md next to the program.

File(s)                                              License                                          Text in licenses/
---------------------------------------------------  -----------------------------------------------  ---------------------------------------------
Qt6*.dll, platforms/, imageformats/, iconengines/,   LGPL-3.0 (Qt $QT_VER: Core, Gui, Network, Qml,     Qt/LGPL-3.0-only.txt, Qt/GPL-3.0-only.txt,
  qml/**/*.dll (Qt Quick, Controls, Dialogs, ...)    Quick, Controls 2, Dialogs 2, Layouts, Svg)      Qt/*.txt, Qt-third-party-attributions.txt
avcodec-*.dll, avutil-*.dll, swscale-*.dll           LGPL-2.1-or-later (FFmpeg $FFMPEG_VER, no GPL or     FFmpeg-LGPL-2.1.txt, FFmpeg-LICENSE.md
                                                     nonfree parts)
icuuc78.dll, icuin78.dll, icudt78.dll                Unicode license (ICU 78)                         ICU-LICENSE.txt
libgcc_s_seh-1.dll, libstdc++-6.dll                  GPL-3.0 with the GCC Runtime Library Exception   GCC-runtime-GPL-3.0.txt,
                                                     3.1                                              GCC-Runtime-Library-Exception-3.1.txt
libwinpthread-1.dll (and the MinGW-w64 run-time      MIT and BSD-style (mingw-w64, winpthreads)       MinGW-w64-runtime-and-winpthreads.txt
  linked into the program)
opengl32sw.dll (software OpenGL fallback)            MIT (Mesa 11.2.2), NCSA (LLVM 3.6.2)             Mesa-opengl32sw-NOTICE.txt
SodiumRemote.exe: Opus 1.6 (static)                  BSD-3-Clause                                     opus-BSD-3-Clause.txt
SodiumRemote.exe: Intel libvpl, NVIDIA codec         MIT                                              libvpl-MIT.txt, nv-codec-headers-MIT.txt,
  headers, AMD AMF headers (static / headers)                                                         AMF-MIT.txt
SodiumRemote.exe: OpenH264 headers                   BSD-2-Clause                                     openh264-headers-BSD-2-Clause.txt
fonts: Inter, JetBrains Mono, Noto Sans CJK          SIL OFL 1.1                                      Inter-OFL.txt, JetBrainsMono-OFL.txt,
                                                                                                      NotoSansCJK-OFL.txt
drivers\sudovda\ (when present): SudoVDA, the        MIT and CC0 (SudoMaker, VirtualDrivers), MS-PL   SudoVDA-MIT.txt, SudoVDA-MS-PL.txt
  virtual display driver, a separate program         (Microsoft's IddSampleDriver)

Not in this package:
  OpenH264 Video Codec provided by Cisco Systems, Inc. Cisco's own prebuilt DLL is downloaded when it is first needed
  (OpenH264-Cisco-NOTICE.txt). d3dcompiler_47.dll is not shipped: Windows 10 and 11 have it in System32 and Qt loads it there.

Sources (LGPL): the release this zip came from, https://github.com/sodium-os/sodium-remote/releases/tag/$TAG, has these as
assets, the exact archives the DLLs were built from (SHA256SUMS on the release lists them):
  qtbase-everywhere-src-$QT_VER.tar.xz, qtshadertools-everywhere-src-$QT_VER.tar.xz,
  qtdeclarative-everywhere-src-$QT_VER.tar.xz, qtsvg-everywhere-src-$QT_VER.tar.xz   (Qt, built unmodified)
  ffmpeg-$FFMPEG_VER.tar.xz                                                          (FFmpeg, built unmodified)
Qt is built with packaging/windows/build-qt-ucrt.sh and FFmpeg with packaging/windows/build-deps.sh (its configure options
are also in avcodec_configuration()), both in Sodium Remote's source: the release's source tarball, or
https://github.com/sodium-os/sodium-remote (folder packaging/windows). Qt and FFmpeg are shared DLLs next to the program:
you can replace them with your own build of the same version.
EOF
if [[ $NO_NOTICES == 0 ]]; then
  (cd "$REMOTE" && cargo about generate --locked about.hbs --manifest-path client-qt/Cargo.toml --target x86_64-pc-windows-gnu \
    --output-file "$OUT/THIRD-PARTY-NOTICES.md")
  python3 -I "$HERE/package-licenses.py" append "$OUT"
fi
python3 -I "$HERE/package-licenses.py" check "$OUT"
echo "staged: $OUT"

# 5. the zip
if [[ $NO_ZIP == 0 ]]; then
  ZIP=$REMOTE/build/SodiumRemote-$VERSION-win64.zip
  rm -f "$ZIP"
  (cd "$(dirname "$OUT")" && 7z a -tzip -mx=6 -bso0 -bsp0 "$ZIP" SodiumRemote)
  ls -la "$ZIP"
fi
