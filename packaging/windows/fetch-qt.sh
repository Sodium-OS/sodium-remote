#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# Fetches Qt 6.12.1 for Windows (the official mingw x64 build) from download.qt.io's online
# repository, checks every archive, and unpacks it into remote/build/qt-win/6.12.1/mingw_64/.
#
#   remote/packaging/windows/fetch-qt.sh
#
# No installer and no helper tools: the repository is plain directories of 7z archives, each with
# a .sha1 beside it. Every archive is checked against that SHA-1 (published by Qt) and against the
# SHA-256 pinned below (taken when this version was first fetched), before it is unpacked.
# Needs: curl, 7z (p7zip), sha1sum, sha256sum. Idempotent (stamp file).
#
# What is fetched (all LGPL-3 / GPL-licensed Qt modules we link dynamically, no GPL-only ones):
#   qtbase, qtdeclarative, qtsvg, qtshadertools   the modules Sodium UI uses
#   opengl32sw.dll                                Mesa software OpenGL, the fallback when D3D11 has no adapter
# Qt's own MinGW 13.1 runtime archive is NOT used: the program is built with Arch's mingw-w64
# GCC (UCRT) and ships that GCC's runtime DLLs (README.md, "Qt and the C runtime").
set -euo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REMOTE=$(cd "$HERE/../.." && pwd)
QT_DIR=${QT_WIN_DIR:-$REMOTE/build/qt-win}
QT_VER=6.12.1
DL=$QT_DIR/dl
ROOT=$QT_DIR/$QT_VER/mingw_64
STAMP=$QT_DIR/.stamp-$QT_VER

BASE=https://download.qt.io/online/qtsdkrepository/windows_x86/desktop/qt6_6121/qt6_6121_mingw
V=6.12.1-0-202611180427
S=Windows-Windows_11_24H2-Mingw-Windows-Windows_11_24H2-X86_64.7z

# package directory | archive name | sha256
ARCHIVES=(
  "qt.qt6.6121.win64_mingw|qtbase-$S|3331d18efd754166872246e04e32e25809f748a487f04cf2ca3d7d37945e21ae"
  "qt.qt6.6121.win64_mingw|qtdeclarative-$S|e281137356b62dbb84eb8eaf52a6829a7ba16797965d89b1ac6c8586ccf41fe5"
  "qt.qt6.6121.win64_mingw|qtsvg-$S|e270a1f19a031429c2c717a63ddb2d2190f6ac78b25705eb9014c2e45915ea43"
  "qt.qt6.6121.addons.qtshadertools.win64_mingw|qtshadertools-$S|b21447c1e97990ae4270241778557dded8432af67956ddb469b68a398c1539df"
  "qt.qt6.6121.win64_mingw|opengl32sw-64-mesa_11_2_2-signed_sha256.7z|781b7d02cf8972df9a1e072520bf9a4923337b5a8cf3d128a84389760c0a2e91"
)

[[ -f $STAMP && -z ${FORCE:-} ]] && { echo "qt $QT_VER: already unpacked ($ROOT)"; exit 0; }
for t in curl 7z sha1sum sha256sum; do command -v "$t" >/dev/null || { echo "missing tool: $t" >&2; exit 1; }; done
mkdir -p "$DL"

for entry in "${ARCHIVES[@]}"; do
  IFS='|' read -r pkg name sha256 <<<"$entry"
  file=$DL/$name
  # the mirror redirects (-L); the .sha1 is Qt's own, from the same repository
  [[ -f $file ]] || curl -fsSL --retry 3 -o "$file" "$BASE/$pkg/$V$name"
  curl -fsSL --retry 3 -o "$file.sha1" "$BASE/$pkg/$V$name.sha1"
  echo "$(cut -c1-40 "$file.sha1")  $file" | sha1sum -c - >/dev/null || { echo "$name: SHA-1 differs from download.qt.io's" >&2; exit 1; }
  echo "$sha256  $file" | sha256sum -c - >/dev/null || { echo "$name: SHA-256 differs from the pinned one" >&2; exit 1; }
done

rm -rf "$QT_DIR/$QT_VER"
mkdir -p "$ROOT"
for entry in "${ARCHIVES[@]}"; do
  IFS='|' read -r pkg name sha256 <<<"$entry"
  7z x -y -bso0 -bsp0 -o"$ROOT" "$DL/$name"
done
touch "$STAMP"
echo "qt $QT_VER unpacked into $ROOT"
