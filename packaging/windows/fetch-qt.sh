#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# Fetches Qt 6.12.0 for Windows (the official mingw x64 build) from download.qt.io's online
# repository, checks every archive, and unpacks it into remote/build/qt-win/6.12.0/mingw_64/.
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
QT_VER=6.12.0
DL=$QT_DIR/dl
ROOT=$QT_DIR/$QT_VER/mingw_64
STAMP=$QT_DIR/.stamp-$QT_VER

BASE=https://download.qt.io/online/qtsdkrepository/windows_x86/desktop/qt6_6120/qt6_6120_mingw
V=6.12.0-0-202609280346
S=Windows-Windows_11_24H2-Mingw-Windows-Windows_11_24H2-X86_64.7z

# package directory | archive name | sha256
ARCHIVES=(
  "qt.qt6.6120.win64_mingw|qtbase-$S|3356346505113242a4f7c0b09ba46188424ee7938232166655f794c28a3477fc"
  "qt.qt6.6120.win64_mingw|qtdeclarative-$S|a567789b9c142da546db76ceaa47aa0ee92bb780d7102d1adb905d39a3bf441b"
  "qt.qt6.6120.win64_mingw|qtsvg-$S|8dd193d39e21395f8fcb802f543f6e43e13a5caf5dd6f94cab731a53c598496a"
  "qt.qt6.6120.addons.qtshadertools.win64_mingw|qtshadertools-$S|5bca307511529d5548c09ce8de11d68fc14ea1597eb1e5a219361ea24a560992"
  "qt.qt6.6120.win64_mingw|opengl32sw-64-mesa_11_2_2-signed_sha256.7z|f51918c465a229efb528e26ea4e74e433663fac99a9296accdfbb090e48ac3b8"
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
