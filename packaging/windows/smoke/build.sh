#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# Cross-builds smoke.c with the deps in build/win-deps, copies the DLLs beside it and,
# with `run`, runs it under wine:   smoke/build.sh [run]
set -euo pipefail
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
. "$HERE/../env.sh"
OUT=$WIN_DEPS/../win-smoke
mkdir -p "$OUT"
PC=$HERE/../win-pkg-config
x86_64-w64-mingw32-gcc -O2 -Wall -Wno-deprecated-declarations -o "$OUT/smoke.exe" "$HERE/smoke.c" \
  $("$PC" --cflags --libs libavcodec libswscale libavutil opus)
cp "$WIN_DEPS"/bin/*.dll "$OUT/"
for d in libgcc_s_seh-1.dll libstdc++-6.dll libwinpthread-1.dll; do
  ldd_found=$(x86_64-w64-mingw32-objdump -p "$OUT/smoke.exe" "$OUT"/*.dll 2>/dev/null | grep -c "$d" || true)
  [[ $ldd_found -gt 0 && -f /usr/x86_64-w64-mingw32/bin/$d ]] && cp /usr/x86_64-w64-mingw32/bin/$d "$OUT/" || true
done
echo "built $OUT/smoke.exe"
if [[ ${1:-} == run ]]; then (cd "$OUT" && wine ./smoke.exe); fi
