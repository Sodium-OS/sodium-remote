#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# Gathers the Windows client for copying into the VM: remote-client.exe, the FFmpeg DLLs
# and libwinpthread, the licenses, into remote/build/win-client/.
#   stage-client.sh [--build]     (--build runs the cross build first)
set -euo pipefail
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
remote=$(cd "$here/../.." && pwd)
. "$here/env.sh"
[[ ${1:-} == --build ]] && "$here/cargo-win.sh" build --release -p client-windows
exe=$remote/target/x86_64-pc-windows-gnu/release/remote-client.exe
[[ -f $exe ]] || { echo "no $exe: run with --build" >&2; exit 1; }
out=$remote/build/win-client
rm -rf "$out"
mkdir -p "$out"
cp "$exe" "$out/"
cp "$WIN_DEPS"/bin/*.dll "$out/"
cp /usr/x86_64-w64-mingw32/bin/libwinpthread-1.dll "$out/"
[[ -d $here/licenses ]] && cp -r "$here/licenses" "$out/licenses"
x86_64-w64-mingw32-strip --strip-unneeded "$out/remote-client.exe" 2>/dev/null || true
echo "staged in $out:"
ls -la "$out"
